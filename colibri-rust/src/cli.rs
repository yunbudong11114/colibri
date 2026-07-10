use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use crate::config::{expand_user_path, AgentConfig, DEFAULT_USER_CONFIG};
use crate::gateway::{
    format_gateway_status, restart_gateway, start_gateway, stop_gateway, GatewaySessionCache,
    GatewayStatus,
};
use crate::memory::MemoryContext;
use crate::model::build_model;
use crate::permissions::{PermissionPrompter, PermissionRequest};
use crate::repl_input::{read_repl_line_auto, ReplReadError};
use crate::session::AgentSession;
use crate::session_history::TranscriptHistoryLoader;
use crate::weixin::{
    perform_weixin_auth, permission_choice, poll_weixin_once, save_weixin_auth_config,
    send_weixin_media, send_weixin_text, InboundWeixinMessage,
};

use std::io::IsTerminal;

pub fn run_with_io<R: Read, W: Write, E: Write>(
    args: Vec<String>,
    stdin: R,
    mut stdout: W,
    mut stderr: E,
) -> i32 {
    run_with_io_mode(args, stdin, &mut stdout, &mut stderr, false)
}

/// Binary entry: enable TTY raw REPL when process stdin is a terminal.
pub fn run(args: Vec<String>) -> i32 {
    let prefer_tty = std::io::stdin().is_terminal();
    run_with_io_mode(
        args,
        std::io::stdin(),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
        prefer_tty,
    )
}

fn run_with_io_mode<R: Read, W: Write, E: Write>(
    args: Vec<String>,
    stdin: R,
    stdout: &mut W,
    stderr: &mut E,
    prefer_process_tty: bool,
) -> i32 {
    match run_inner(args, stdin, stdout, stderr, prefer_process_tty) {
        Ok(code) => code,
        Err(error) => {
            let _ = writeln!(stderr, "{}", error);
            1
        }
    }
}

fn run_inner<R: Read, W: Write, E: Write>(
    args: Vec<String>,
    stdin: R,
    stdout: &mut W,
    stderr: &mut E,
    prefer_process_tty: bool,
) -> Result<i32, String> {
    let mut stdin = std::io::BufReader::new(stdin);
    let mut index = 0;
    let mut config_path = None;
    if args.get(index).map(String::as_str) == Some("--config") {
        let Some(path) = args.get(index + 1) else {
            let _ = writeln!(stderr, "Usage: colibri [--config path] <command>");
            return Ok(2);
        };
        config_path = Some(PathBuf::from(path));
        index += 2;
    }
    let Some(command) = args.get(index).map(String::as_str) else {
        let _ = writeln!(stderr, "Usage: colibri [--config path] <command>");
        return Ok(2);
    };
    let rest = &args[index + 1..];

    if command == "gateway" {
        return gateway_command(rest, config_path, stdout, stderr);
    }

    let config = AgentConfig::load(config_path.as_deref())?;
    let mut status = StatusWriter {
        enabled: config.console.status,
        stderr,
    };

    match command {
        "diagnostics" => {
            for line in diagnostics(&config, config_path.as_ref()) {
                writeln!(stdout, "{}", line).map_err(|error| error.to_string())?;
            }
            Ok(0)
        }
        "ask" => {
            let Some(text) = rest.first() else {
                let _ = writeln!(status.stderr, "Usage: colibri ask <text>");
                return Ok(2);
            };
            status.write("ready", &[("model", config.model.model.as_str())]);
            status.write("thinking", &[]);
            write_memory_status(&config, &mut status);
            let model = build_model(&config.model)?;
            let restore = config.session.restore_transcript;
            let session_config = config.session.clone();
            let mut session = AgentSession::new(config, model);
            if restore {
                session = session.with_history_loader(Box::new(move || {
                    TranscriptHistoryLoader::default(&session_config).load()
                }));
            }
            let mut prompter = ConsolePermissionPrompter {
                stdin: &mut stdin,
                stdout,
            };
            let response = session.submit_with_permission_prompter(text, Some(&mut prompter))?;
            writeln!(stdout, "{}", response.text).map_err(|error| error.to_string())?;
            Ok(0)
        }
        "repl" => {
            status.write("ready", &[("model", config.model.model.as_str())]);
            repl(config, stdin, stdout, status, prefer_process_tty)
        }
        "auth" if rest.first().map(String::as_str) == Some("weixin") => {
            let (result, lines) = perform_weixin_auth(&config)?;
            for line in lines {
                writeln!(stdout, "{}", line).map_err(|error| error.to_string())?;
            }
            let active_path = config_path.unwrap_or_else(|| expand_user_path(DEFAULT_USER_CONFIG));
            save_weixin_auth_config(&active_path, &result)?;
            writeln!(stdout, "Weixin auth succeeded.").map_err(|error| error.to_string())?;
            writeln!(stdout, "user_id={}", result.user_id).map_err(|error| error.to_string())?;
            writeln!(stdout, "account_id={}", result.account_id)
                .map_err(|error| error.to_string())?;
            writeln!(stdout, "base_url={}", result.base_url).map_err(|error| error.to_string())?;
            writeln!(stdout, "Config updated: {}", active_path.display())
                .map_err(|error| error.to_string())?;
            Ok(0)
        }
        _ => {
            let _ = writeln!(status.stderr, "Unknown command: {}", command);
            Ok(2)
        }
    }
}

