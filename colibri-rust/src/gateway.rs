use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::{colibri_home, rss_kb as process_rss_kb, AgentConfig};
use crate::messages::MediaPart;
use crate::model::{build_model, ModelClient};
use crate::session::AgentSession;
use crate::session_history::TranscriptHistoryLoader;
use crate::steering::SteerHandle;
use crate::transcript::{beijing_timestamp_now, TranscriptWriter};

#[derive(Clone, Debug)]
pub struct GatewayStatus {
    pub running: bool,
    pub pid: Option<String>,
    pub rss_kb: Option<String>,
    pub config_path: String,
    pub cwd: String,
    pub log_path: PathBuf,
    pub state_path: PathBuf,
    pub started_at: String,
    pub reason: String,
}

pub struct GatewaySessionCache {
    config: Arc<AgentConfig>,
    model: Arc<Mutex<Box<dyn ModelClient>>>,
    transcript: Option<Arc<Mutex<TranscriptWriter>>>,
    history_loader: Option<Arc<dyn Fn() -> Vec<crate::messages::Message> + Send + Sync>>,
    max_sessions: usize,
    idle_seconds: u64,
    entries: BTreeMap<String, GatewaySessionEntry>,
    /// Cloned independently of session ownership so receive can steer while
    /// the worker holds the session outside this cache mutex during submit.
    steer_handles: BTreeMap<String, SteerHandle>,
}

struct GatewaySessionEntry {
    session: AgentSession,
    last_activity_at: Instant,
}

impl GatewaySessionCache {
    pub fn new(config: AgentConfig) -> Result<Self, String> {
        let config = Arc::new(config);
        let model = Arc::new(Mutex::new(build_model(&config.model)?));
        let transcript = if config.session.transcript {
            TranscriptWriter::default_with_metadata_and_limits(
                BTreeMap::new(),
                config.session.transcript_retention_days,
                config.session.transcript_max_total_bytes,
            )
            .ok()
            .map(|writer| Arc::new(Mutex::new(writer)))
        } else {
            None
        };
        let history_loader = if config.session.restore_transcript {
            let session_config = config.session.clone();
            Some(Arc::new(move || {
                TranscriptHistoryLoader::default(&session_config).load()
            })
                as Arc<dyn Fn() -> Vec<crate::messages::Message> + Send + Sync>)
        } else {
            None
        };
        Ok(Self {
            max_sessions: config.gateway.max_sessions.max(1),
            idle_seconds: config.gateway.session_idle_seconds,
            config,
            model,
            transcript,
            history_loader,
            entries: BTreeMap::new(),
            steer_handles: BTreeMap::new(),
        })
    }

    pub fn get_or_create(&mut self, key: &str) -> Result<&mut AgentSession, String> {
        self.get_or_create_with_metadata(key, BTreeMap::new())
    }

    pub fn get_or_create_with_metadata(
        &mut self,
        key: &str,
        metadata: BTreeMap<String, String>,
    ) -> Result<&mut AgentSession, String> {
        self.get_or_create_with_metadata_and_media_sender(key, metadata, None)
    }

    pub fn get_or_create_with_metadata_and_media_sender(
        &mut self,
        key: &str,
        metadata: BTreeMap<String, String>,
        media_sender: Option<Arc<dyn Fn(MediaPart) -> Result<(), String> + Send + Sync>>,
    ) -> Result<&mut AgentSession, String> {
        self.evict_idle();
        if self.entries.contains_key(key) {
            let handle = {
                let entry = self.entries.get_mut(key).unwrap();
                entry.last_activity_at = Instant::now();
                entry.session.set_media_sender(media_sender);
                entry.session.steer_handle()
            };
            self.steer_handles.insert(key.to_string(), handle);
            return Ok(&mut self.entries.get_mut(key).unwrap().session);
        }
        while self.entries.len() >= self.max_sessions {
            self.evict_oldest();
        }
        let mut session = AgentSession::from_shared(
            Arc::clone(&self.config),
            Arc::clone(&self.model),
            self.transcript.as_ref().map(Arc::clone),
            metadata,
        );
        if let Some(loader) = &self.history_loader {
            let loader = Arc::clone(loader);
            session = session.with_history_loader(Box::new(move || loader()));
        }
        self.steer_handles
            .insert(key.to_string(), session.steer_handle());
        self.entries.insert(
            key.to_string(),
            GatewaySessionEntry {
                session,
                last_activity_at: Instant::now(),
            },
        );
        let session = &mut self.entries.get_mut(key).unwrap().session;
        session.set_media_sender(media_sender);
        Ok(session)
    }

