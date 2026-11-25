//! Cardputer protocol adapter for the shared Rust runtime.
mod task_view;
use crate::mcp;
mod pinyin;
mod settings;
mod voice;

use crate::config::{colibri_home, expand_user_path, AgentConfig};
use crate::messages::Message;
use crate::runtime_reload::{config_fingerprint, ConfigFingerprint};
use crate::session::AgentSession;
use crate::session_history::TranscriptHistoryLoader;
use crate::skills::SkillIndex;
use crate::transcript::{beijing_timestamp_now, TranscriptWriter};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use self::settings::{device_model, Settings};
use self::voice::Recording;
use crate::config_io::{atomic_write, lock};

const MAX_MEMORY_BYTES: usize = 64 * 1024;
type Notice = Arc<dyn Fn(&str) + Send + Sync>;

struct App {
    settings: Settings,
    session: Option<AgentSession>,
    recording: Option<Recording>,
    notice: Notice,
    activity: Option<Notice>,
    restore: bool,
    settings_changed: bool,
    loaded_config: Option<ConfigFingerprint>,
    transcript_path: Option<PathBuf>,
}

impl App {
    fn new(home: PathBuf, notice: Notice) -> Self {
        Self {
            settings: Settings { home },
            session: None,
            recording: None,
            notice,
            activity: None,
            restore: true,
            settings_changed: false,
            loaded_config: None,
            transcript_path: None,
        }
    }

    fn model_hint(&self) -> String {
        self.settings
            .config()
            .and_then(|config| device_model(&config))
            .err()
            .unwrap_or_default()
    }

    fn history_loader(&self, config: &AgentConfig) -> TranscriptHistoryLoader {
        let session = &config.session;
        TranscriptHistoryLoader::new(
            self.settings.home.clone(),
            session.restore_message_limit,
            session.restore_char_limit,
            session.restore_scan_bytes,
        )
    }

