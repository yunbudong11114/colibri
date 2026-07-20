use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use crate::config::{expand_user_path, AgentConfig};
use crate::http::request_json;
use crate::memory::truncate;
use crate::messages::{MediaPart, ToolResult};
use crate::shell_policy::denied_shell_executable;
use crate::skills::run_skill_command;

static TOOL_SPECS_CACHE: Mutex<Option<(Vec<String>, Vec<serde_json::Value>)>> = Mutex::new(None);
const BAIDU_DEFAULT_SEARCH_ENDPOINT: &str = "https://qianfan.baidubce.com/v2/ai_search/web_search";
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const ALIYUN_MCP_TOOL_NAME: &str = "bailian_web_search";

#[derive(Clone)]
pub struct ToolContext {
    pub config: Arc<AgentConfig>,
    pub cwd: PathBuf,
    pub allowed_file_roots: Vec<PathBuf>,
    pub media_sender: Option<Arc<dyn Fn(MediaPart) -> Result<(), String> + Send + Sync>>,
    pub image_analyzer: Option<Arc<dyn Fn(&Path, &str) -> Result<String, String> + Send + Sync>>,
}

#[derive(Clone, Debug)]
pub struct ToolInfo {
    pub name: String,
    pub read_only: bool,
}

impl ToolInfo {
    pub fn new(name: &str, read_only: bool) -> Self {
        Self {
            name: name.to_string(),
            read_only,
        }
    }
}

pub fn tool_info(name: &str) -> ToolInfo {
    ToolInfo::new(
        name,
        matches!(
            name,
            "files.list"
                | "files.read"
                | "memory.list"
                | "memory.read"
                | "memory.search"
                | "skill.read"
                | "image.understand"
        ),
    )
}

impl ToolContext {
    pub fn new(config: impl Into<Arc<AgentConfig>>, cwd: PathBuf) -> Self {
        Self {
            config: config.into(),
            cwd,
            allowed_file_roots: Vec::new(),
            media_sender: None,
            image_analyzer: None,
        }
    }

    pub fn with_allowed_file_root(&self, root: PathBuf) -> Self {
        let mut next = self.clone();
        next.allowed_file_roots = vec![root];
        next
    }

    pub fn with_media_sender(
        mut self,
        sender: Arc<dyn Fn(MediaPart) -> Result<(), String> + Send + Sync>,
    ) -> Self {
        self.media_sender = Some(sender);
        self
    }

    pub fn with_image_analyzer(
        mut self,
        analyzer: Arc<dyn Fn(&Path, &str) -> Result<String, String> + Send + Sync>,
    ) -> Self {
        self.image_analyzer = Some(analyzer);
        self
    }
}

pub fn tool_specs() -> Vec<serde_json::Value> {
    tool_specs_for_enabled(&[
        "files".to_string(),
        "shell".to_string(),
        "memory".to_string(),
        "skills".to_string(),
        "web".to_string(),
        "image".to_string(),
    ])
}

pub fn tool_specs_for_config(config: &AgentConfig) -> Vec<serde_json::Value> {
    tool_specs_for_enabled(&config.tools.enabled)
}

fn tool_specs_for_enabled(enabled: &[String]) -> Vec<serde_json::Value> {
    if let Ok(cache) = TOOL_SPECS_CACHE.lock() {
        if let Some((key, specs)) = cache.as_ref() {
            if key.as_slice() == enabled {
                return specs.clone();
            }
        }
    }
    let specs = build_tool_specs(enabled);
    if let Ok(mut cache) = TOOL_SPECS_CACHE.lock() {
        *cache = Some((enabled.to_vec(), specs.clone()));
    }
    specs
}

fn build_tool_specs(enabled: &[String]) -> Vec<serde_json::Value> {
    let has = |name: &str| enabled.iter().any(|item| item == name);
    let mut specs = Vec::new();
    if has("files") {
        specs.extend(files_tool_specs());
    }
    if has("memory") {
        specs.extend(memory_tool_specs());
    }
    if has("shell") {
        specs.push(openai_tool(
            "shell.run",
            "Run a shell command after Colibri permission approval. Do not use this to create or edit files; use files.write for generated artifacts and text file changes.",
            serde_json::json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}),
        ));
    }
    if has("web") {
        specs.push(openai_tool(
            "web.search",
            "Search the web using the configured search engine.",
            serde_json::json!({
                "type":"object",
                "properties":{
                    "query":{"type":"string","description":"Search query."},
                    "count":{"type":"integer","description":"Maximum number of web results. Defaults to config.web_search.max_results."},
                    "freshness":{"type":"string","description":"Optional freshness filter: pd, pw, pm, py, or YYYY-MM-DDtoYYYY-MM-DD."}
                },
                "required":["query"],
                "additionalProperties":false
            }),
        ));
    }
    if has("image") {
        specs.push(openai_tool(
            "image.understand",
            "Understand a local image with the configured vision model. Use the optional prompt to ask what should be inspected or extracted.",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"prompt":{"type":"string"}},"required":["path"]}),
        ));
    }
    if has("skills") {
        specs.push(openai_tool(
            "skill.read",
            "Read the full SKILL.md instructions for a skill listed in the catalog. Prefer this over guessing skill contents.",
            serde_json::json!({"type":"object","properties":{"name":{"type":"string","description":"Exact skill name from the catalog."}},"required":["name"]}),
        ));
        specs.push(openai_tool(
            "skill.run",
            "Run a command declared in a skill's SKILL.md YAML frontmatter. Prefer this tool whenever a configured command matches the requested action; do not invoke the underlying executable through shell.run.",
            serde_json::json!({"type":"object","properties":{"skill":{"type":"string"},"command":{"type":"string"},"args":{"type":"array","items":{"type":"string"}}},"required":["skill","command"]}),
        ));
    }
    specs
}

