use std::fs;
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{expand_user_path, rss_kb, AgentConfig, DEFAULT_USER_CONFIG};
use crate::console::format_answer_for_console;
use crate::gateway::{
    format_gateway_status, restart_gateway, run_gateway, start_gateway, stop_gateway, GatewayStatus,
};
use crate::model::build_model;
use crate::permissions::{PermissionPrompter, PermissionRequest};
use crate::repl_input::{
    read_repl_line_auto, stdin_supports_steering_pump, try_read_line, ReplReadError,
};
use crate::session::AgentSession;
use crate::session_history::TranscriptHistoryLoader;
use crate::steering::SteerHandle;
use crate::weixin::{perform_weixin_auth, save_weixin_auth_config};

use std::io::IsTerminal;

pub fn run_with_io<R: Read, W: Write + Send + 'static, E: Write + Send + 'static>(
    args: Vec<String>,
    stdin: R,
    stdout: W,
    stderr: E,
) -> i32 {
    run_with_io_mode(args, stdin, stdout, stderr, false)
}

/// Binary entry: enable TTY raw REPL when process stdin is a terminal.
pub fn run(args: Vec<String>) -> i32 {
    let prefer_tty = std::io::stdin().is_terminal();
    run_with_io_mode(
        args,
        std::io::stdin(),
        std::io::stdout(),
        std::io::stderr(),
        prefer_tty,
    )
}

fn run_with_io_mode<R: Read, W: Write + Send + 'static, E: Write + Send + 'static>(
    args: Vec<String>,
    stdin: R,
    stdout: W,
    stderr: E,
    prefer_process_tty: bool,
) -> i32 {
    let stdout = Arc::new(Mutex::new(stdout));
    let stderr = Arc::new(Mutex::new(stderr));
    match run_inner(
        args,
        stdin,
        Arc::clone(&stdout),
        Arc::clone(&stderr),
        prefer_process_tty,
    ) {
        Ok(code) => code,
        Err(error) => {
            if let Ok(mut stderr) = stderr.lock() {
                let _ = writeln!(stderr, "{}", error);
            }
            1
        }
    }
}