    fn startup_history(&self) -> Result<Vec<Message>, String> {
        let config = self.settings.config()?;
        if !(config.session.transcript && config.session.restore_transcript) {
            return Ok(Vec::new());
        }
        let offsets = match fs::read(self.settings.home.join("chat-display-cursor.json")) {
            Ok(data) => serde_json::from_slice(&data).map_err(|error| error.to_string())?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.to_string()),
        };
        Ok(self.history_loader(&config).load_after(&offsets))
    }

    fn clear_chat(&self) -> Result<(), String> {
        let mut offsets = BTreeMap::new();
        match fs::read_dir(self.settings.home.join("transcripts")) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|error| error.to_string())?;
                    if entry.path().extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                        let metadata = entry.metadata().map_err(|error| error.to_string())?;
                        if metadata.is_file() {
                            offsets.insert(
                                entry.file_name().to_string_lossy().into_owned(),
                                metadata.len(),
                            );
                        }
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        let path = self.settings.home.join("chat-display-cursor.json");
        let data = serde_json::to_vec(&offsets).map_err(|error| error.to_string())?;
        fs::create_dir_all(&self.settings.home).map_err(|error| error.to_string())?;
        atomic_write(&path, &data).map_err(|error| error.to_string())
    }

    fn open_transcript(
        &mut self,
        config: &AgentConfig,
    ) -> Result<Option<Arc<Mutex<TranscriptWriter>>>, String> {
        if !config.session.transcript {
            self.transcript_path = None;
            return Ok(None);
        }
        let date = beijing_timestamp_now();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = self
            .settings
            .home
            .join("transcripts")
            .join(format!("{}-{stamp}.jsonl", &date[..10]));
        let writer = TranscriptWriter::new(
            path.clone(),
            Default::default(),
            config.session.transcript_retention_days,
            config.session.transcript_max_total_bytes,
        )?;
        self.transcript_path = Some(path);
        Ok(Some(Arc::new(Mutex::new(writer))))
    }

    fn ask(&mut self, prompt: &str) -> Result<String, String> {
        let fingerprint = config_fingerprint(&self.settings.home.join("config.toml"));
        if self.session.is_none() || self.settings_changed || fingerprint != self.loaded_config {
            let config = self.settings.config()?;
            crate::memory::initialize(&config)?;
            let model = device_model(&config)?;
            let transcript = self.open_transcript(&config)?;
            let mut session = AgentSession::from_shared(
                Arc::new(config),
                Arc::new(Mutex::new(model)),
                transcript,
                Default::default(),
            )
            .with_working_directory(self.settings.home.clone())
            .with_runtime_home(self.settings.home.clone())
            .with_status_callback(false, Arc::clone(&self.notice));
            if let Some(activity) = &self.activity {
                session = session.with_activity_callback(Arc::clone(activity));
            }
            if let Some(old) = self.session.take() {
                session.inherit_history(old);
            } else if self.restore
                && session.config.session.transcript
                && session.config.session.restore_transcript
            {
                let loader = self.history_loader(&session.config);
                session =
                    session.with_model_history_loader(Box::new(move || loader.load_model("local")));
            }
            self.restore = false;
            self.settings_changed = false;
            self.loaded_config = fingerprint;
            self.session = Some(session);
        }
        let response = self.session.as_mut().unwrap().submit(prompt)?;
        let config = Arc::clone(&self.session.as_ref().unwrap().config);
        if self.transcript_path.as_ref().is_some_and(|path| {
            let cap = config.session.transcript_max_total_bytes;
            let today = beijing_timestamp_now();
            !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(&today[..10]))
                || (cap != 0
                    && fs::metadata(path).map_or(true, |metadata| {
                        metadata.len() >= cap.min(1024 * 1024) as u64
                    }))
        }) {
            let writer = self.open_transcript(&config)?;
            self.session.as_mut().unwrap().replace_transcript(writer);
        }
        Ok(response.text)
    }

    fn reload(&mut self) -> Result<(), String> {
        if self.session.is_some() {
            self.settings_changed = true;
            device_model(&self.settings.config()?)?;
        }
        Ok(())
    }

    fn memory_root(&self) -> Result<PathBuf, String> {
        let config = self.settings.config()?;
        crate::memory::initialize(&config)?;
        Ok(config.memory.root)
    }

    fn mcp(
        &self,
        name: &str,
        operation: &str,
        target: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        let values = self.settings.values()?;
        let server = mcp::servers(&values["mcp.servers"])?
            .into_iter()
            .find(|server| server.name == name)
            .ok_or("未找到 MCP 服务器")?;
        mcp::operation(&server, operation, target, arguments)
    }

    fn catalog(&self, kind: &str) -> Result<Vec<Value>, String> {
        match kind {
            "skills" => Ok(SkillIndex::scan(&self.settings.config()?.skills.dir)
                .skills
                .iter()
                .map(|skill| json!({"name":skill.name,"description":skill.description}))
                .collect()),
            "mcp" => Ok(mcp::servers(&self.settings.values()?["mcp.servers"])?
                .into_iter()
                .map(|server| {
                    let description = if !server.description.is_empty() {
                        server.description
                    } else if server.command.is_some() {
                        "stdio · 未填写说明".into()
                    } else {
                        "Streamable HTTP · 未填写说明".into()
                    };
                    json!({"name":server.name,"description":description})
                })
                .collect()),
            _ => Err("目录类型无效".into()),
        }
    }

    fn start_voice(&mut self) -> Result<&'static str, String> {
        if self.recording.is_none() {
            self.recording = Some(Recording::start()?);
        }
        Ok(self.recording.as_ref().unwrap().backend())
    }

    fn finish_voice(&mut self, cancel: bool) -> Result<String, String> {
        self.recording
            .take()
            .map_or(Ok(String::new()), |recording| recording.finish(cancel))
    }

    fn new_conversation(&mut self) {
        self.session = None;
        self.restore = false;
        self.recording.take();
    }
}

fn topic(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub(crate) fn memory_path(root: &Path, name: &str) -> Result<PathBuf, String> {
    let is_topic = name
        .strip_prefix("topics/")
        .and_then(|name| name.strip_suffix(".md"))
        .is_some_and(topic);
    if !matches!(name, "SOUL.md" | "USER.md" | "MEMORY.md" | "INDEX.md") && !is_topic {
        return Err("记忆文件名无效".into());
    }
    let path = root.join(name);
    let symlink =
        |path: &Path| fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_symlink());
    if symlink(&path) || (is_topic && symlink(&root.join("topics"))) {
        return Err("记忆编辑器无法打开符号链接".into());
    }
    Ok(path)
}