fn files_tool_specs() -> Vec<serde_json::Value> {
    vec![
        openai_tool(
            "files.list",
            "List direct children of an allowed directory.",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        ),
        openai_tool(
            "files.read",
            "Read a UTF-8 text file under an allowed root. Prefer start_line/end_line ranges for large files. Optional max_chars caps this read result and is itself capped by tools.max_result_chars.",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"start_line":{"type":"integer","minimum":1},"end_line":{"type":"integer","minimum":1},"max_chars":{"type":"integer","minimum":1}},"required":["path"]}),
        ),
        openai_tool(
            "files.write",
            "Write a UTF-8 text file under an allowed root. Use this for generated artifacts and file edits; do not use shell redirection or heredocs to create files.",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}),
        ),
        openai_tool(
            "files.send",
            "Send a local file to the current chat channel. This can expose host files outside Colibri, so use it only when the user asked to send a file.",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"caption":{"type":"string"}},"required":["path"]}),
        ),
    ]
}

fn memory_tool_specs() -> Vec<serde_json::Value> {
    vec![
        openai_tool("memory.list", "List available memory files.", serde_json::json!({"type":"object","properties":{}})),
        openai_tool(
            "memory.read",
            "Read SOUL.md, USER.md, MEMORY.md, INDEX.md, or a topic memory file.",
            serde_json::json!({"type":"object","properties":{"file":{"type":"string"},"topic":{"type":"string"}}}),
        ),
        openai_tool(
            "memory.search",
            "Search INDEX.md memory manifest lines by keyword.",
            serde_json::json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}),
        ),
        openai_tool(
            "memory.write",
            "Append to or replace an allowed memory file: SOUL.md, USER.md, MEMORY.md, INDEX.md, or topics/<name>.md. Memory files must use frontmatter:\n---\ntype: soul|user|feedback|project|reference|system\ndescription: one-line description\nupdated: YYYY-MM-DD\n---",
            serde_json::json!({"type":"object","properties":{"file":{"type":"string"},"topic":{"type":"string"},"content":{"type":"string"},"mode":{"type":"string","enum":["append","replace"]}},"required":["content"]}),
        ),
    ]
}

fn openai_tool(name: &str, description: &str, parameters: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters
        }
    })
}

pub fn run_tool(
    name: &str,
    arguments_json: &str,
    context: &ToolContext,
) -> Result<ToolResult, String> {
    let values = parse_json_object(arguments_json)?;
    let args = string_arguments(&values);
    run_tool_map(name, &args, context)
}

pub fn run_tool_map(
    name: &str,
    args: &BTreeMap<String, String>,
    context: &ToolContext,
) -> Result<ToolResult, String> {
    let result = match name {
        "files.list" => files_list(args, context),
        "files.read" => files_read(args, context),
        "files.write" => files_write(args, context),
        "files.send" => files_send(args, context),
        "shell.run" => shell_run(args, context),
        "memory.list" => memory_list(context),
        "memory.read" => memory_read(args, context),
        "memory.search" => memory_search(args, context),
        "memory.write" => memory_write(args, context),
        "skill.read" => crate::skills::read_skill(args, context),
        "skill.run" => run_skill_command(args, context),
        "web.search" => web_search(args, context),
        "image.understand" => image_understand(args, context),
        _ => ToolResult::error("unknown_tool", format!("Unknown tool: {}", name)),
    };
    Ok(result)
}

fn files_list(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let path = resolve_arg_path(args.get("path").map(String::as_str).unwrap_or("."), context);
    if !is_allowed(&path, context) {
        return ToolResult::error(
            "permission_denied",
            format!("Path is outside allowed roots: {}", path.display()),
        );
    }
    let entries = match fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(error) => return ToolResult::error("io_error", error.to_string()),
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let mut name = entry.file_name().to_string_lossy().to_string();
        if entry.path().is_dir() {
            name.push('/');
        }
        names.push(name);
    }
    names.sort();
    ToolResult::ok(names.join("\n"))
}

fn files_read(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some(raw_path) = args.get("path").filter(|value| !value.is_empty()) else {
        return ToolResult::error("invalid_arguments", "Missing path");
    };
    let path = resolve_arg_path(raw_path, context);
    if !is_allowed(&path, context) {
        return ToolResult::error(
            "permission_denied",
            format!("Path is outside allowed roots: {}", path.display()),
        );
    }
    if !path.exists() {
        return ToolResult::error("not_found", "Path does not exist");
    }
    if !path.is_file() {
        return ToolResult::error("not_file", "Path is not a file");
    }
    let Some((start_line, end_line, max_chars)) = read_range_args(args) else {
        return ToolResult::error("invalid_arguments", "Invalid line range or max_chars");
    };
    if let (Some(start), Some(end)) = (start_line, end_line) {
        if start > end {
            return ToolResult::error("invalid_arguments", "start_line must be <= end_line");
        }
    }
    match fs::read_to_string(&path) {
        Ok(mut text) => {
            if start_line.is_some() || end_line.is_some() {
                text = select_line_range(&text, start_line, end_line);
            }
            let limit = max_chars
                .unwrap_or(context.config.tools.max_result_chars)
                .min(context.config.tools.max_result_chars);
            let (text, truncated) = truncate(text, limit);
            let mut result = ToolResult::ok(text);
            result.truncated = truncated;
            result
        }
        Err(error) => ToolResult::error("io_error", error.to_string()),
    }
}