fn gateway_command<W: Write, E: Write>(
    rest: &[String],
    config_path: Option<PathBuf>,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<i32, String> {
    let Some(action) = rest.first().map(String::as_str) else {
        let _ = writeln!(
            stderr,
            "Usage: colibri gateway {{run,start,stop,restart,status}}"
        );
        return Ok(2);
    };
    match action {
        "status" => {
            for line in format_gateway_status(&GatewayStatus::current()) {
                writeln!(stdout, "{}", line).map_err(|error| error.to_string())?;
            }
            Ok(0)
        }
        "run" => {
            let config = AgentConfig::load(config_path.as_deref())?;
            run_gateway_foreground(config, stdout)
        }
        "start" | "stop" | "restart" => {
            let status = match action {
                "start" => start_gateway(config_path)?,
                "stop" => stop_gateway()?,
                "restart" => restart_gateway(config_path)?,
                _ => unreachable!(),
            };
            for line in format_gateway_status(&status) {
                writeln!(stdout, "{}", line).map_err(|error| error.to_string())?;
            }
            Ok(0)
        }
        _ => {
            let _ = writeln!(
                stderr,
                "Usage: colibri gateway {{run,start,stop,restart,status}}"
            );
            Ok(2)
        }
    }
}

fn run_gateway_foreground<W: Write>(config: AgentConfig, _stdout: &mut W) -> Result<i32, String> {
    if !config
        .gateway
        .enabled_channels
        .iter()
        .any(|item| item == "weixin")
        || !config.channels_weixin.enabled
    {
        return Err("No gateway channels are enabled".to_string());
    }
    let (work_tx, work_rx) = mpsc::sync_channel::<InboundWeixinMessage>(8);
    let (error_tx, error_rx) = mpsc::channel::<String>();
    let waiters: Arc<Mutex<HashMap<String, mpsc::SyncSender<String>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let worker_config = config.clone();
    let worker_waiters = Arc::clone(&waiters);
    let worker_error_tx = error_tx.clone();
    let worker = std::thread::spawn(move || {
        if let Err(error) = run_weixin_worker(worker_config, work_rx, worker_waiters) {
            let _ = worker_error_tx.send(error);
        }
    });
    let mut get_updates_buf = String::new();
    loop {
        if let Ok(error) = error_rx.try_recv() {
            let _ = worker.join();
            return Err(error);
        }
        let (next_buf, messages) = poll_weixin_once(&config, &get_updates_buf)?;
        get_updates_buf = next_buf;
        for message in messages {
            if deliver_weixin_waiter(&waiters, &message) {
                continue;
            }
            work_tx
                .send(message)
                .map_err(|error| format!("Weixin worker stopped: {}", error))?;
        }
    }
}

fn run_weixin_worker(
    config: AgentConfig,
    work_rx: mpsc::Receiver<InboundWeixinMessage>,
    waiters: Arc<Mutex<HashMap<String, mpsc::SyncSender<String>>>>,
) -> Result<(), String> {
    let mut sessions = GatewaySessionCache::new(config.clone())?;
    let config = Arc::new(config);
    while let Ok(message) = work_rx.recv() {
        let key = format!("weixin:{}", message.sender_id);
        let sender_id = message.sender_id.clone();
        let context_token = message.context_token.clone();
        let media_sender_config = Arc::clone(&config);
        let media_sender_id = sender_id.clone();
        let media_context_token = context_token.clone();
        let media_sender = Arc::new(move |media| {
            send_weixin_media(
                &media_sender_config,
                &media_sender_id,
                &media_context_token,
                &media,
            )
        });
        let session = sessions.get_or_create_with_metadata_and_media_sender(
            &key,
            std::collections::BTreeMap::from([
                ("channel".to_string(), "weixin".to_string()),
                ("sender_id".to_string(), sender_id.clone()),
                ("session_key".to_string(), key.clone()),
            ]),
            Some(media_sender),
        )?;
        let mut prompter = WeixinPermissionPrompter {
            config: Arc::clone(&config),
            sender_id: sender_id.clone(),
            context_token: context_token.clone(),
            waiters: Arc::clone(&waiters),
            timeout_seconds: 300,
        };
        let response = session.submit_with_media_and_permission_prompter(
            &message.text,
            message.media,
            Some(&mut prompter),
        )?;
        sessions.touch(&key);
        if !response.text.trim().is_empty() {
            send_weixin_text(&config, &sender_id, &context_token, &response.text)?;
        }
    }
    Ok(())
}

fn deliver_weixin_waiter(
    waiters: &Arc<Mutex<HashMap<String, mpsc::SyncSender<String>>>>,
    message: &InboundWeixinMessage,
) -> bool {
    if !message.media.is_empty() || message.text.trim().is_empty() {
        return false;
    }
    let waiter = waiters
        .lock()
        .ok()
        .and_then(|map| map.get(&message.sender_id).cloned());
    let Some(waiter) = waiter else {
        return false;
    };
    waiter.try_send(message.text.trim().to_string()).is_ok()
}

struct WeixinPermissionPrompter {
    config: Arc<AgentConfig>,
    sender_id: String,
    context_token: String,
    waiters: Arc<Mutex<HashMap<String, mpsc::SyncSender<String>>>>,
    timeout_seconds: u64,
}

impl PermissionPrompter for WeixinPermissionPrompter {
    fn confirm(&mut self, request: PermissionRequest) -> String {
        let (tx, rx) = mpsc::sync_channel(1);
        if let Ok(mut waiters) = self.waiters.lock() {
            waiters.insert(self.sender_id.clone(), tx);
        }
        let prompt =
            format_permission_prompt_lines(&request).join("\n") + "\nReply with y, s, e, p, or n.";
        let send_result =
            send_weixin_text(&self.config, &self.sender_id, &self.context_token, &prompt);
        if send_result.is_err() {
            if let Ok(mut waiters) = self.waiters.lock() {
                waiters.remove(&self.sender_id);
            }
            return "n".to_string();
        }
        let reply = rx
            .recv_timeout(std::time::Duration::from_secs(self.timeout_seconds))
            .ok();
        if let Ok(mut waiters) = self.waiters.lock() {
            waiters.remove(&self.sender_id);
        }
        reply
            .as_deref()
            .map(permission_choice)
            .unwrap_or_else(|| "n".to_string())
    }
}

fn repl<R: Read, W: Write, E: Write>(
    config: AgentConfig,
    stdin: std::io::BufReader<R>,
    stdout: &mut W,
    mut status: StatusWriter<'_, E>,
    prefer_process_tty: bool,
) -> Result<i32, String> {
    let model = build_model(&config.model)?;
    let mut session = AgentSession::new(config.clone(), model);
    if config.session.restore_transcript {
        let session_config = config.session.clone();
        session = session.with_history_loader(Box::new(move || {
            TranscriptHistoryLoader::default(&session_config).load()
        }));
    }
    let mut reader = stdin;
    let mut history: Vec<String> = Vec::new();
    let mut last_activity = Instant::now();
    loop {
        let idle_seconds = if config.session.idle_exit_enabled {
            config.session.idle_exit_seconds
        } else {
            0
        };
        if idle_seconds > 0 && last_activity.elapsed().as_secs() >= idle_seconds {
            status.write("idle_exit", &[("seconds", &idle_seconds.to_string())]);
            return Ok(0);
        }
        let timeout_remaining = if idle_seconds > 0 {
            (idle_seconds as f64) - last_activity.elapsed().as_secs_f64()
        } else {
            0.0
        };
        let timeout_remaining = timeout_remaining.max(0.0);
        let user_text = match read_repl_line_auto(
            "colibri> ",
            timeout_remaining,
            &history,
            &mut reader,
            stdout,
            prefer_process_tty,
        ) {
            Ok(Some(text)) => text,
            Ok(None) => {
                status.write("idle_exit", &[("seconds", &idle_seconds.to_string())]);
                return Ok(0);
            }
            Err(ReplReadError::Eof) => {
                writeln!(stdout).map_err(|error| error.to_string())?;
                return Ok(0);
            }
            Err(ReplReadError::Interrupted) => return Err("interrupted".to_string()),
            Err(ReplReadError::Io(message)) => return Err(message),
        };
        if user_text.trim() == "/quit" || user_text.trim() == "/exit" {
            return Ok(0);
        }
        if user_text.trim().is_empty() {
            continue;
        }
        history.push(user_text.clone());
        status.write("thinking", &[]);
        write_memory_status(&config, &mut status);
        let mut prompter = ConsolePermissionPrompter {
            stdin: &mut reader,
            stdout,
        };
        let response =
            session.submit_with_permission_prompter(&user_text, Some(&mut prompter))?;
        writeln!(stdout, "{}", response.text).map_err(|error| error.to_string())?;
        last_activity = Instant::now();
    }
}

struct ConsolePermissionPrompter<'a, R: BufRead, W: Write> {
    stdin: &'a mut R,
    stdout: &'a mut W,
}