fn memory_topics(root: &Path) -> Vec<String> {
    let folder = root.join("topics");
    if fs::symlink_metadata(&folder).is_ok_and(|metadata| metadata.is_symlink()) {
        return Vec::new();
    }
    let mut names: Vec<_> = fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            (entry.file_type().ok()?.is_file() && name.strip_suffix(".md").is_some_and(topic))
                .then(|| format!("topics/{name}"))
        })
        .collect();
    names.sort();
    names
}

fn read_memory(root: &Path, name: &str) -> Result<Value, String> {
    let path = memory_path(root, name)?;
    let mut bytes = Vec::new();
    match File::open(path) {
        Ok(file) => file
            .take((MAX_MEMORY_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(json!({"file":name,"text":"","revision":null}))
        }
        Err(error) => return Err(error.to_string()),
    };
    if bytes.len() > MAX_MEMORY_BYTES {
        return Err("文件超过 64 KiB，请在设备终端编辑".into());
    }
    let content = std::str::from_utf8(&bytes).map_err(|_| "记忆文件不是 UTF-8 文本")?;
    Ok(json!({"file":name,"text":content,"revision":format!("{:x}",md5::compute(bytes))}))
}

fn save_memory(root: &Path, name: &str, content: &str, revision: &Value) -> Result<Value, String> {
    if content.len() > MAX_MEMORY_BYTES {
        return Err("文件超过 64 KiB".into());
    }
    let path = memory_path(root, name)?;
    let parent = path.parent().ok_or("记忆目录无效")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let _lock = lock(&root.join(".memory-editor.lock")).map_err(|e| e.to_string())?;
    if read_memory(root, name)?["revision"] != *revision {
        return Err("文件已在别处修改，请重新打开后再保存".into());
    }
    atomic_write(&path, content.as_bytes()).map_err(|e| e.to_string())?;
    read_memory(root, name)
}

fn required<'a>(request: &'a Value, field: &str) -> Result<&'a str, String> {
    request[field]
        .as_str()
        .ok_or_else(|| format!("缺少参数 {field}"))
}

fn event(name: &str, mut fields: Value) -> Value {
    fields["type"] = json!(name);
    fields
}

fn send(value: &Value) -> io::Result<()> {
    let mut output = io::stdout().lock();
    serde_json::to_writer(&mut output, value)?;
    output.write_all(b"\n")?;
    output.flush()
}

fn handle(app: &mut App, request: Value) -> Result<Option<Value>, String> {
    let result = match required(&request, "type")? {
        "task_status" => {
            let context =
                crate::tools::ToolContext::new(app.settings.config()?, app.settings.home.clone())
                    .with_runtime_home(app.settings.home.clone());
            event("task_status", json!({"text":task_view::render(&context)}))
        }
        "model_status" => {
            let history = app
                .startup_history()
                .unwrap_or_default()
                .into_iter()
                .map(|message| json!({"role":message.role,"text":message.content}))
                .collect::<Vec<_>>();
            event(
                "model_status",
                json!({"text":app.model_hint(),"history":history}),
            )
        }
        "clear_chat" => {
            app.clear_chat()?;
            event("chat_cleared", json!({}))
        }
        "settings" => event(
            "settings",
            json!({"fields":app.settings.public()?,"model_hint":app.model_hint()}),
        ),
        "save" => {
            app.settings
                .save(required(&request, "key")?, required(&request, "value")?)?;
            app.reload()?;
            event("saved", json!({}))
        }
        "save_model" => {
            app.settings.save_model(
                required(&request, "base_url")?,
                required(&request, "model")?,
                request["api_key"].as_str(),
            )?;
            app.reload()?;
            event("model_saved", json!({"fields":app.settings.public()?}))
        }
        "catalog" => event(
            "catalog_items",
            json!({"items":app.catalog(required(&request,"kind")?)?}),
        ),
        "memory_topics" => event(
            "memory_topics",
            json!({"files":memory_topics(&app.memory_root()?)}),
        ),
        "memory_read" => event(
            "memory_file",
            read_memory(&app.memory_root()?, required(&request, "file")?)?,
        ),
        "memory_save" => event(
            "memory_saved",
            save_memory(
                &app.memory_root()?,
                required(&request, "file")?,
                required(&request, "text")?,
                &request["revision"],
            )?,
        ),
        "submit" => event(
            "reply",
            json!({"text":app.ask(required(&request,"text")?)?}),
        ),
        "pinyin" => {
            let spelling = required(&request, "spelling")?;
            event(
                "pinyin",
                json!({"spelling":spelling,"text":pinyin::single_characters(spelling),
                       "candidates":pinyin::candidates(spelling)}),
            )
        }
        "voice_start" => event("recording", json!({"backend":app.start_voice()?})),
        "voice_stop" => {
            let text = app.finish_voice(false)?;
            if text.is_empty() {
                return Err("未识别到语音，请靠近麦克风重试。".into());
            }
            event("draft", json!({"text":text}))
        }
        "voice_cancel" => {
            app.finish_voice(true)?;
            event("recording_cancelled", json!({}))
        }
        "new" => {
            app.new_conversation();
            event("reply", json!({"text":"新会话"}))
        }
        "quit" => return Ok(None),
        _ => return Err("未知操作".into()),
    };
    Ok(Some(result))
}