fn read_range_args(
    args: &BTreeMap<String, String>,
) -> Option<(Option<usize>, Option<usize>, Option<usize>)> {
    Some((
        positive_usize_arg(args, "start_line")?,
        positive_usize_arg(args, "end_line")?,
        positive_usize_arg(args, "max_chars")?,
    ))
}

fn positive_usize_arg(args: &BTreeMap<String, String>, name: &str) -> Option<Option<usize>> {
    let Some(value) = args.get(name) else {
        return Some(None);
    };
    let parsed = value.parse::<usize>().ok()?;
    if parsed == 0 {
        return None;
    }
    Some(Some(parsed))
}

fn select_line_range(text: &str, start_line: Option<usize>, end_line: Option<usize>) -> String {
    let start = start_line.unwrap_or(1).saturating_sub(1);
    let end = end_line.unwrap_or(usize::MAX);
    text.split_inclusive('\n')
        .enumerate()
        .filter_map(|(index, segment)| {
            let line_no = index + 1;
            if line_no > start && line_no <= end {
                Some(segment)
            } else {
                None
            }
        })
        .collect::<String>()
}

fn files_write(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some(raw_path) = args.get("path").filter(|value| !value.is_empty()) else {
        return ToolResult::error("invalid_arguments", "Missing path");
    };
    let path = resolve_arg_path(raw_path, context);
    if !is_allowed(&path, context) {
        return ToolResult::error(
            "permission_denied",
            format!("Path is outside allowed roots: {}", path.display()),
        );
    }
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            return ToolResult::error("io_error", error.to_string());
        }
    }
    let content = args.get("content").cloned().unwrap_or_default();
    match fs::write(&path, &content) {
        Ok(()) => ToolResult::ok(format!(
            "Wrote {} bytes to {}",
            content.as_bytes().len(),
            path.display()
        )),
        Err(error) => ToolResult::error("io_error", error.to_string()),
    }
}

fn files_send(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    if context.media_sender.is_none() {
        return ToolResult::error(
            "media_unavailable",
            "No active channel can send files in this session",
        );
    }
    let Some(raw_path) = args.get("path").filter(|value| !value.is_empty()) else {
        return ToolResult::error("invalid_arguments", "Missing path");
    };
    let path = resolve_arg_path(raw_path, context);
    if !is_allowed(&path, context) {
        return ToolResult::error("permission_denied", "Path is outside allowed roots");
    }
    if !path.exists() {
        return ToolResult::error("not_found", "Path does not exist");
    }
    if !path.is_file() {
        return ToolResult::error("not_file", "Path is not a file");
    }
    let content_type = content_type_for_path(&path);
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let media = MediaPart::new(
        media_type_for_content(&content_type),
        path,
        filename.clone(),
        content_type,
        args.get("caption").cloned().unwrap_or_default(),
    );
    ToolResult::ok(format!("Sent file to channel: {}", filename)).with_media(media)
}

fn shell_run(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let command = args.get("command").cloned().unwrap_or_default();
    if command.trim().is_empty() {
        return ToolResult::error("invalid_arguments", "Missing command");
    }
    match shell_words::split(&command) {
        Ok(argv) if !argv.is_empty() => argv,
        Ok(_) => return ToolResult::error("invalid_arguments", "Missing command"),
        Err(error) => return ToolResult::error("invalid_arguments", error.to_string()),
    };
    if denied_shell_executable(&command, &context.config.shell.deny).is_some() {
        return ToolResult::error("permission_denied", "Command is denied");
    }
    let mut process = platform_shell_command(&command);
    process
        .current_dir(&context.cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match run_command_with_timeout(process, context.config.tools.max_shell_seconds) {
        Err(CommandRunError::Timeout) => ToolResult::error("timeout", "Command timed out"),
        Err(CommandRunError::Io(error)) => ToolResult::error("execution_error", error),
        Ok(output) => {
            let mut text = String::new();
            text.push_str(&String::from_utf8_lossy(&output.stdout));
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            let (text, truncated) = truncate(text, context.config.tools.max_result_chars);
            let mut result = if output.status.success() {
                ToolResult::ok(text)
            } else {
                ToolResult::error("nonzero_exit", text)
            };
            result.truncated = truncated;
            result
        }
    }
}

fn platform_shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut process = Command::new("cmd");
        process.arg("/C").arg(command);
        process
    }
    #[cfg(not(windows))]
    {
        let mut process = Command::new("sh");
        process.arg("-c").arg(command);
        process.process_group(0);
        process
    }
}

enum CommandRunError {
    Timeout,
    Io(String),
}

fn run_command_with_timeout(
    mut command: Command,
    timeout_seconds: f64,
) -> Result<std::process::Output, CommandRunError> {
    let mut child = command
        .spawn()
        .map_err(|error| CommandRunError::Io(error.to_string()))?;
    let stdout_reader = child.stdout.take().map(|mut handle| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            handle.read_to_end(&mut bytes).map(|_| bytes)
        })
    });
    let stderr_reader = child.stderr.take().map(|mut handle| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            handle.read_to_end(&mut bytes).map(|_| bytes)
        })
    });
    let started = Instant::now();
    let timeout = Duration::from_secs_f64(timeout_seconds.max(0.0));
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() >= timeout => {
                kill_shell_process_tree(&mut child);
                let _ = child.wait();
                join_output_reader(stdout_reader)?;
                join_output_reader(stderr_reader)?;
                return Err(CommandRunError::Timeout);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(error) => return Err(CommandRunError::Io(error.to_string())),
        }
    }
    let status = child
        .wait()
        .map_err(|error| CommandRunError::Io(error.to_string()))?;
    let stdout = join_output_reader(stdout_reader)?;
    let stderr = join_output_reader(stderr_reader)?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(unix)]