impl<R: BufRead, W: Write> PermissionPrompter for ConsolePermissionPrompter<'_, R, W> {
    fn confirm(&mut self, request: PermissionRequest) -> String {
        for line in format_permission_prompt_lines(&request) {
            let _ = writeln!(self.stdout, "{}", line);
        }
        let prompt = match request.subject_kind.as_str() {
            "shell" => "[y] once [s] session [e] executable-session [p] project [n] deny: ",
            "file_path" => "[y] once [s] session-dir [p] project-dir [n] deny: ",
            _ => "[y] once [s] session [p] project [n] deny: ",
        };
        let _ = write!(self.stdout, "{}", prompt);
        let _ = self.stdout.flush();
        let mut line = String::new();
        match self.stdin.read_line(&mut line) {
            Ok(_) => line.trim().to_lowercase(),
            Err(_) => "n".to_string(),
        }
    }
}

fn format_permission_prompt_lines(request: &PermissionRequest) -> Vec<String> {
    match request.subject_kind.as_str() {
        "shell" => vec![format!(
            "shell: {}",
            request.shell_command.as_deref().unwrap_or("")
        )],
        "file_path" => {
            let mut lines = vec![format!(
                "file: {} {}",
                request.tool_name,
                request.file_path.as_deref().unwrap_or("")
            )];
            if let Some(command) = &request.shell_command {
                lines.push(format!("command: {}", command));
            }
            if request.tool_name == "files.write" {
                lines.push(content_summary(request.arguments.get("content")));
            }
            lines
        }
        _ if request.tool_name == "memory.write" => {
            let mut lines = vec![format!("tool: {}", request.tool_name)];
            if let Some(target) = request
                .arguments
                .get("file")
                .or_else(|| request.arguments.get("topic"))
            {
                lines.push(format!("file: {}", target));
            }
            if let Some(mode) = request.arguments.get("mode") {
                lines.push(format!("mode: {}", mode));
            }
            lines.push(content_summary(request.arguments.get("content")));
            lines
        }
        _ => vec![format!(
            "tool: {} {}",
            request.tool_name,
            summarized_arguments(&request.arguments)
        )],
    }
}