fn run_inner<R: Read, W: Write + Send + 'static, E: Write + Send + 'static>(
    args: Vec<String>,
    stdin: R,
    stdout: Arc<Mutex<W>>,
    stderr: Arc<Mutex<E>>,
    prefer_process_tty: bool,
) -> Result<i32, String> {
    let mut stdin = std::io::BufReader::new(stdin);
    let mut index = 0;
    let mut config_path = None;
    if args.get(index).map(String::as_str) == Some("--config") {
        let Some(path) = args.get(index + 1) else {
            let _ = writeln!(
                stderr.lock().map_err(|_| "stderr lock poisoned")?,
                "Usage: colibri [--config path] <command>"
            );
            return Ok(2);
        };
        config_path = Some(PathBuf::from(path));
        index += 2;
    }
    let Some(command) = args.get(index).map(String::as_str) else {
        let _ = writeln!(
            stderr.lock().map_err(|_| "stderr lock poisoned")?,
            "Usage: colibri [--config path] <command>"
        );
        return Ok(2);
    };
    let rest = &args[index + 1..];

    if command == "gateway" {
        return gateway_command(rest, config_path, &stdout, &stderr);
    }

    let config = AgentConfig::load(config_path.as_deref())?;
    let mut status = StatusWriter {
        enabled: config.console.status,
        stderr: Arc::clone(&stderr),
    };

    match command {
        "diagnostics" => {
            for line in diagnostics(&config, config_path.as_ref()) {
                writeln!(
                    stdout.lock().map_err(|_| "stdout lock poisoned")?,
                    "{}",
                    line
                )
                .map_err(|error| error.to_string())?;
            }
            Ok(0)
        }
        "ask" => {
            let Some(text) = rest.first() else {
                let _ = writeln!(
                    stderr.lock().map_err(|_| "stderr lock poisoned")?,
                    "Usage: colibri ask <text>"
                );
                return Ok(2);
            };
            let plain_answer = config.console.plain_answer;
            let status_enabled = config.console.status;
            status.write("ready", &[("model", config.model.model.as_str())]);
            status.write("thinking", &[]);
            let model = build_model(&config.model)?;
            let restore = config.session.restore_transcript;
            let session_config = config.session.clone();
            let status_stderr = Arc::clone(&stderr);
            let mut session = AgentSession::new(config, model).with_status_callback(
                status_enabled,
                Arc::new(move |line: &str| {
                    if let Ok(mut stderr) = status_stderr.lock() {
                        let _ = writeln!(stderr, "{line}");
                    }
                }),
            );
            if restore {
                session = session.with_history_loader(Box::new(move || {
                    TranscriptHistoryLoader::default(&session_config).load()
                }));
            }
            let result = (|| -> Result<i32, String> {
                let response = {
                    let mut stdout_guard = stdout.lock().map_err(|_| "stdout lock poisoned")?;
                    let mut prompter = ConsolePermissionPrompter {
                        stdin: &mut stdin,
                        stdout: &mut *stdout_guard,
                    };
                    session.submit_with_permission_prompter(text, Some(&mut prompter))?
                };
                writeln!(
                    stdout.lock().map_err(|_| "stdout lock poisoned")?,
                    "{}",
                    format_answer_for_console(&response.text, plain_answer)
                )
                .map_err(|error| error.to_string())?;
                Ok(if response.error_type.is_some() { 1 } else { 0 })
            })();
            session.close();
            result
        }
        "repl" => {
            status.write("ready", &[("model", config.model.model.as_str())]);
            repl(config, stdin, stdout, stderr, status, prefer_process_tty)
        }
        "auth" if rest.first().map(String::as_str) == Some("weixin") => {
            let mut stdout = stdout.lock().map_err(|_| "stdout lock poisoned")?;
            let result = perform_weixin_auth(&config, |line| {
                writeln!(stdout, "{}", line).map_err(|error| error.to_string())?;
                stdout.flush().map_err(|error| error.to_string())
            })?;
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
            let _ = writeln!(
                stderr.lock().map_err(|_| "stderr lock poisoned")?,
                "Unknown command: {}",
                command
            );
            Ok(2)
        }
    }
}

fn gateway_command<W: Write + Send + 'static, E: Write + Send + 'static>(
    rest: &[String],
    config_path: Option<PathBuf>,
    stdout: &Arc<Mutex<W>>,
    stderr: &Arc<Mutex<E>>,
) -> Result<i32, String> {
    let Some(action) = rest.first().map(String::as_str) else {
        let _ = writeln!(
            stderr.lock().map_err(|_| "stderr lock poisoned")?,
            "Usage: colibri gateway {{run,start,stop,restart,status}}"
        );
        return Ok(2);
    };
    match action {
        "status" => {
            let mut stdout = stdout.lock().map_err(|_| "stdout lock poisoned")?;
            for line in format_gateway_status(&GatewayStatus::current()) {
                writeln!(stdout, "{}", line).map_err(|error| error.to_string())?;
            }
            Ok(0)
        }
        "run" => {
            let config = AgentConfig::load(config_path.as_deref())?;
            run_gateway_foreground(config)
        }
        "start" | "stop" | "restart" => {
            let status = match action {
                "start" => start_gateway(config_path)?,
                "stop" => stop_gateway()?,
                "restart" => restart_gateway(config_path)?,
                _ => unreachable!(),
            };
            let mut stdout = stdout.lock().map_err(|_| "stdout lock poisoned")?;
            for line in format_gateway_status(&status) {
                writeln!(stdout, "{}", line).map_err(|error| error.to_string())?;
            }
            Ok(0)
        }
        _ => {
            let _ = writeln!(
                stderr.lock().map_err(|_| "stderr lock poisoned")?,
                "Usage: colibri gateway {{run,start,stop,restart,status}}"
            );
            Ok(2)
        }
    }
}