fn kill_shell_process_tree(child: &mut std::process::Child) {
    let pgid = child.id() as i32;
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_shell_process_tree(child: &mut std::process::Child) {
    let _ = child.kill();
}

fn join_output_reader(
    reader: Option<std::thread::JoinHandle<std::io::Result<Vec<u8>>>>,
) -> Result<Vec<u8>, CommandRunError> {
    let Some(reader) = reader else {
        return Ok(Vec::new());
    };
    reader
        .join()
        .map_err(|_| CommandRunError::Io("command output reader panicked".to_string()))?
        .map_err(|error| CommandRunError::Io(error.to_string()))
}

fn memory_list(context: &ToolContext) -> ToolResult {
    let mut names = Vec::new();
    for name in ["SOUL.md", "USER.md", "MEMORY.md", "INDEX.md"] {
        if context.config.memory.root.join(name).is_file() {
            names.push(name.to_string());
        }
    }
    let topics = context.config.memory.root.join("topics");
    if let Ok(entries) = fs::read_dir(topics) {
        let mut topic_names = Vec::new();
        for entry in entries.flatten() {
            if entry.path().is_file()
                && entry.path().extension().and_then(|value| value.to_str()) == Some("md")
            {
                topic_names.push(format!("topics/{}", entry.file_name().to_string_lossy()));
            }
        }
        topic_names.sort();
        names.extend(topic_names);
    }
    let (text, truncated) = truncate(names.join("\n"), context.config.tools.max_result_chars);
    let mut result = ToolResult::ok(text);
    result.truncated = truncated;
    result
}

fn memory_read(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some((path, _)) = memory_target(args, context) else {
        return ToolResult::error("invalid_arguments", "Invalid memory file");
    };
    if !path.exists() {
        return ToolResult::error("not_found", "Memory file does not exist");
    }
    if !path.is_file() {
        return ToolResult::error("not_file", "Memory path is not a file");
    }
    match fs::read_to_string(path) {
        Ok(text) => {
            let (text, truncated) = truncate(text, context.config.tools.max_result_chars);
            let mut result = ToolResult::ok(text);
            result.truncated = truncated;
            result
        }
        Err(error) => ToolResult::error("io_error", error.to_string()),
    }
}

fn memory_search(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some(query) = args
        .get("query")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return ToolResult::error("invalid_arguments", "Missing query");
    };
    let query = query.to_lowercase();
    let index = context.config.memory.root.join("INDEX.md");
    let text = fs::read_to_string(index).unwrap_or_default();
    let mut matches = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.to_lowercase().contains(&query) {
            matches.push(format!("INDEX.md:{}: {}", index + 1, line));
        }
        if matches.len() >= context.config.memory.max_search_results {
            break;
        }
    }
    let (text, truncated) = truncate(matches.join("\n"), context.config.tools.max_result_chars);
    let mut result = ToolResult::ok(text);
    result.truncated = truncated;
    result
}

fn memory_write(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some((path, label)) = memory_target(args, context) else {
        return ToolResult::error("invalid_arguments", "Invalid memory file");
    };
    let Some(content) = args
        .get("content")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return ToolResult::error("invalid_arguments", "Missing content");
    };
    let mode = args.get("mode").map(String::as_str).unwrap_or("append");
    if !matches!(mode, "append" | "replace") {
        return ToolResult::error("invalid_arguments", "Invalid write mode");
    }
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            return ToolResult::error("io_error", error.to_string());
        }
    }
    let write_result = if mode == "replace" {
        fs::write(
            &path,
            format!(
                "{}{}",
                content,
                if content.ends_with('\n') { "" } else { "\n" }
            ),
        )
    } else {
        use std::io::Write;
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                file.write_all(content.as_bytes())?;
                if !content.ends_with('\n') {
                    file.write_all(b"\n")?;
                }
                Ok(())
            })
    };
    match write_result {
        Ok(()) => {
            let mut message = format!("Updated memory file: {}", label);
            if label.starts_with("topics/") {
                message.push_str(
                    "\nRemember to update INDEX.md so this topic can be found by memory.search.",
                );
            }
            let limit = match label.as_str() {
                "SOUL.md" => Some(1000usize),
                "USER.md" => Some(1000usize),
                "MEMORY.md" => Some(2000usize),
                _ => None,
            };
            if let Some(limit) = limit {
                let size = fs::read_to_string(&path)
                    .map(|text| text.chars().count())
                    .unwrap_or(0);
                if size > limit {
                    message.push_str(&format!(
                        "\n{} exceeds {} characters. Summarize or consolidate it, then call memory.write with file=\"{}\", mode=\"replace\" to keep it within the limit.",
                        label, limit, label
                    ));
                }
            }
            let (text, truncated) = truncate(message, context.config.tools.max_result_chars);
            let mut result = ToolResult::ok(text);
            result.truncated = truncated;
            result
        }
        Err(error) => ToolResult::error("io_error", error.to_string()),
    }
}