    /// Look up an existing session without creating one (Python `get_existing`).
    pub fn get_existing(&mut self, key: &str) -> Option<&mut AgentSession> {
        self.entries.get_mut(key).map(|entry| &mut entry.session)
    }

    /// Clone the steer handle for a session key, if one has been registered.
    /// Safe to call while the worker owns the session outside this cache.
    pub fn steer_handle_for(&self, key: &str) -> Option<SteerHandle> {
        self.steer_handles.get(key).cloned()
    }

    /// Route text to an active turn without creating a session.
    pub fn try_steer(&self, key: &str, text: &str) -> bool {
        self.steer_handle_for(key)
            .map(|handle| handle.steer(text))
            .unwrap_or(false)
    }

    /// Take ownership of a session for submit so the cache mutex is not held
    /// during the turn. Steer handles remain registered for receive-loop try_steer.
    pub fn take_or_create_with_metadata_and_media_sender(
        &mut self,
        key: &str,
        metadata: BTreeMap<String, String>,
        media_sender: Option<Arc<dyn Fn(MediaPart) -> Result<(), String> + Send + Sync>>,
    ) -> Result<AgentSession, String> {
        self.evict_idle();
        if let Some(mut entry) = self.entries.remove(key) {
            entry.last_activity_at = Instant::now();
            entry.session.set_media_sender(media_sender);
            self.steer_handles
                .insert(key.to_string(), entry.session.steer_handle());
            return Ok(entry.session);
        }
        while self.entries.len() >= self.max_sessions {
            self.evict_oldest();
        }
        let mut session = AgentSession::from_shared(
            Arc::clone(&self.config),
            Arc::clone(&self.model),
            self.transcript.as_ref().map(Arc::clone),
            metadata,
        );
        if let Some(loader) = &self.history_loader {
            let loader = Arc::clone(loader);
            session = session.with_history_loader(Box::new(move || loader()));
        }
        session.set_media_sender(media_sender);
        self.steer_handles
            .insert(key.to_string(), session.steer_handle());
        Ok(session)
    }

    /// Return a session taken via `take_or_create_*` to the cache.
    pub fn put_back(&mut self, key: &str, session: AgentSession) {
        self.steer_handles
            .insert(key.to_string(), session.steer_handle());
        self.entries.insert(
            key.to_string(),
            GatewaySessionEntry {
                session,
                last_activity_at: Instant::now(),
            },
        );
    }

    pub fn touch(&mut self, key: &str) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_activity_at = Instant::now();
        }
    }

    pub fn close(&mut self) {
        let keys: Vec<String> = self.entries.keys().cloned().collect();
        for key in keys {
            if let Some(mut entry) = self.entries.remove(&key) {
                entry.session.close();
            }
            self.steer_handles.remove(&key);
        }
        self.steer_handles.clear();
        if let Some(transcript) = self.transcript.take() {
            if let Ok(mut writer) = transcript.lock() {
                writer.close();
            }
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    fn evict_idle(&mut self) {
        if self.idle_seconds == 0 {
            return;
        }
        let idle_seconds = self.idle_seconds;
        let expired: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.last_activity_at.elapsed().as_secs() >= idle_seconds)
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            if let Some(mut entry) = self.entries.remove(&key) {
                entry.session.close();
            }
            self.steer_handles.remove(&key);
        }
    }

    fn evict_oldest(&mut self) {
        let Some(key) = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_activity_at)
            .map(|(key, _)| key.clone())
        else {
            return;
        };
        if let Some(mut entry) = self.entries.remove(&key) {
            entry.session.close();
        }
        self.steer_handles.remove(&key);
    }
}

impl GatewayStatus {
    pub fn current() -> Self {
        let home = colibri_home();
        let state_path = home.join("run/gateway.json");
        let log_path = home.join("logs/gateway.log");
        Self::from_paths(state_path, log_path)
    }

    pub fn from_paths(state_path: PathBuf, default_log_path: PathBuf) -> Self {
        if !state_path.is_file() {
            return Self {
                running: false,
                pid: None,
                rss_kb: None,
                config_path: "default".to_string(),
                cwd: String::new(),
                log_path: default_log_path,
                state_path,
                started_at: String::new(),
                reason: "state_missing".to_string(),
            };
        }
        let text = fs::read_to_string(&state_path).unwrap_or_default();
        let pid = json_field(&text, "pid");
        let running = pid
            .as_deref()
            .and_then(|value| value.parse::<u32>().ok())
            .is_some_and(pid_running);
        let reason = if running { "" } else { "not_running" };
        let rss_kb = pid
            .as_deref()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|_| running)
            .and_then(|pid| process_rss_kb(Some(pid)))
            .map(|value| value.to_string());
        Self {
            running,
            pid,
            rss_kb,
            config_path: json_field(&text, "config").unwrap_or_else(|| {
                json_field(&text, "config_path").unwrap_or_else(|| "default".to_string())
            }),
            cwd: json_field(&text, "cwd").unwrap_or_default(),
            log_path: json_field(&text, "log")
                .map(PathBuf::from)
                .unwrap_or(default_log_path),
            state_path,
            started_at: json_field(&text, "started_at").unwrap_or_default(),
            reason: reason.to_string(),
        }
    }
}

