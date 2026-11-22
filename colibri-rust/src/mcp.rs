use crate::messages::{ToolCall, ToolResult};
use crate::session::{bound_text, SessionToolExtension};
use crate::tools::ToolContext;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const VERSION: &str = "2025-06-18";
const MAX_MESSAGE: u64 = 4 * 1024 * 1024;

#[derive(Clone, Deserialize)]
pub struct Server {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub url: Option<String>,
    pub command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    token: String,
}

pub fn servers(value: &Value) -> Result<Vec<Server>, String> {
    let entries: Vec<Server> =
        serde_json::from_value(value.clone()).map_err(|_| "MCP 服务器配置格式错误")?;
    let mut names = BTreeSet::new();
    if entries.len() > 8 {
        return Err("最多配置 8 个 MCP 服务".into());
    }
    for server in &entries {
        if server.name.is_empty()
            || !server
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            || !names.insert(&server.name)
        {
            return Err("MCP 服务名称无效或重复".into());
        }
        match (&server.url, &server.command) {
            (Some(url), None) => {
                crate::http::http_endpoint(url)?;
            }
            (None, Some(command)) if !command.trim().is_empty() => (),
            _ => return Err("MCP 服务需配置一个 URL 或 command".into()),
        }
    }
    Ok(entries)
}

struct StdioConnection {
    child: Child,
    input: ChildStdin,
    messages: mpsc::Receiver<Value>,
}

impl StdioConnection {
    fn start(server: &Server, command: &str) -> Result<Self, String> {
        let mut child = Command::new(command)
            .args(&server.args)
            .envs(&server.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|e| format!("无法启动 MCP 进程 ({})", e.kind()))?;
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, messages) = mpsc::sync_channel(16);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                let Ok(size) = reader.by_ref().take(MAX_MESSAGE + 1).read_line(&mut line) else {
                    break;
                };
                if size == 0 || size as u64 > MAX_MESSAGE {
                    break;
                }
                let Ok(value) = serde_json::from_str(&line) else {
                    break;
                };
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        let peer = Self {
            child,
            input,
            messages,
        };
        let fd = peer.input.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err("无法设置 MCP 进程管道".into());
        }
        Ok(peer)
    }

    fn send(&mut self, value: &Value, deadline: Instant) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
        if bytes.len() as u64 >= MAX_MESSAGE {
            return Err("MCP 请求超出大小上限".into());
        }
        bytes.push(b'\n');
        let mut remaining = &bytes[..];
        while !remaining.is_empty() {
            let wait = deadline.saturating_duration_since(Instant::now());
            if wait.is_zero() {
                return Err("MCP 请求超时".into());
            }
            match self.input.write(remaining) {
                Ok(0) => return Err("MCP 进程无法接收消息".into()),
                Ok(written) => remaining = &remaining[written..],
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    let mut descriptor = libc::pollfd {
                        fd: self.input.as_raw_fd(),
                        events: libc::POLLOUT,
                        revents: 0,
                    };
                    let millis = wait.as_millis().max(1).min(i32::MAX as u128) as i32;
                    if unsafe { libc::poll(&mut descriptor, 1, millis) } < 0
                        && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
                    {
                        return Err("MCP 进程管道异常".into());
                    }
                }
                Err(_) => return Err("MCP 进程无法接收消息".into()),
            }
        }
        Ok(())
    }

    fn receive(&mut self, id: u64, deadline: Instant) -> Result<Value, String> {
        loop {
            let value = self
                .messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|e| match e {
                    mpsc::RecvTimeoutError::Timeout => "MCP 响应超时",
                    mpsc::RecvTimeoutError::Disconnected => "MCP 进程提前退出",
                })?;
            if value["method"].is_string() {
                if let Some(request_id) = value.get("id") {
                    let reply = if value["method"] == "ping" {
                        json!({"jsonrpc":"2.0","id":request_id,"result":{}})
                    } else {
                        json!({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"Method not found"}})
                    };
                    self.send(&reply, deadline)?;
                }
                continue;
            }
            if value["id"].as_u64() == Some(id) {
                return rpc_result(value);
            }
        }
    }
}

