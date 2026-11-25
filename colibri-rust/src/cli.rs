use std::fs;
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use crate::config::{default_config_path, rss_kb, AgentConfig};
use crate::console::format_answer_for_console;
use crate::gateway::{
    format_gateway_log, format_gateway_status, restart_gateway, run_gateway, start_gateway,
    stop_gateway, GatewayStatus,
};
use crate::hardware::{probe_hardware, HardwareSimulator};
use crate::model::build_model;
use crate::repl_input::{
    read_repl_line_auto, stdin_supports_steering_pump, try_read_line, ReplReadError,
};
use crate::runtime_reload::{PartialRuntimeReloader, RuntimeReloadResult};
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
        "hardware" if rest.first().map(String::as_str) == Some("probe") => {
            let output = serde_json::to_string_pretty(&probe_hardware())
                .map_err(|error| error.to_string())?;
            writeln!(
                stdout.lock().map_err(|_| "stdout lock poisoned")?,
                "{}",
                output
            )
            .map_err(|error| error.to_string())?;
            Ok(0)
        }
        "hardware" if rest.first().map(String::as_str) == Some("simulate") => {
            let mut simulator = HardwareSimulator::default();
            let mut line = String::new();
            loop {
                line.clear();
                let count = stdin
                    .read_line(&mut line)
                    .map_err(|error| error.to_string())?;
                if count == 0 {
                    break;
                }
                if line.trim().is_empty() {
                    continue;
                }
                let response = if line.len() > config.hardware.max_transfer_bytes {
                    "{\"id\":null,\"ok\":false,\"error\":\"request exceeds max_transfer_bytes\"}"
                        .to_string()
                } else {
                    simulator.handle_line(&line)
                };
                let response = if response.len() > config.hardware.max_transfer_bytes {
                    let request_id = serde_json::from_str::<serde_json::Value>(&line)
                        .ok()
                        .and_then(|value| value.get("id").cloned())
                        .unwrap_or(serde_json::Value::Null);
                    serde_json::json!({
                        "id":request_id,
                        "ok":false,
                        "error":"response exceeds max_transfer_bytes"
                    })
                    .to_string()
                } else {
                    response
                };
                let mut output = stdout.lock().map_err(|_| "stdout lock poisoned")?;
                writeln!(output, "{}", response).map_err(|error| error.to_string())?;
                output.flush().map_err(|error| error.to_string())?;
            }
            Ok(0)
        }
        "hardware" => {
            let _ = writeln!(
                stderr.lock().map_err(|_| "stderr lock poisoned")?,
                "Usage: colibri hardware {{probe,simulate}}"
            );
            Ok(2)
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
                session = session.with_model_history_loader(Box::new(move || {
                    TranscriptHistoryLoader::default(&session_config).load_model("local")
                }));
            }
            let result = (|| -> Result<i32, String> {
                let response = session.submit(text)?;
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
            repl(
                config,
                config_path,
                stdin,
                stdout,
                stderr,
                status,
                prefer_process_tty,
            )
        }
        "auth" if rest.first().map(String::as_str) == Some("weixin") => {
            let mut stdout = stdout.lock().map_err(|_| "stdout lock poisoned")?;
            let result = perform_weixin_auth(&config, |line| {
                writeln!(stdout, "{}", line).map_err(|error| error.to_string())?;
                stdout.flush().map_err(|error| error.to_string())
            })?;
            let active_path = config_path.unwrap_or_else(default_config_path);
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
            run_gateway_foreground(config, config_path)
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
            let success = status.reason.is_empty() && status.running;
            Ok(i32::from(if action == "stop" {
                status.running
            } else {
                !success
            }))
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

fn run_gateway_foreground(
    config: AgentConfig,
    config_path: Option<PathBuf>,
) -> Result<i32, String> {
    run_gateway(config, config_path)
        .map_err(|error| format_gateway_log(&format!("stopped: {}", error)))
}

/// Background loop: forward stdin lines to `session.steer` while a turn runs.
pub fn run_steering_pump(
    handle: &SteerHandle,
    stop: &AtomicBool,
    mut read_line: impl FnMut(f64) -> Option<String>,
) {
    while !stop.load(Ordering::SeqCst) {
        if let Some(line) = read_line(0.2) {
            handle.steer(line.trim());
        }
    }
}

fn spawn_repl_steering_pump(
    handle: SteerHandle,
    stop: Arc<AtomicBool>,
) -> Option<thread::JoinHandle<()>> {
    if !stdin_supports_steering_pump() {
        return None;
    }
    Some(thread::spawn(move || {
        run_steering_pump(&handle, &stop, |timeout| try_read_line(timeout, None));
    }))
}

fn repl<R: Read, W: Write + Send + 'static, E: Write + Send + 'static>(
    config: AgentConfig,
    config_path: Option<PathBuf>,
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
        session = session.with_model_history_loader(Box::new(move || {
            TranscriptHistoryLoader::default(&session_config).load_model("local")
        }));
    }
    let result = repl_loop(
        &mut session,
        config_path,
        stdin,
        stdout,
        stderr,
        &mut status,
        prefer_process_tty,
        plain_answer,
    );
    session.close();
    result
}

fn repl_loop<R: Read, W: Write + Send + 'static, E: Write + Send + 'static>(
    session: &mut AgentSession,
    config_path: Option<PathBuf>,
    stdin: std::io::BufReader<R>,
    stdout: Arc<Mutex<W>>,
    stderr: Arc<Mutex<E>>,
    status: &mut StatusWriter<E>,
    prefer_process_tty: bool,
    plain_answer: bool,
) -> Result<i32, String> {
    let mut reader = stdin;
    let mut history: Vec<String> = Vec::new();
    let mut last_activity = Instant::now();
    let active_config_path = config_path.unwrap_or_else(default_config_path);
    let mut runtime_reloader =
        PartialRuntimeReloader::new(active_config_path, session.runtime_snapshot());
    loop {
        let idle_seconds = if session.config.session.idle_exit_enabled {
            session.config.session.idle_exit_seconds
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
        match runtime_reloader.reload_if_changed() {
            RuntimeReloadResult::Unchanged => {}
            RuntimeReloadResult::Reloaded(snapshot) => {
                session.adopt_runtime(snapshot.config, snapshot.model);
                status.write(
                    "config_reloaded",
                    &[("model", session.config.model.model.as_str())],
                );
            }
            RuntimeReloadResult::Rejected(error) => {
                let _ = writeln!(
                    stderr.lock().map_err(|_| "stderr lock poisoned")?,
                    "[colibri] config reload skipped: {}",
                    error
                );
            }
        }
        status.write("thinking", &[]);
        let stop = Arc::new(AtomicBool::new(false));
        let pump = if prefer_process_tty {
            spawn_repl_steering_pump(session.steer_handle(), Arc::clone(&stop))
        } else {
            None
        };
        let submit_result = session.submit(&user_text);
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

fn diagnostics(config: &AgentConfig, config_path: Option<&PathBuf>) -> Vec<String> {
    let mut metadata_config = config.model.clone();
    if metadata_config.api_key.is_empty() {
        metadata_config.api_key = std::env::var("COLIBRI_API_KEY").unwrap_or_default();
    }
    #[cfg(unix)]
    let capabilities =
        crate::model_metadata::peek(&metadata_config, &crate::config::colibri_home());
    #[cfg(not(unix))]
    let capabilities = crate::model_capabilities::resolve(&metadata_config);

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
            if config.memory.root.exists() {
                "true"
            } else {
                "false"
            }
        ),
        format!(
            "skills_dir={} skills_found={}",
            config.skills.dir.display(),
            count_available_skills(config)
        ),
        "execution=direct".to_string(),
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
            "compaction=auto input_context_tokens={} max_output_tokens={} capacity_source={}",
            capabilities.context_window, capabilities.max_output_tokens, capabilities.source
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