fn memory_target(
    args: &BTreeMap<String, String>,
    context: &ToolContext,
) -> Option<(PathBuf, String)> {
    if let Some(topic) = args.get("topic").map(|value| value.trim()) {
        if valid_topic_name(topic) {
            let label = format!("topics/{}.md", topic);
            return Some((context.config.memory.root.join(&label), label));
        }
    }
    let file = args.get("file")?.trim();
    if matches!(file, "SOUL.md" | "USER.md" | "MEMORY.md" | "INDEX.md") {
        return Some((context.config.memory.root.join(file), file.to_string()));
    }
    let topic = file.strip_prefix("topics/")?.strip_suffix(".md")?;
    valid_topic_name(topic).then(|| {
        let label = format!("topics/{}.md", topic);
        (context.config.memory.root.join(&label), label)
    })
}

fn valid_topic_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn web_search(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    if !matches!(
        context.config.web_search.engine.as_str(),
        "baidu" | "aliyun_mcp"
    ) {
        return ToolResult::error(
            "invalid_config",
            format!(
                "Unsupported web search engine: {}",
                context.config.web_search.engine
            ),
        );
    }
    let query = args.get("query").cloned().unwrap_or_default();
    if query.trim().is_empty() {
        return ToolResult::error(
            "invalid_arguments",
            "web.search requires a non-empty string query",
        );
    }
    let count = match args.get("count") {
        None => context.config.web_search.max_results,
        Some(value) => match value.parse::<isize>() {
            Ok(value) => value.clamp(1, 50) as usize,
            Err(_) => {
                return ToolResult::error(
                    "invalid_arguments",
                    "web.search count must be an integer",
                )
            }
        },
    };
    let search_filter = match web_search_filter(args.get("freshness").map(String::as_str)) {
        Ok(value) => value,
        Err(error) => return ToolResult::error("invalid_arguments", error),
    };
    let text = if context.config.web_search.engine == "baidu" {
        match baidu_web_search(&query, count, search_filter, context) {
            Ok(text) => text,
            Err((kind, text)) => return ToolResult::error(kind, text),
        }
    } else {
        match aliyun_mcp_web_search(
            query.trim(),
            count,
            args.get("freshness")
                .map(String::as_str)
                .filter(|value| !value.is_empty()),
            context,
        ) {
            Ok(text) => text,
            Err((kind, text)) => return ToolResult::error(kind, text),
        }
    };
    let (text, truncated) = truncate(text, context.config.tools.max_result_chars);
    let mut result = ToolResult::ok(text);
    result.truncated = truncated;
    result
}

fn baidu_web_search(
    query: &str,
    count: usize,
    search_filter: serde_json::Value,
    context: &ToolContext,
) -> Result<String, (&'static str, String)> {
    let body = serde_json::to_string(&serde_json::json!({
        "messages":[{"content":query.trim(),"role":"user"}],
        "search_source":"baidu_search_v2",
        "resource_type_filter":[{"type":"web","top_k":count}],
        "search_filter":search_filter
    }))
    .unwrap_or_else(|_| "{}".to_string());
    let (url, headers) = match baidu_url_and_headers(context) {
        Ok(value) => value,
        Err(error) => return Err(("invalid_config", error)),
    };
    let response = match request_json(
        "POST",
        &url,
        &headers
            .iter()
            .map(|(key, value)| (key.as_str(), value.clone()))
            .collect::<Vec<_>>(),
        Some(&body),
        context.config.web_search.timeout_seconds,
    ) {
        Ok(response) => response,
        Err(error) if error.contains("(28)") || error.to_lowercase().contains("timed out") => {
            return Err(("timeout", "web.search request timed out".to_string()))
        }
        Err(error) => {
            return Err((
                "network_error",
                format!("web.search request failed: {}", error),
            ))
        }
    };
    if let Some(status) = response.status {
        if status >= 400 {
            return Err((
                "http_error",
                format!("web.search failed with HTTP {}: {}", status, response.body),
            ));
        }
    }
    format_baidu_references(&response.body)
}

fn image_understand(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some(raw_path) = args.get("path").filter(|value| !value.is_empty()) else {
        return ToolResult::error("invalid_arguments", "Missing path");
    };
    let Some(analyzer) = &context.image_analyzer else {
        return ToolResult::error("vision_unavailable", "Image understanding is unavailable");
    };
    let path = resolve_arg_path(raw_path, context);
    if !is_allowed(&path, context) {
        return ToolResult::error("permission_denied", "Path is outside allowed roots");
    }
    if !path.exists() {
        return ToolResult::error("not_found", "Path does not exist");
    }
    if !path.is_file() {
        return ToolResult::error("not_file", "Path is not a file");
    }
    let content_type = content_type_for_path(&path);
    if !content_type.starts_with("image/") {
        return ToolResult::error("invalid_media", "Path is not an image");
    }
    let prompt = args.get("prompt").map(String::as_str).unwrap_or("");
    match analyzer(&path, prompt) {
        Ok(text) => {
            let (text, truncated) = truncate(text, context.config.tools.max_result_chars);
            let mut result = ToolResult::ok(text);
            result.truncated = truncated;
            result
        }
        Err(error) if error.starts_with("image_too_large:") => ToolResult::error(
            "image_too_large",
            error.trim_start_matches("image_too_large:"),
        ),
        Err(error) => ToolResult::error("model_error", error),
    }
}