fn pipe(home: PathBuf) -> Result<(), String> {
    let mut app = App::new(
        home,
        Arc::new(|message| {
            let _ = send(&event("status", json!({"text":message})));
        }),
    );
    if let Err(error) = app
        .settings
        .config()
        .and_then(|config| crate::memory::initialize(&config))
    {
        send(&event("error", json!({"text":error}))).map_err(|e| e.to_string())?;
    }
    app.activity = Some(Arc::new(|stage| {
        let text = match stage {
            "thinking" => "思考中",
            "compacting" => "压缩上下文",
            "running_command" => "执行命令",
            "waiting_command" => "等待命令",
            "planning_task" => "整理任务",
            "working_files" => "处理文件",
            "calling_tool" => "调用工具",
            _ => stage,
        };
        let _ = send(&event("activity", json!({"text":text})));
    }));
    for line in io::stdin().lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        let result = serde_json::from_str(&line)
            .map_err(|_| "请求格式错误".into())
            .and_then(|request| handle(&mut app, request));
        let value = match result {
            Ok(Some(value)) => value,
            Ok(None) => break,
            Err(error) => event("error", json!({"text":error})),
        };
        if let Err(error) = send(&value) {
            if error.kind() == io::ErrorKind::BrokenPipe {
                break;
            }
            return Err(error.to_string());
        }
    }
    Ok(())
}

fn repl(home: PathBuf) -> Result<(), String> {
    let mut app = App::new(home, Arc::new(|message| println!("{message}")));
    crate::memory::initialize(&app.settings.config()?)?;
    let terminal = io::stdin().is_terminal();
    println!(
        "Colibri REPL  /compact  /new  /voice  /set KEY VALUE  /mcp SERVER OP [NAME] [JSON]  /quit"
    );
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut line = String::new();
    loop {
        print!("› ");
        io::stdout().flush().map_err(|e| e.to_string())?;
        line.clear();
        if input.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            break;
        }
        let command = line.trim();
        if command == "/quit" {
            break;
        }
        let result = (|| -> Result<(), String> {
            if command == "/new" {
                app.new_conversation();
            } else if command == "/voice" {
                app.start_voice()?;
                println!("录音中，最多 60 秒。Enter 结束，输入 /cancel 取消");
                let mut choice = String::new();
                let eof = input.read_line(&mut choice).map_err(|e| e.to_string())? == 0;
                let draft = app.finish_voice(eof || choice.trim() == "/cancel")?;
                if !draft.is_empty() && terminal {
                    println!("语音草稿：{draft}\nEnter 发送，输入新文字替换，/cancel 取消。");
                    choice.clear();
                    let eof = input.read_line(&mut choice).map_err(|e| e.to_string())? == 0;
                    if !eof && choice.trim() != "/cancel" {
                        let value = if choice.trim().is_empty() {
                            &draft
                        } else {
                            choice.trim()
                        };
                        println!("{}", app.ask(value)?);
                    }
                } else if !draft.is_empty() {
                    println!("{draft}");
                }
            } else if let Some(args) = command.strip_prefix("/set ") {
                let (key, value) = args.split_once(' ').ok_or("用法：/set KEY VALUE")?;
                app.settings.save(key, value)?;
                app.reload()?;
            } else if let Some(args) = command.strip_prefix("/mcp ") {
                let mut parts = args.trim().splitn(4, char::is_whitespace);
                let server = parts.next().unwrap_or_default();
                let operation = parts.next().ok_or("用法：/mcp SERVER OP [NAME] [JSON]")?;
                let name = parts.next().unwrap_or_default();
                let arguments = parts
                    .next()
                    .map(serde_json::from_str)
                    .transpose()
                    .map_err(|_| "MCP 参数必须是 JSON")?
                    .unwrap_or(json!({}));
                println!("{}", app.mcp(server, operation, name, arguments)?);
            } else if !command.is_empty() {
                println!("{}", app.ask(command)?);
            }
            Ok(())
        })();
        if let Err(error) = result {
            println!("{error}");
        }
    }
    Ok(())
}