impl Drop for StdioConnection {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

struct HttpConnection {
    agent: ureq::Agent,
    url: String,
    token: String,
    session: Option<String>,
    version: Option<String>,
}

impl HttpConnection {
    fn request(&self, method: &str) -> ureq::Request {
        let mut request = self
            .agent
            .request(method, &self.url)
            .set("Accept", "application/json, text/event-stream");
        if !self.token.is_empty() {
            request = request.set("Authorization", &format!("Bearer {}", self.token));
        }
        if let Some(session) = &self.session {
            request = request.set("Mcp-Session-Id", session);
        }
        if let Some(version) = &self.version {
            request = request.set("MCP-Protocol-Version", version);
        }
        request
    }

    fn post(&mut self, value: &Value, id: Option<u64>, deadline: Instant) -> Result<Value, String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("MCP 响应超时".into());
        }
        let response = self
            .request("POST")
            .set("Content-Type", "application/json")
            .timeout(remaining)
            .send_string(&value.to_string())
            .map_err(|e| match e {
                ureq::Error::Status(code, _) => format!("MCP HTTP {code}"),
                _ => "MCP HTTP 连接或响应失败".into(),
            })?;
        if let Some(session) = response.header("Mcp-Session-Id") {
            self.session = Some(session.to_owned());
        }
        let Some(id) = id else { return Ok(Value::Null) };
        let sse = response
            .header("Content-Type")
            .unwrap_or_default()
            .starts_with("text/event-stream");
        let mut reader = BufReader::new(response.into_reader().take(MAX_MESSAGE + 1));
        if !sse {
            let value: Value = serde_json::from_reader(reader).map_err(|_| "MCP JSON 响应无效")?;
            if value["id"].as_u64() != Some(id) {
                return Err("MCP 响应 ID 不匹配".into());
            }
            return rpc_result(value);
        }
        let mut data = String::new();
        let mut line = String::new();
        loop {
            line.clear();
            if reader
                .read_line(&mut line)
                .map_err(|_| "MCP SSE 响应中断")?
                == 0
            {
                break;
            }
            let line = line.trim_end_matches(['\r', '\n']);
            if let Some(value) = line.strip_prefix("data:") {
                data.push_str(value.strip_prefix(' ').unwrap_or(value));
                data.push('\n');
            } else if line.is_empty() && !data.is_empty() {
                let packet: Value = serde_json::from_str(&data).map_err(|_| "MCP SSE 数据无效")?;
                if packet["id"].as_u64() == Some(id) {
                    return rpc_result(packet);
                }
                data.clear();
            }
        }
        Err("MCP 响应缺少对应的结果".into())
    }
}

impl Drop for HttpConnection {
    fn drop(&mut self) {
        if self.session.is_some() {
            let _ = self
                .request("DELETE")
                .timeout(Duration::from_secs(1))
                .call();
        }
    }
}

enum Transport {
    Stdio(StdioConnection),
    Http(HttpConnection),
}

struct Connection {
    transport: Transport,
    next_id: u64,
    deadline: Instant,
}

impl Connection {
    fn connect(server: &Server) -> Result<Self, String> {
        let transport = if let Some(command) = &server.command {
            Transport::Stdio(StdioConnection::start(server, command)?)
        } else {
            Transport::Http(HttpConnection {
                agent: ureq::AgentBuilder::new().redirects(0).build(),
                url: server.url.clone().ok_or("缺少 MCP URL")?,
                token: server.token.clone(),
                session: None,
                version: None,
            })
        };
        let mut connection = Self {
            transport,
            next_id: 1,
            deadline: Instant::now() + Duration::from_secs(30),
        };
        let result = connection.call(
            "initialize",
            json!({
                "protocolVersion":VERSION,"capabilities":{},
                "clientInfo":{"name":"colibri-cardputer","version":"0.1"}
            }),
        )?;
        let version = result["protocolVersion"]
            .as_str()
            .filter(|v| {
                matches!(
                    *v,
                    "2025-03-26" | "2025-06-18" | "2025-11-25" | "2024-11-05"
                )
            })
            .ok_or("MCP 协议版本不兼容")?;
        let notification = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        match &mut connection.transport {
            Transport::Stdio(peer) => peer.send(&notification, connection.deadline)?,
            Transport::Http(peer) => {
                peer.version = Some(version.into());
                peer.post(&notification, None, connection.deadline)?;
            }
        }
        Ok(connection)
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let packet = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        match &mut self.transport {
            Transport::Stdio(peer) => {
                peer.send(&packet, self.deadline)?;
                peer.receive(id, self.deadline)
            }
            Transport::Http(peer) => peer.post(&packet, Some(id), self.deadline),
        }
    }