fn run_gateway_foreground(config: AgentConfig) -> Result<i32, String> {
    run_gateway(config)
}

/// Background loop: forward stdin lines to `session.steer` while a turn runs.
///
/// Approach C: do not read stdin while a permission prompt may be pending, so
/// permission input is not stolen. No extra prompt is printed.
#[allow(unused_assignments)] // notified_pending mirrors Python debounce; top-level pending continue is the main gate
pub fn run_steering_pump<R, N, S>(
    handle: &SteerHandle,
    stop: &AtomicBool,
    mut read_line: R,
    mut notify_permission_pending: N,
    mut sleep_fn: S,
) where
    R: FnMut(f64) -> Option<String>,
    N: FnMut(),
    S: FnMut(Duration),
{
    let mut notified_pending = false;
    while !stop.load(Ordering::SeqCst) {
        if handle.is_permission_pending() {
            sleep_fn(Duration::from_millis(50));
            continue;
        }
        notified_pending = false;
        let line = read_line(0.2);
        let Some(line) = line else {
            continue;
        };
        let stripped = line.trim();
        if stripped.is_empty() {
            continue;
        }
        if !handle.steer(stripped) {
            if handle.is_permission_pending() && !notified_pending {
                notify_permission_pending();
                notified_pending = true;
            }
        }
    }
}

fn spawn_repl_steering_pump<E: Write + Send + 'static>(
    handle: SteerHandle,
    stop: Arc<AtomicBool>,
    status_enabled: bool,
    stderr: Arc<Mutex<E>>,
) -> Option<thread::JoinHandle<()>> {
    if !stdin_supports_steering_pump() {
        return None;
    }
    Some(thread::spawn(move || {
        run_steering_pump(
            &handle,
            stop.as_ref(),
            |timeout| try_read_line(timeout, Some(&|| handle.is_permission_pending())),
            || {
                if !status_enabled {
                    return;
                }
                if let Ok(mut stderr) = stderr.lock() {
                    let _ = writeln!(stderr, "[colibri] permission_pending");
                }
            },
            thread::sleep,
        );
    }))
}

fn repl<R: Read, W: Write + Send + 'static, E: Write + Send + 'static>(
    config: AgentConfig,
    stdin: std::io::BufReader<R>,
    stdout: Arc<Mutex<W>>,
    stderr: Arc<Mutex<E>>,
    mut status: StatusWriter<E>,
    prefer_process_tty: bool,
) -> Result<i32, String> {
    let plain_answer = config.console.plain_answer;
    let status_enabled = config.console.status;
    let model = build_model(&config.model)?;
    let status_stderr = Arc::clone(&stderr);
    let mut session = AgentSession::new(config.clone(), model).with_status_callback(
        status_enabled,
        Arc::new(move |line: &str| {
            if let Ok(mut stderr) = status_stderr.lock() {
                let _ = writeln!(stderr, "{line}");
            }
        }),
    );
    if config.session.restore_transcript {
        let session_config = config.session.clone();
        session = session.with_history_loader(Box::new(move || {
            TranscriptHistoryLoader::default(&session_config).load()
        }));
    }
    let result = repl_loop(
        &mut session,
        &config,
        stdin,
        stdout,
        stderr,
        &mut status,
        prefer_process_tty,
        plain_answer,
        status_enabled,
    );
    session.close();
    result
}