fn content_summary(value: Option<&String>) -> String {
    let value = value.map(String::as_str).unwrap_or("");
    let char_count = value.chars().count();
    let byte_count = value.len();
    let mut preview = value.replace('\n', "\\n");
    if preview.chars().count() > 40 {
        preview = preview.chars().take(37).collect::<String>() + "...";
    }
    format!(
        "content: {} chars, {} bytes, preview='{}'",
        char_count, byte_count, preview
    )
}

fn summarized_arguments(arguments: &std::collections::BTreeMap<String, String>) -> String {
    let pairs = arguments
        .iter()
        .map(|(key, value)| format!("{}={}", key, value))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{}}}", pairs)
}

fn write_memory_status<E: Write>(config: &AgentConfig, status: &mut StatusWriter<'_, E>) {
    let Ok(memory) = MemoryContext::new(config.clone()).load() else {
        return;
    };
    if !memory.files.is_empty() {
        status.write("memory", &[("files", &memory.files.join(","))]);
    }
}

fn diagnostics(config: &AgentConfig, config_path: Option<&PathBuf>) -> Vec<String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let project_permissions = if cwd.join(".colibri/permissions.toml").exists() {
        "present"
    } else {
        "missing"
    };
    vec![
        "colibri diagnostics".to_string(),
        format!("rust=unknown platform={}", std::env::consts::OS),
        format!(
            "provider={} model={}",
            config.model.provider, config.model.model
        ),
        format!(
            "config={}",
            config_path
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "default".to_string())
        ),
        format!("tools={}", config.tools.enabled.join(",")),
        format!(
            "memory_root={} exists={}",
            config.memory.root.display(),
            if config.memory.root.exists() { "true" } else { "false" }
        ),
        format!(
            "skills_dirs={} skills_found={}",
            config.skills.dirs.len(),
            count_available_skills(config)
        ),
        format!("project_permissions={}", project_permissions),
        format!(
            "transcript={} rss_kb={}",
            if config.session.transcript {
                "true"
            } else {
                "false"
            },
            rss_kb()
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ),
        format!(
            "trigger_message_limit={} recent_message_limit={} model_input_char_limit={} summary_max_chars={}",
            config.session.trigger_message_limit,
            config.session.recent_message_limit,
            config.session.model_input_char_limit,
            config.session.summary_max_chars
        ),
    ]
}

fn count_available_skills(config: &AgentConfig) -> usize {
    let mut count = 1;
    for dir in &config.skills.dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.path().join("SKILL.md").is_file() {
                count += 1;
            }
        }
    }
    count
}

fn rss_kb() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<u64>().ok());
        }
    }
    None
}

struct StatusWriter<'a, E: Write> {
    enabled: bool,
    stderr: &'a mut E,
}

impl<E: Write> StatusWriter<'_, E> {
    fn write(&mut self, name: &str, fields: &[(&str, &str)]) {
        if !self.enabled {
            return;
        }
        let mut line = format!("[colibri] {}", name);
        for (key, value) in fields {
            line.push(' ');
            line.push_str(key);
            line.push('=');
            line.push_str(value);
        }
        let _ = writeln!(self.stderr, "{}", line);
    }
}
