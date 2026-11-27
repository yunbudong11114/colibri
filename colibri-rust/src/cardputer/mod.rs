//! Cardputer protocol adapter for the shared Rust runtime.
mod protocol;
mod task_view;
use crate::mcp;
use protocol::pipe;
mod pinyin;
mod settings;
mod voice;

use crate::config::{colibri_home, expand_user_path, AgentConfig};
use crate::runtime_reload::{config_fingerprint, ConfigFingerprint};
use crate::session::AgentSession;
use crate::session_history::TranscriptHistoryLoader;
use crate::skills::SkillIndex;
use crate::transcript::TranscriptWriter;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use self::settings::{device_model, Settings};
use self::voice::Recording;
use crate::config_io::atomic_write;

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
    loaded_mcp: Option<ConfigFingerprint>,
    control: Option<Arc<Mutex<Option<crate::steering::SteerHandle>>>>,
    execution_epoch: Option<(Arc<std::sync::atomic::AtomicU64>, u64)>,
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
            loaded_mcp: None,
            control: None,
            execution_epoch: None,
        }
    }

    fn model_hint(&self) -> String {
        self.settings
            .config()
            .and_then(|config| device_model(&config, &self.settings.home))
            .err()
            .map(app_model_guidance)
            .unwrap_or_default()
    }

    fn history_loader(&self, _config: &AgentConfig) -> TranscriptHistoryLoader {
        TranscriptHistoryLoader::for_home(self.settings.home.clone())
    }

    fn history_page(&self, direction: &str, start: usize) -> Result<Value, String> {
        let config = self.settings.config()?;
        if !(config.session.transcript && config.session.restore_transcript) {
            return Ok(
                json!({"history":[],"start":0,"total":0,"has_before":false,"has_after":false}),
            );
        }
        crate::display_history::DisplayHistory::new(self.settings.home.clone(), "local")
            .page(direction, start)
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

    fn ask(&mut self, prompt: &str) -> Result<String, String> {
        let fingerprint = config_fingerprint(&self.settings.home.join("config.toml"));
        let mcp_changed = self.session.as_ref().is_some_and(|session| {
            config_fingerprint(&session.config.mcp_config_path) != self.loaded_mcp
        });
        if self.session.is_none()
            || self.settings_changed
            || fingerprint != self.loaded_config
            || mcp_changed
        {
            let config = self.settings.config()?;
            crate::memory::initialize(&config)?;
            let model = device_model(&config, &self.settings.home)?;
            let transcript = TranscriptWriter::for_session(&self.settings.home, &config.session)?
                .map(|writer| Arc::new(Mutex::new(writer)));
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
            self.loaded_config = config_fingerprint(&self.settings.home.join("config.toml"));
            self.loaded_mcp = config_fingerprint(&session.config.mcp_config_path);
            self.session = Some(session);
        }
        if self.control.is_some() {
            self.session
                .as_mut()
                .unwrap()
                .set_steer_notifier(Some(Arc::new(|text| {
                    let _ = send(&event("steering_applied", json!({"text":text})));
                })));
        }
        if let Some(slot) = &self.control {
            *slot.lock().map_err(|_| "control lock poisoned")? =
                Some(self.session.as_ref().unwrap().steer_handle());
        }
        let superseded = self
            .execution_epoch
            .as_ref()
            .is_some_and(|(epoch, generation)| {
                epoch.load(std::sync::atomic::Ordering::SeqCst) != *generation
            });
        let terminal =
            self.control.is_none() && io::stdin().is_terminal() && io::stdout().is_terminal();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pump = if terminal {
            self.session
                .as_mut()
                .unwrap()
                .set_steer_notifier(Some(Arc::new(|text| println!("{text}"))));
            let handle = self.session.as_ref().unwrap().steer_handle();
            let flag = Arc::clone(&stop);
            Some(std::thread::spawn(move || {
                crate::cli::run_steering_pump(&handle, &flag, |timeout| {
                    crate::repl_input::try_read_line(timeout, None)
                })
            }))
        } else {
            None
        };
        let response =
            self.session
                .as_mut()
                .unwrap()
                .submit(if superseded { "/stop" } else { prompt });
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(thread) = pump {
            let _ = thread.join();
        }
        Ok(response?.text)
    }

    fn reload(&mut self) -> Result<(), String> {
        if self.session.is_some() {
            self.settings_changed = true;
            device_model(&self.settings.config()?, &self.settings.home)?;
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
        let server = mcp::servers(&self.settings.config()?.mcp_servers)?
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
            "mcp" => Ok(mcp::servers(&self.settings.config()?.mcp_servers)?
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
        return Err("记忆阅读器无法打开符号链接".into());
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
            return Err("记忆文件不存在".into())
        }
        Err(error) => return Err(error.to_string()),
    };
    if bytes.len() > MAX_MEMORY_BYTES {
        return Err("文件超过 64 KiB，请让 Agent 分段读取".into());
    }
    let content = std::str::from_utf8(&bytes).map_err(|_| "记忆文件不是 UTF-8 文本")?;
    Ok(json!({"file":name,"text":content}))
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
            let mut page = app.history_page("last", 0)?;
            page["text"] = json!(app.model_hint());
            event("model_status", page)
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

fn repl(home: PathBuf) -> Result<(), String> {
    let mut app = App::new(home, Arc::new(|message| println!("{message}")));
    let config = app.settings.config()?;
    crate::memory::initialize(&config)?;
    device_model(&config, &app.settings.home)?;
    let terminal = io::stdin().is_terminal();
    println!(
        "Colibri REPL  /stop  /compact  /new  /voice  /set KEY VALUE  /mcp SERVER OP [NAME] [JSON]  /quit"
    );
    let stdin = io::stdin();
    let mut line = String::new();
    loop {
        print!("› ");
        io::stdout().flush().map_err(|e| e.to_string())?;
        line.clear();
        if stdin.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
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
                let eof = stdin.read_line(&mut choice).map_err(|e| e.to_string())? == 0;
                let draft = app.finish_voice(eof || choice.trim() == "/cancel")?;
                if !draft.is_empty() && terminal {
                    println!("语音草稿：{draft}\nEnter 发送，输入新文字替换，/cancel 取消。");
                    choice.clear();
                    let eof = stdin.read_line(&mut choice).map_err(|e| e.to_string())? == 0;
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
        if env::args().skip(1).nth(command_index).is_some_and(|arg| {
            matches!(
                arg.as_str(),
                "version" | "--version" | "help" | "--help" | "configure"
            )
        }) {
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

fn app_model_guidance(error: String) -> String {
    if error.starts_with("未配置 AI 模型") {
        "未配置 AI 模型。\n按 Enter 配置，或进入\n设置 → 配置 → 模型。".into()
    } else {
        error
    }
}