// Hardware presentation is selected by this adapter, never by the shared harness.
fn configure_qr_presenter() {
    #[cfg(target_os = "linux")]
    {
        use std::io::IsTerminal;
        if !std::io::stdout().is_terminal() || env::var_os("COLIBRI_QR_VIEWER").is_some() {
            return;
        }
        let mut pid = unsafe { libc::getppid() };
        for _ in 0..16 {
            if pid <= 1 {
                break;
            }
            if std::fs::read_link(format!("/proc/{pid}/exe"))
                .ok()
                .as_deref()
                == Some(std::path::Path::new(
                    "/usr/share/APPLaunch/bin/M5CardputerZero-APPLaunch",
                ))
            {
                env::set_var("COLIBRI_QR_VIEWER", "/usr/bin/colibri --weixin-qr-view");
                break;
            }
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            pid = stat
                .rsplit_once(')')
                .and_then(|(_, v)| v.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
        }
    }
}

pub fn run() -> Result<(), String> {
    configure_qr_presenter();
    let mut args = env::args().skip(1).peekable();
    if args.peek().is_some_and(|a| a == "__job-run") {
        std::process::exit(crate::cli::run(args.collect()));
    }
    let command = args
        .peek()
        .is_some_and(|arg| !matches!(arg.as_str(), "--repl" | "--home"));
    unsafe {
        libc::umask(0o077);
    }
    if command {
        let command_index = if args.peek().is_some_and(|arg| arg == "--config") {
            2
        } else {
            0
        };
        if env::args()
            .skip(1)
            .nth(command_index)
            .is_some_and(|arg| matches!(arg.as_str(), "version" | "--version" | "help" | "--help"))
        {
            std::process::exit(crate::cli::run(args.collect()));
        }
        let home = colibri_home();
        let legacy = expand_user_path("~/.local/share/colibri-cardputer");
        settings::migrate(&legacy, &home).map_err(|e| e.to_string())?;
        Settings { home }.initialize()?;
        std::process::exit(crate::cli::run(args.collect()));
    }
    let mut explicit = env::var_os("COLIBRI_CARDPUTER_HOME").map(PathBuf::from);
    let mut terminal = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--repl" => terminal = true,
            "--home" => explicit = Some(PathBuf::from(args.next().ok_or("--home 缺少目录")?)),
            _ => return Err("未知命令行参数".into()),
        }
    }
    let home = explicit.clone().map_or_else(colibri_home, |path| {
        expand_user_path(&path.to_string_lossy())
    });
    if explicit.is_none() {
        let legacy = expand_user_path("~/.local/share/colibri-cardputer");
        settings::migrate(&legacy, &home).map_err(|e| e.to_string())?;
    }
    Settings { home: home.clone() }.initialize()?;
    if terminal {
        repl(home)
    } else {
        pipe(home)
    }
}