pub fn start_gateway(config_path: Option<PathBuf>) -> Result<GatewayStatus, String> {
    let status = GatewayStatus::current();
    if status.running {
        return Ok(status);
    }
    let home = colibri_home();
    let run_dir = home.join("run");
    let log_dir = home.join("logs");
    fs::create_dir_all(&run_dir).map_err(|error| error.to_string())?;
    fs::create_dir_all(&log_dir).map_err(|error| error.to_string())?;
    let state_path = run_dir.join("gateway.json");
    let log_path = log_dir.join("gateway.log");
    let current_exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut command = Command::new(&current_exe);
    if let Some(path) = &config_path {
        command.arg("--config").arg(path);
    }
    command.arg("gateway").arg("run");
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| error.to_string())?;
    let process = command
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|error| error.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|error| error.to_string())?;
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let started_at = beijing_timestamp_now();
    let state = format!(
        "{{\"pid\":{},\"config\":\"{}\",\"cwd\":\"{}\",\"log\":\"{}\",\"started_at\":\"{}\"}}\n",
        process.id(),
        config_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "default".to_string()),
        cwd.display(),
        log_path.display(),
        started_at
    );
    fs::write(&state_path, state).map_err(|error| error.to_string())?;
    Ok(GatewayStatus::from_paths(state_path, log_path))
}

pub fn stop_gateway() -> Result<GatewayStatus, String> {
    let status = GatewayStatus::current();
    if let Some(pid) = status
        .pid
        .as_deref()
        .and_then(|value| value.parse::<u32>().ok())
    {
        if status.running {
            if !pid_matches_gateway(pid) {
                let mut status = status;
                status.reason = "unverified_pid".to_string();
                return Ok(status);
            }
            let _ = Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .status();
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && pid_running(pid) {
                std::thread::sleep(Duration::from_millis(100));
            }
            if pid_running(pid) {
                let _ = Command::new("kill")
                    .arg("-KILL")
                    .arg(pid.to_string())
                    .status();
            }
        }
    }
    Ok(GatewayStatus::current())
}

pub fn restart_gateway(config_path: Option<PathBuf>) -> Result<GatewayStatus, String> {
    let _ = stop_gateway();
    start_gateway(config_path)
}

pub fn format_gateway_status(status: &GatewayStatus) -> Vec<String> {
    let mut lines = vec![
        format!("running={}", status.running),
        format!("pid={}", status.pid.as_deref().unwrap_or("unknown")),
        format!("rss_kb={}", status.rss_kb.as_deref().unwrap_or("unknown")),
        format!("config={}", status.config_path),
        format!(
            "cwd={}",
            if status.cwd.is_empty() {
                "unknown"
            } else {
                &status.cwd
            }
        ),
        format!("log={}", status.log_path.display()),
        format!("state={}", status.state_path.display()),
    ];
    if !status.started_at.is_empty() {
        lines.push(format!("started_at={}", status.started_at));
    }
    if !status.reason.is_empty() {
        lines.push(format!("reason={}", status.reason));
    }
    lines
}

fn json_field(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let start = text.find(&needle)?;
    let after = &text[start + needle.len()..];
    let colon = after.find(':')?;
    let value = after[colon + 1..].trim_start();
    if let Some(stripped) = value.strip_prefix('"') {
        return stripped.split('"').next().map(|item| item.to_string());
    }
    Some(
        value
            .split(|ch: char| ch == ',' || ch == '}' || ch.is_whitespace())
            .next()
            .unwrap_or("")
            .to_string(),
    )
}

fn pid_running(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn pid_matches_gateway(pid: u32) -> bool {
    let Some(command) = pid_command(pid) else {
        return false;
    };
    let binary = std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "colibri".to_string());
    (command.contains(&binary) || command.contains("colibri"))
        && command.contains("gateway")
        && command.contains("run")
}

fn pid_command(pid: u32) -> Option<String> {
    let proc_path = PathBuf::from("/proc").join(pid.to_string()).join("cmdline");
    if let Ok(bytes) = fs::read(proc_path) {
        let text = String::from_utf8_lossy(&bytes).replace('\0', " ");
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    let output = Command::new("ps")
        .arg("-o")
        .arg("command=")
        .arg("-p")
        .arg(pid.to_string())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}