    fn list(&mut self, method: &str, key: &str) -> Result<Value, String> {
        let mut items = Vec::new();
        let mut seen = BTreeSet::new();
        let mut params = json!({});
        loop {
            let result = self.call(method, params)?;
            items.extend(
                result[key]
                    .as_array()
                    .ok_or("MCP 列表响应无效")?
                    .iter()
                    .cloned(),
            );
            let Some(cursor) = result["nextCursor"].as_str() else {
                return Ok(Value::Array(items));
            };
            if seen.len() >= 128 || !seen.insert(cursor.to_owned()) {
                return Err("MCP 分页游标重复或页数超限".into());
            }
            params = json!({"cursor":cursor});
        }
    }
}

fn rpc_result(mut packet: Value) -> Result<Value, String> {
    if let Some(code) = packet["error"]["code"].as_i64() {
        return Err(format!("MCP JSON-RPC 错误 ({code})"));
    }
    packet
        .as_object_mut()
        .and_then(|value| value.remove("result"))
        .ok_or("MCP 响应缺少结果".into())
}

pub fn operation(
    server: &Server,
    operation: &str,
    name: &str,
    arguments: Value,
) -> Result<Value, String> {
    let mut connection = Connection::connect(server)?;
    match operation {
        "list_tools" => connection.list("tools/list", "tools"),
        "list_resources" => connection.list("resources/list", "resources"),
        "list_prompts" => connection.list("prompts/list", "prompts"),
        "call_tool" => connection.call("tools/call", json!({"name":name,"arguments":arguments})),
        "read_resource" => connection.call("resources/read", json!({"uri":name})),
        "get_prompt" => connection.call("prompts/get", json!({"name":name,"arguments":arguments})),
        _ => Err("MCP 操作无效".into()),
    }
}

#[derive(Default)]
pub struct Extensions {
    tools: Vec<(Server, String, Value)>,
}

impl Extensions {
    pub fn discover(servers: &[Server], notice: impl Fn(String)) -> Self {
        let mut extensions = Self::default();
        for server in servers {
            match operation(server, "list_tools", "", json!({})) {
                Ok(tools) => {
                    for tool in tools.as_array().into_iter().flatten() {
                        if let Some(name) = tool["name"].as_str() {
                            let full_name = format!("mcp.{}.{name}", server.name);
                            let spec = json!({"type":"function","function":{
                                "name":full_name,"description":tool["description"].as_str().unwrap_or(name),
                                "parameters":tool["inputSchema"]
                            }});
                            extensions.tools.push((server.clone(), name.into(), spec));
                        }
                    }
                }
                Err(error) => notice(format!("MCP {} 连接失败 ({error})", server.name)),
            }
        }
        extensions
    }
}

impl SessionToolExtension for Extensions {
    fn specs(&self) -> Vec<Value> {
        self.tools.iter().map(|(_, _, spec)| spec.clone()).collect()
    }

    fn run(&self, call: &ToolCall, context: &ToolContext) -> Option<ToolResult> {
        if !call.name.starts_with("mcp.") {
            return None;
        }
        let Some((server, name, _)) = self
            .tools
            .iter()
            .find(|(_, _, spec)| spec["function"]["name"] == call.name)
        else {
            return Some(ToolResult::error("unknown_tool", "未发现此 MCP 工具"));
        };
        let result = match operation(server, "call_tool", name, json!(call.arguments)) {
            Ok(result) => result,
            Err(error) => return Some(ToolResult::error("mcp_error", error)),
        };
        let value = if result["structuredContent"].is_null() {
            json!(result["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>())
        } else {
            result["structuredContent"].clone()
        };
        let raw = value.to_string();
        let text = bound_text(&raw, context.config.tools.max_result_chars);
        let mut output = if result["isError"] == true {
            ToolResult::error("mcp_error", text.clone())
        } else {
            ToolResult::ok(text.clone())
        };
        output.truncated = text != raw;
        Some(output)
    }
}