fn web_search_filter(freshness: Option<&str>) -> Result<serde_json::Value, String> {
    match freshness {
        None | Some("") => Ok(serde_json::json!({})),
        Some(value) => {
            let today = local_date();
            let end = add_date_days(&today, 1).unwrap_or_else(|| today.clone());
            let start = match value {
                "pd" => add_date_days(&today, -1),
                "pw" => add_date_days(&today, -6),
                "pm" => add_date_days(&today, -30),
                "py" => add_date_days(&today, -364),
                _ if valid_date_range(value) => {
                    let (start, end) = value.split_once("to").unwrap();
                    return Ok(serde_json::json!({"range":{"page_time":{"gte":start,"lt":end}}}));
                }
                _ => None,
            }
            .ok_or_else(|| {
                "web.search freshness must be pd, pw, pm, py, or YYYY-MM-DDtoYYYY-MM-DD".to_string()
            })?;
            Ok(serde_json::json!({"range":{"page_time":{"gte":start,"lt":end}}}))
        }
    }
}

fn format_baidu_references(body: &str) -> Result<String, (&'static str, String)> {
    let data: serde_json::Value = serde_json::from_str(body).map_err(|_| {
        (
            "invalid_response",
            "web.search response was not valid JSON".to_string(),
        )
    })?;
    if data.get("code").is_some() {
        let message = data
            .get("message")
            .or_else(|| data.get("msg"))
            .and_then(|value| value.as_str())
            .unwrap_or("Baidu web search API error");
        return Err(("api_error", message.to_string()));
    }
    let references = data
        .get("references")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            (
                "invalid_response",
                "Baidu web search response missing references".to_string(),
            )
        })?;
    let cleaned = references
        .iter()
        .map(|item| {
            let mut item = item.clone();
            if let Some(object) = item.as_object_mut() {
                object.remove("snippet");
            }
            item
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&cleaned).map_err(|error| ("invalid_response", error.to_string()))
}

fn baidu_url_and_headers(context: &ToolContext) -> Result<(String, Vec<(String, String)>), String> {
    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    if let (Ok(session_id), Ok(scheduler_url)) = (
        std::env::var("DUMATE_SESSION_ID"),
        std::env::var("DUMATE_SCHEDULER_URL"),
    ) {
        let endpoint = &context.config.web_search.endpoint;
        let without_scheme = endpoint
            .split_once("://")
            .map(|(_, value)| value)
            .unwrap_or(endpoint);
        let (host, path) = without_scheme
            .split_once('/')
            .unwrap_or((without_scheme, ""));
        headers.push(("Host".to_string(), host.to_string()));
        headers.push(("X-Dumate-Session-Id".to_string(), session_id));
        headers.push(("X-Appbuilder-From".to_string(), "desktop".to_string()));
        return Ok((
            format!(
                "{}/api/qianfanproxy/{}",
                scheduler_url.trim_end_matches('/'),
                path
            ),
            headers,
        ));
    }
    if context.config.web_search.api_key.is_empty() {
        return Err("Missing Baidu web search API key: set web_search.api_key".to_string());
    }
    headers.push((
        "Authorization".to_string(),
        format!("Bearer {}", context.config.web_search.api_key),
    ));
    headers.push(("X-Appbuilder-From".to_string(), "openclaw".to_string()));
    Ok((context.config.web_search.endpoint.clone(), headers))
}

fn aliyun_mcp_web_search(
    query: &str,
    count: usize,
    freshness: Option<&str>,
    context: &ToolContext,
) -> Result<String, (&'static str, String)> {
    let endpoint = context.config.web_search.endpoint.trim();
    if endpoint.is_empty() || endpoint == BAIDU_DEFAULT_SEARCH_ENDPOINT {
        return Err((
            "invalid_config",
            "Missing Aliyun WebSearch MCP endpoint: set web_search.endpoint".to_string(),
        ));
    }
    let api_key = if context.config.web_search.api_key.is_empty() {
        std::env::var("DASHSCOPE_API_KEY").unwrap_or_default()
    } else {
        context.config.web_search.api_key.clone()
    };
    if api_key.is_empty() {
        return Err((
            "invalid_config",
            "Missing Aliyun WebSearch MCP API key: set web_search.api_key or DASHSCOPE_API_KEY"
                .to_string(),
        ));
    }

    let mut session_id = None;
    let result = (|| {
        let initialize = serde_json::json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{
                "protocolVersion":MCP_PROTOCOL_VERSION,
                "capabilities":{},
                "clientInfo":{"name":"colibri","version":"0.1.0"}
            }
        });
        let (messages, initialized_session) =
            mcp_post(endpoint, &api_key, &initialize, None, false, context)?;
        session_id = initialized_session;
        mcp_result(&messages, 1)?;

        mcp_post(
            endpoint,
            &api_key,
            &serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            session_id.as_deref(),
            true,
            context,
        )?;
        let (messages, _) = mcp_post(
            endpoint,
            &api_key,
            &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
            session_id.as_deref(),
            false,
            context,
        )?;
        let listed = mcp_result(&messages, 2)?;
        let tool = select_mcp_search_tool(&listed)?;
        let tool_name = tool
            .get("name")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                (
                    "invalid_response",
                    "web.search MCP tool was missing name".to_string(),
                )
            })?;
        let properties = tool
            .get("inputSchema")
            .and_then(|value| value.get("properties"))
            .and_then(|value| value.as_object());
        let mut arguments = serde_json::Map::new();
        arguments.insert(
            "query".to_string(),
            serde_json::Value::String(query.to_string()),
        );
        if properties.is_some_and(|properties| properties.contains_key("count")) {
            arguments.insert(
                "count".to_string(),
                serde_json::Value::Number(serde_json::Number::from(count.min(20))),
            );
        }
        if let Some(freshness) = freshness {
            if properties.is_some_and(|properties| properties.contains_key("freshness")) {
                arguments.insert(
                    "freshness".to_string(),
                    serde_json::Value::String(freshness.to_string()),
                );
            }
        }
        let (messages, _) = mcp_post(
            endpoint,
            &api_key,
            &serde_json::json!({
                "jsonrpc":"2.0",
                "id":3,
                "method":"tools/call",
                "params":{"name":tool_name,"arguments":arguments}
            }),
            session_id.as_deref(),
            false,
            context,
        )?;
        mcp_tool_text(&mcp_result(&messages, 3)?)
    })();

    if let Some(session_id) = session_id {
        mcp_delete(endpoint, &api_key, &session_id, context);
    }
    result
}