fn repl_loop<R: Read, W: Write + Send + 'static, E: Write + Send + 'static>(
    session: &mut AgentSession,
    config: &AgentConfig,
    stdin: std::io::BufReader<R>,
    stdout: Arc<Mutex<W>>,
    stderr: Arc<Mutex<E>>,
    status: &mut StatusWriter<E>,
    prefer_process_tty: bool,
    plain_answer: bool,
    status_enabled: bool,
) -> Result<i32, String> {
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
        let user_text = {
            let mut stdout_guard = stdout.lock().map_err(|_| "stdout lock poisoned")?;
            match read_repl_line_auto(
                "colibri> ",
                timeout_remaining,
                &history,
                &mut reader,
                &mut *stdout_guard,
                prefer_process_tty,
            ) {
                Ok(Some(text)) => text,
                Ok(None) => {
                    drop(stdout_guard);
                    status.write("idle_exit", &[("seconds", &idle_seconds.to_string())]);
                    return Ok(0);
                }
                Err(ReplReadError::Eof) => {
                    writeln!(stdout_guard).map_err(|error| error.to_string())?;
                    return Ok(0);
                }
                Err(ReplReadError::Interrupted) => return Err("interrupted".to_string()),
                Err(ReplReadError::Io(message)) => return Err(message),
            }
        };
        if user_text.trim() == "/quit" || user_text.trim() == "/exit" {
            return Ok(0);
        }
        if user_text.trim().is_empty() {
            continue;
        }
        history.push(user_text.clone());
        status.write("thinking", &[]);
        let stop = Arc::new(AtomicBool::new(false));
        let pump = if prefer_process_tty {
            spawn_repl_steering_pump(
                session.steer_handle(),
                Arc::clone(&stop),
                status_enabled,
                Arc::clone(&stderr),
            )
        } else {
            None
        };
        let submit_result = {
            let mut stdout_guard = stdout.lock().map_err(|_| "stdout lock poisoned")?;
            let mut prompter = ConsolePermissionPrompter {
                stdin: &mut reader,
                stdout: &mut *stdout_guard,
            };
            session.submit_with_permission_prompter(&user_text, Some(&mut prompter))
        };
        stop.store(true, Ordering::SeqCst);
        if let Some(handle) = pump {
            let _ = handle.join();
        }
        let response = submit_result?;
        writeln!(
            stdout.lock().map_err(|_| "stdout lock poisoned")?,
            "{}",
            format_answer_for_console(&response.text, plain_answer)
        )
        .map_err(|error| error.to_string())?;
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
            "shell" => {
                "[1] once [2] session-command [3] session-executable [4] user-command [5] user-executable [0] deny: "
            }
            "file_path" => "[1] once [2] session-dir [4] user-dir [0] deny: ",
            _ => "[1] once [2] session [4] user [0] deny: ",
        };
        let _ = write!(self.stdout, "{}", prompt);
        let _ = self.stdout.flush();
        let mut line = String::new();
        match self.stdin.read_line(&mut line) {
            Ok(_) => line.trim().to_lowercase(),
            Err(_) => "0".to_string(),
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

fn diagnostics(config: &AgentConfig, config_path: Option<&PathBuf>) -> Vec<String> {
    let user_permissions_path = expand_user_path("~/.colibri/permissions.toml");
    let user_permissions = if user_permissions_path.exists() {
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
            "skills_dir={} skills_found={}",
            config.skills.dir.display(),
            count_available_skills(config)
        ),
        format!("user_permissions={}", user_permissions),
        format!(
            "transcript={} rss_kb={}",
            if config.session.transcript {
                "true"
            } else {
                "false"
            },
            rss_kb(None)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ),
        format!(
            "trigger_message_limit={} recent_message_limit={} input_context_tokens={} summary_max_chars={}",
            config.session.trigger_message_limit,
            config.session.recent_message_limit,
            config.model.input_context_tokens,
            config.session.summary_max_chars
        ),
    ]
}

fn count_available_skills(config: &AgentConfig) -> usize {
    let mut count = 1;
    let Ok(entries) = fs::read_dir(&config.skills.dir) else {
        return count;
    };
    for entry in entries.flatten() {
        if entry.path().join("SKILL.md").is_file() {
            count += 1;
        }
    }
    count
}

struct StatusWriter<E: Write + Send + 'static> {
    enabled: bool,
    stderr: Arc<Mutex<E>>,
}

impl<E: Write + Send + 'static> StatusWriter<E> {
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
        if let Ok(mut stderr) = self.stderr.lock() {
            let _ = writeln!(stderr, "{}", line);
        }
    }
}