fn mcp_post(
    endpoint: &str,
    api_key: &str,
    payload: &serde_json::Value,
    session_id: Option<&str>,
    allow_empty: bool,
    context: &ToolContext,
) -> Result<(Vec<serde_json::Value>, Option<String>), (&'static str, String)> {
    let mut headers = vec![
        ("Authorization".to_string(), format!("Bearer {}", api_key)),
        (
            "Accept".to_string(),
            "application/json, text/event-stream".to_string(),
        ),
        (
            "MCP-Protocol-Version".to_string(),
            MCP_PROTOCOL_VERSION.to_string(),
        ),
    ];
    if let Some(session_id) = session_id {
        headers.push(("Mcp-Session-Id".to_string(), session_id.to_string()));
    }
    let body = serde_json::to_string(payload).map_err(|error| {
        (
            "invalid_arguments",
            format!("web.search MCP request was not valid JSON: {}", error),
        )
    })?;
    let response = request_json(
        "POST",
        endpoint,
        &headers
            .iter()
            .map(|(key, value)| (key.as_str(), value.clone()))
            .collect::<Vec<_>>(),
        Some(&body),
        context.config.web_search.timeout_seconds,
    )
    .map_err(mcp_transport_error)?;
    if response.status.is_some_and(|status| status >= 400) {
        return Err((
            "http_error",
            format!(
                "web.search MCP failed with HTTP {}: {}",
                response.status.unwrap_or_default(),
                response.body
            ),
        ));
    }
    let response_session = response
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("Mcp-Session-Id"))
        .map(|(_, value)| value.clone())
        .or_else(|| session_id.map(ToString::to_string));
    if response.body.trim().is_empty() {
        if allow_empty {
            return Ok((Vec::new(), response_session));
        }
        return Err((
            "invalid_response",
            "web.search MCP response was empty".to_string(),
        ));
    }
    let content_type = response
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("Content-Type"))
        .map(|(_, value)| value.as_str())
        .unwrap_or("");
    Ok((
        mcp_messages(&response.body, content_type)?,
        response_session,
    ))
}

fn mcp_delete(endpoint: &str, api_key: &str, session_id: &str, context: &ToolContext) {
    let headers = [
        ("Authorization", format!("Bearer {}", api_key)),
        ("Accept", "application/json, text/event-stream".to_string()),
        ("MCP-Protocol-Version", MCP_PROTOCOL_VERSION.to_string()),
        ("Mcp-Session-Id", session_id.to_string()),
    ];
    let _ = request_json(
        "DELETE",
        endpoint,
        &headers,
        None,
        context.config.web_search.timeout_seconds,
    );
}

fn mcp_transport_error(error: String) -> (&'static str, String) {
    if error.contains("(28)") || error.to_ascii_lowercase().contains("timed out") {
        ("timeout", "web.search MCP request timed out".to_string())
    } else {
        (
            "network_error",
            format!("web.search MCP request failed: {}", error),
        )
    }
}

fn mcp_messages(
    body: &str,
    content_type: &str,
) -> Result<Vec<serde_json::Value>, (&'static str, String)> {
    let is_sse = content_type
        .to_ascii_lowercase()
        .contains("text/event-stream")
        || body.trim_start().starts_with("data:")
        || body.trim_start().starts_with("event:");
    if !is_sse {
        let parsed = serde_json::from_str::<serde_json::Value>(body).map_err(|_| {
            (
                "invalid_response",
                "web.search MCP response was not valid JSON or SSE".to_string(),
            )
        })?;
        return match parsed {
            serde_json::Value::Array(values) if values.iter().all(serde_json::Value::is_object) => {
                Ok(values)
            }
            value if value.is_object() => Ok(vec![value]),
            _ => Err((
                "invalid_response",
                "web.search MCP response was not valid JSON or SSE".to_string(),
            )),
        };
    }

    let normalized = body.replace("\r\n", "\n");
    let mut messages = Vec::new();
    for event in normalized.split("\n\n") {
        let data = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            continue;
        }
        let value = serde_json::from_str::<serde_json::Value>(&data).map_err(|_| {
            (
                "invalid_response",
                "web.search MCP response was not valid JSON or SSE".to_string(),
            )
        })?;
        if value.is_object() {
            messages.push(value);
        }
    }
    if messages.is_empty() {
        return Err((
            "invalid_response",
            "web.search MCP response contained no JSON-RPC message".to_string(),
        ));
    }
    Ok(messages)
}

fn mcp_result(
    messages: &[serde_json::Value],
    request_id: i64,
) -> Result<serde_json::Value, (&'static str, String)> {
    let response = messages
        .iter()
        .find(|message| message.get("id").and_then(|value| value.as_i64()) == Some(request_id))
        .ok_or_else(|| {
            (
                "invalid_response",
                "web.search MCP response was missing the requested result".to_string(),
            )
        })?;
    if let Some(error) = response.get("error") {
        let message = error
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("MCP request failed");
        return Err(("api_error", message.to_string()));
    }
    response.get("result").cloned().ok_or_else(|| {
        (
            "invalid_response",
            "web.search MCP response was missing result".to_string(),
        )
    })
}

fn select_mcp_search_tool(
    result: &serde_json::Value,
) -> Result<serde_json::Value, (&'static str, String)> {
    let tools = result
        .get("tools")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            (
                "invalid_response",
                "web.search MCP tools/list response was invalid".to_string(),
            )
        })?;
    let candidates = tools
        .iter()
        .filter(|tool| tool.get("name").and_then(|value| value.as_str()).is_some())
        .collect::<Vec<_>>();
    if let Some(tool) = candidates.iter().find(|tool| {
        tool.get("name").and_then(|value| value.as_str()) == Some(ALIYUN_MCP_TOOL_NAME)
    }) {
        return Ok((*tool).clone());
    }
    if candidates.len() == 1 {
        return Ok(candidates[0].clone());
    }
    Err((
        "invalid_response",
        "web.search MCP did not advertise a unique search tool".to_string(),
    ))
}

fn mcp_tool_text(result: &serde_json::Value) -> Result<String, (&'static str, String)> {
    if result.get("isError").and_then(|value| value.as_bool()) == Some(true) {
        return Err((
            "api_error",
            "Aliyun WebSearch MCP tool returned an error".to_string(),
        ));
    }
    if let Some(content) = result.get("content").and_then(|value| value.as_array()) {
        let text = content
            .iter()
            .filter(|block| block.get("type").and_then(|value| value.as_str()) == Some("text"))
            .filter_map(|block| block.get("text").and_then(|value| value.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        if !text.trim().is_empty() {
            return Ok(text);
        }
    }
    if let Some(structured) = result.get("structuredContent") {
        return serde_json::to_string_pretty(structured)
            .map_err(|error| ("invalid_response", error.to_string()));
    }
    Err((
        "invalid_response",
        "Aliyun WebSearch MCP result contained no usable content".to_string(),
    ))
}

fn valid_date_range(value: &str) -> bool {
    let Some((start, end)) = value.split_once("to") else {
        return false;
    };
    valid_date(start) && valid_date(end) && value.matches("to").count() == 1
}

fn valid_date(value: &str) -> bool {
    value.len() == 10
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value
            .chars()
            .enumerate()
            .all(|(index, ch)| matches!(index, 4 | 7) || ch.is_ascii_digit())
}

fn local_date() -> String {
    Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| valid_date(value))
        .unwrap_or_else(|| "1970-01-01".to_string())
}

fn add_date_days(value: &str, offset: i64) -> Option<String> {
    let year = value.get(0..4)?.parse::<i64>().ok()?;
    let month = value.get(5..7)?.parse::<i64>().ok()?;
    let day = value.get(8..10)?.parse::<i64>().ok()?;
    let days = days_from_civil(year, month, day) + offset;
    let (year, month, day) = civil_from_days(days);
    Some(format!("{:04}-{:02}-{:02}", year, month, day))
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * shifted_month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719468;
    let era = days.div_euclid(146097);
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn resolve_arg_path(value: &str, context: &ToolContext) -> PathBuf {
    let path = expand_user_path(value);
    let joined = if path.is_absolute() {
        path
    } else {
        context.cwd.join(path)
    };
    canonicalize_existing_prefix(&joined)
}

pub fn is_allowed(path: &Path, context: &ToolContext) -> bool {
    let canonical = canonicalize_existing_prefix(path);
    let mut roots = context.config.files.roots.clone();
    roots.push(context.cwd.clone());
    roots.extend(context.allowed_file_roots.clone());
    roots.into_iter().any(|root| {
        let root = canonicalize_existing_prefix(&root);
        canonical.starts_with(root)
    })
}

pub fn content_type_for_path(path: &Path) -> String {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "txt" | "md" | "csv" | "log" => "text/plain".to_string(),
        "html" | "htm" => "text/html".to_string(),
        "json" => "application/json".to_string(),
        "png" => "image/png".to_string(),
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "gif" => "image/gif".to_string(),
        "webp" => "image/webp".to_string(),
        "mp4" => "video/mp4".to_string(),
        "mov" => "video/quicktime".to_string(),
        "mp3" => "audio/mpeg".to_string(),
        "wav" => "audio/wav".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

fn media_type_for_content(content_type: &str) -> String {
    if content_type.starts_with("image/") {
        "image".to_string()
    } else if content_type.starts_with("video/") {
        "video".to_string()
    } else if content_type.starts_with("audio/") {
        "audio".to_string()
    } else {
        "file".to_string()
    }
}

fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    let mut missing = Vec::new();
    let mut current = path;
    while !current.exists() {
        if let Some(name) = current.file_name() {
            missing.push(name.to_os_string());
        }
        let Some(parent) = current.parent() else {
            return path.to_path_buf();
        };
        current = parent;
    }
    let mut rebuilt = current
        .canonicalize()
        .unwrap_or_else(|_| current.to_path_buf());
    for item in missing.iter().rev() {
        rebuilt.push(item);
    }
    rebuilt
}

pub fn parse_json_object(text: &str) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("expected JSON object: {}", error))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "expected JSON object".to_string())
}

pub fn string_arguments(
    values: &serde_json::Map<String, serde_json::Value>,
) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| value.to_string()),
            )
        })
        .collect()
}
