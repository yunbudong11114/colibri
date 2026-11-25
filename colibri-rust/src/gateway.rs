use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::channel::{
    validate_channel_envelope, ChannelPollError, ChannelRegistry, GatewayChannel, InboundEnvelope,
};
use crate::channel_registry::build_enabled_channels;
use crate::config::{colibri_home, rss_kb as process_rss_kb, AgentConfig};
use crate::messages::MediaPart;
use crate::model::{build_model, ModelClient};
use crate::runtime_reload::{PartialRuntimeReloader, RuntimeReloadResult, RuntimeSnapshot};
use crate::session::AgentSession;
use crate::session_history::TranscriptHistoryLoader;
use crate::steering::SteerHandle;
use crate::transcript::{beijing_timestamp_now, TranscriptWriter};

#[derive(Clone, Debug)]
pub struct GatewayStatus {
    pub running: bool,
    pub agent_status: String,
    pub channel_status: BTreeMap<String, String>,
    pub pid: Option<String>,
    pub rss_kb: Option<String>,
    pub config_path: String,
    pub cwd: String,
    pub log_path: PathBuf,
    pub state_path: PathBuf,
    pub started_at: String,
    pub reason: String,
}

pub struct GatewayAgentHealth {
    status: Mutex<String>,
    reporter: Arc<dyn Fn(&str) + Send + Sync>,
}

impl Default for GatewayAgentHealth {
    fn default() -> Self {
        Self {
            status: Mutex::new("healthy".to_string()),
            reporter: Arc::new(update_gateway_agent_status),
        }
    }
}

impl GatewayAgentHealth {
    pub fn with_reporter(reporter: Arc<dyn Fn(&str) + Send + Sync>) -> Self {
        Self {
            status: Mutex::new("healthy".to_string()),
            reporter,
        }
    }

    pub fn report(&self, status: &str) {
        if !matches!(status, "healthy" | "unhealthy") {
            return;
        }
        let Ok(mut current) = self.status.lock() else {
            return;
        };
        if current.as_str() == status {
            return;
        }
        (self.reporter)(status);
        *current = status.to_string();
    }
}

pub fn format_gateway_log(message: &str) -> String {
    format!("[{}] [gateway] {}", beijing_timestamp_now(), message)
}

fn gateway_log(message: impl AsRef<str>) {
    eprintln!("{}", format_gateway_log(message.as_ref()));
}

#[derive(Default)]
struct ChannelHealth(Mutex<BTreeMap<String, &'static str>>);

impl ChannelHealth {
    fn report(&self, channel: &str, status: &'static str) {
        let Ok(mut states) = self.0.lock() else {
            return;
        };
        if states.get(channel) == Some(&status) {
            return;
        }
        states.insert(channel.into(), status);
        gateway_log(format!("channel={channel} transport={status}"));
        let path = colibri_home().join("run/gateway-channels.json");
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let data = serde_json::json!({"pid":std::process::id(),"channels":&*states}).to_string();
        #[cfg(unix)]
        let _ = crate::config_io::atomic_write(&path, data.as_bytes());
        #[cfg(not(unix))]
        let _ = fs::write(path, data);
    }
}

fn read_channel_health(state_path: &Path, pid: Option<&str>) -> BTreeMap<String, String> {
    let read = || -> Option<_> {
        let pid: u64 = pid?.parse().ok()?;
        let text = fs::read_to_string(state_path.with_file_name("gateway-channels.json")).ok()?;
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        if value["pid"].as_u64() != Some(pid) {
            return None;
        }
        serde_json::from_value(value["channels"].clone()).ok()
    };
    read().unwrap_or_default()
}

pub struct GatewaySessionCache {
    config: Arc<AgentConfig>,
    model: Arc<Mutex<Box<dyn ModelClient>>>,
    transcript: Option<Arc<Mutex<TranscriptWriter>>>,
    max_sessions: usize,
    idle_seconds: u64,
    entries: BTreeMap<String, GatewaySessionEntry>,
    /// Cloned independently of session ownership so receive can steer while
    /// the worker holds the session outside this cache mutex during submit.
    steer_handles: BTreeMap<String, SteerHandle>,
    runtime_reloader: PartialRuntimeReloader,
}

struct GatewaySessionEntry {
    session: AgentSession,
    last_activity_at: Instant,
}

impl GatewaySessionCache {
    pub fn new(config: AgentConfig) -> Result<Self, String> {
        Self::new_with_config_path(config, None)
    }

    pub fn new_with_config_path(
        config: AgentConfig,
        config_path: Option<PathBuf>,
    ) -> Result<Self, String> {
        let active_path = config_path.unwrap_or_else(crate::config::default_config_path);
        let config = Arc::new(config);
        let model = Arc::new(Mutex::new(build_model(&config.model)?));
        let runtime_reloader = PartialRuntimeReloader::new(
            active_path,
            RuntimeSnapshot {
                config: Arc::clone(&config),
                model: Arc::clone(&model),
            },
        );
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
        Ok(Self {
            max_sessions: config.gateway.max_sessions.max(1),
            idle_seconds: config.gateway.session_idle_seconds,
            config,
            model,
            transcript,
            entries: BTreeMap::new(),
            steer_handles: BTreeMap::new(),
            runtime_reloader,
        })
    }

    pub fn reload_if_changed(&mut self) {
        let snapshot = match self.runtime_reloader.reload_if_changed() {
            RuntimeReloadResult::Unchanged => return,
            RuntimeReloadResult::Rejected(error) => {
                gateway_log(format!("config reload skipped: {}", error));
                return;
            }
            RuntimeReloadResult::Reloaded(snapshot) => snapshot,
        };
        self.config = Arc::clone(&snapshot.config);
        self.model = Arc::clone(&snapshot.model);
        for entry in self.entries.values_mut() {
            entry
                .session
                .adopt_runtime(Arc::clone(&snapshot.config), Arc::clone(&snapshot.model));
        }
        gateway_log(format!(
            "config reloaded model={}",
            snapshot.config.model.model
        ));
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
        let mut metadata = metadata;
        metadata
            .entry("session_key".to_string())
            .or_insert_with(|| key.to_string());
        let mut session = AgentSession::from_shared(
            Arc::clone(&self.config),
            Arc::clone(&self.model),
            self.transcript.as_ref().map(Arc::clone),
            metadata,
        );
        if self.config.session.restore_transcript {
            let policy = self.config.session.clone();
            let source = key.to_string();
            session = session.with_model_history_loader(Box::new(move || {
                TranscriptHistoryLoader::default(&policy).load_model(&source)
            }));
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
            if !Arc::ptr_eq(&entry.session.config, &self.config) {
                entry
                    .session
                    .adopt_runtime(Arc::clone(&self.config), Arc::clone(&self.model));
            }
            entry.session.set_media_sender(media_sender);
            self.steer_handles
                .insert(key.to_string(), entry.session.steer_handle());
            return Ok(entry.session);
        }
        while self.entries.len() >= self.max_sessions {
            self.evict_oldest();
        }
        let mut metadata = metadata;
        metadata
            .entry("session_key".to_string())
            .or_insert_with(|| key.to_string());
        let mut session = AgentSession::from_shared(
            Arc::clone(&self.config),
            Arc::clone(&self.model),
            self.transcript.as_ref().map(Arc::clone),
            metadata,
        );
        if self.config.session.restore_transcript {
            let policy = self.config.session.clone();
            let source = key.to_string();
            session = session.with_model_history_loader(Box::new(move || {
                TranscriptHistoryLoader::default(&policy).load_model(&source)
            }));
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
                channel_status: BTreeMap::new(),
                agent_status: "unhealthy".to_string(),
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
        let channel_status = read_channel_health(&state_path, pid.as_deref());
        Self {
            running,
            channel_status,
            agent_status: if running {
                json_field(&text, "agent_status").unwrap_or_else(|| "healthy".to_string())
            } else {
                "unhealthy".to_string()
            },
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
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
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
    let mut process = command
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|error| error.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|error| error.to_string())?;
    let started_at = beijing_timestamp_now();
    let state = format!(
        "{{\"pid\":{},\"agent_status\":\"healthy\",\"config\":\"{}\",\"cwd\":\"{}\",\"log\":\"{}\",\"started_at\":\"{}\"}}\n",
        process.id(),
        config_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "default".to_string()),
        cwd.display(),
        log_path.display(),
        started_at
    );
    if let Err(error) = fs::write(&state_path, state) {
        let _ = process.kill();
        let _ = process.wait();
        return Err(error.to_string());
    }
    let deadline = Instant::now() + Duration::from_millis(250);
    loop {
        if process
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            let mut status = GatewayStatus::from_paths(state_path, log_path);
            status.running = false;
            status.agent_status = "unhealthy".to_string();
            status.rss_kb = None;
            status.reason = "startup_exited".to_string();
            return Ok(status);
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    Ok(GatewayStatus::from_paths(state_path, log_path))
}

pub fn stop_gateway() -> Result<GatewayStatus, String> {
    let status = GatewayStatus::current();
    let Some(pid) = status
        .pid
        .as_deref()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|_| status.running)
    else {
        return Ok(status);
    };
    let mut failure = "";
    for (signal, timeout) in [("-TERM", 5), ("-KILL", 2)] {
        if !pid_running(pid) {
            break;
        }
        if !pid_matches_gateway(pid) {
            failure = "unverified_pid";
            break;
        }
        let sent = Command::new("kill")
            .arg(signal)
            .arg(pid.to_string())
            .status()
            .is_ok_and(|result| result.success());
        if !sent {
            failure = "signal_failed";
            break;
        }
        let deadline = Instant::now() + Duration::from_secs(timeout);
        while Instant::now() < deadline && pid_running(pid) {
            thread::sleep(Duration::from_millis(100));
        }
    }
    let mut status = GatewayStatus::current();
    if status.running {
        status.reason = if failure.is_empty() {
            "stop_timeout"
        } else {
            failure
        }
        .to_string();
    }
    Ok(status)
}

pub fn restart_gateway(config_path: Option<PathBuf>) -> Result<GatewayStatus, String> {
    let stopped = stop_gateway()?;
    if stopped.running {
        return Ok(stopped);
    }
    start_gateway(config_path)
}

pub fn format_gateway_status(status: &GatewayStatus) -> Vec<String> {
    let mut lines = vec![
        format!("running={}", status.running),
        format!("agent_status={}", status.agent_status),
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
    for (name, mode) in &status.channel_status {
        lines.push(format!("channel.{name}={mode}"));
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
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    #[cfg(target_os = "linux")]
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => {
            if let Some(state) = stat
                .rsplit_once(") ")
                .and_then(|(_, tail)| tail.chars().next())
            {
                return !matches!(state, 'Z' | 'X' | 'x');
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => {}
    }
    if let Ok(output) = Command::new("ps")
        .args(["-o", "stat=", "-p"])
        .arg(pid.to_string())
        .output()
    {
        let state = String::from_utf8_lossy(&output.stdout);
        let state = state.trim();
        if let Some(state) = state.chars().next() {
            return !matches!(state, 'Z' | 'X' | 'x');
        }
    }
    #[cfg(unix)]
    {
        // A process we cannot signal is still live.
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    false
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
    let Some(args) = command.strip_suffix(" gateway run") else {
        return false;
    };
    let executable = args.split_once(" --config ").map_or(args, |(exe, _)| exe);
    Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        == Some(binary.as_str())
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

/// Per-session inbound queues with a global pending bound and fair RR acquire.
pub struct InboundRouter<T> {
    inner: Mutex<InboundRouterInner<T>>,
    cv: Condvar,
}

struct InboundRouterInner<T> {
    max_pending: usize,
    queues: BTreeMap<String, VecDeque<T>>,
    rr: VecDeque<String>,
    total: usize,
    active: HashSet<String>,
    closed: bool,
}

impl<T> InboundRouter<T> {
    pub fn new(max_pending: usize) -> Self {
        Self {
            inner: Mutex::new(InboundRouterInner {
                max_pending: max_pending.max(1),
                queues: BTreeMap::new(),
                rr: VecDeque::new(),
                total: 0,
                active: HashSet::new(),
                closed: false,
            }),
            cv: Condvar::new(),
        }
    }

    /// Enqueue or return the item if the global bound is hit (fail-fast).
    pub fn try_enqueue(&self, key: String, item: T) -> Result<(), T> {
        let mut guard = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if guard.closed || guard.total >= guard.max_pending {
            return Err(item);
        }
        let queue = guard.queues.entry(key.clone()).or_default();
        let was_empty = queue.is_empty();
        queue.push_back(item);
        guard.total += 1;
        if was_empty && !guard.active.contains(&key) && !guard.rr.iter().any(|item| item == &key) {
            guard.rr.push_back(key);
        }
        self.cv.notify_one();
        Ok(())
    }

    /// Block until a session that is not already mid-turn has work, or the router closes.
    pub fn acquire(&self) -> Option<(String, T)> {
        let mut guard = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        loop {
            if let Some(pair) = Self::try_acquire_locked(&mut guard) {
                return Some(pair);
            }
            if guard.closed {
                return None;
            }
            guard = self
                .cv
                .wait(guard)
                .unwrap_or_else(|error| error.into_inner());
        }
    }

    fn try_acquire_locked(guard: &mut InboundRouterInner<T>) -> Option<(String, T)> {
        let n = guard.rr.len();
        for _ in 0..n {
            let key = guard.rr.pop_front()?;
            if guard.active.contains(&key) {
                guard.rr.push_back(key);
                continue;
            }
            let Some(queue) = guard.queues.get_mut(&key) else {
                continue;
            };
            let Some(item) = queue.pop_front() else {
                continue;
            };
            guard.total = guard.total.saturating_sub(1);
            guard.active.insert(key.clone());
            if !queue.is_empty() {
                guard.rr.push_back(key.clone());
            }
            return Some((key, item));
        }
        None
    }

    pub fn release(&self, key: &str) {
        let mut guard = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        guard.active.remove(key);
        let has_work = guard
            .queues
            .get(key)
            .map(|queue| !queue.is_empty())
            .unwrap_or(false);
        if has_work && !guard.rr.iter().any(|item| item == key) {
            guard.rr.push_back(key.to_string());
        }
        self.cv.notify_all();
    }

    pub fn close(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        guard.closed = true;
        self.cv.notify_all();
    }

    fn closed_after(&self, delay: Duration) -> bool {
        let guard = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if delay.is_zero() {
            return guard.closed;
        }
        let (guard, _) = self
            .cv
            .wait_timeout_while(guard, delay, |state| !state.closed)
            .unwrap_or_else(|error| error.into_inner());
        guard.closed
    }

    pub fn pending_len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .total
    }

    pub fn active_len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .active
            .len()
    }

    pub fn wait_idle(&self, timeout: Option<Duration>) -> bool {
        let mut guard = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let deadline = timeout.map(|duration| Instant::now() + duration);
        loop {
            if guard.total == 0 && guard.active.is_empty() {
                return true;
            }
            let Some(deadline) = deadline else {
                guard = self
                    .cv
                    .wait(guard)
                    .unwrap_or_else(|error| error.into_inner());
                continue;
            };
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next_guard, wait_result) = self
                .cv
                .wait_timeout(guard, remaining)
                .unwrap_or_else(|error| error.into_inner());
            guard = next_guard;
            if wait_result.timed_out() && (guard.total > 0 || !guard.active.is_empty()) {
                return false;
            }
        }
    }
}

/// Foreground gateway: enabled channel pollers → inbound router → turn workers.
pub fn run_gateway(config: AgentConfig, config_path: Option<PathBuf>) -> Result<i32, String> {
    gateway_log(format!(
        "started pid={} model={}",
        std::process::id(),
        config.model.model
    ));
    let channels = Arc::new(build_enabled_channels(&config)?);
    if channels.is_empty() {
        return Err("No gateway channels are enabled".to_string());
    }
    let max_pending = config.gateway.max_pending_inbound.max(1);
    let max_turns = config.gateway.max_concurrent_turns.max(1);
    let router = Arc::new(InboundRouter::new(max_pending));
    let (error_tx, error_rx) = mpsc::channel::<String>();
    let sessions = Arc::new(Mutex::new(GatewaySessionCache::new_with_config_path(
        config.clone(),
        config_path,
    )?));
    let health = Arc::new(GatewayAgentHealth::default());
    let mut workers = Vec::with_capacity(max_turns);
    for _ in 0..max_turns {
        let worker_router = Arc::clone(&router);
        let worker_channels = Arc::clone(&channels);
        let worker_sessions = Arc::clone(&sessions);
        let worker_health = Arc::clone(&health);
        let worker_error_tx = error_tx.clone();
        workers.push(thread::spawn(move || {
            if let Err(error) = run_turn_worker(
                worker_router,
                worker_channels,
                worker_sessions,
                worker_health,
            ) {
                gateway_log(format!("turn worker failed: {}", error));
                let _ = worker_error_tx.send(error);
            }
        }));
    }
    let channel_health = Arc::new(ChannelHealth::default());
    let mut pollers = Vec::new();
    for channel in channels.values().cloned() {
        let channel_name = channel.name().to_string();
        let poll_router = Arc::clone(&router);
        let poll_sessions = Arc::clone(&sessions);
        let channel_health = Arc::clone(&channel_health);
        pollers.push(thread::spawn(move || {
            if let Err(error) =
                run_channel_poll_loop(channel, poll_router, poll_sessions, |status| {
                    channel_health.report(&channel_name, status);
                })
            {
                gateway_log(format!("channel={channel_name} poller stopped: {error}"));
            }
        }));
    }
    let result = (|| -> Result<i32, String> {
        loop {
            if let Ok(error) = error_rx.try_recv() {
                return Err(error);
            }
            thread::sleep(Duration::from_millis(50));
            if pollers.iter().all(|poller| poller.is_finished()) {
                let error = "gateway channel poller stopped".to_string();
                return Err(error);
            }
        }
    })();
    router.close();
    for worker in workers {
        let _ = worker.join();
    }
    for poller in pollers {
        let _ = poller.join();
    }
    if let Ok(mut cache) = sessions.lock() {
        cache.close();
    }
    result
}

fn run_channel_poll_loop(
    channel: Arc<dyn GatewayChannel>,
    router: Arc<InboundRouter<InboundEnvelope>>,
    sessions: Arc<Mutex<GatewaySessionCache>>,
    report: impl Fn(&'static str),
) -> Result<(), String> {
    let initial_delay = Duration::from_secs(1);
    let mut backoff = initial_delay;
    report("connecting");
    while !router.closed_after(Duration::ZERO) {
        let messages = match channel.poll_once() {
            Ok(messages) => {
                backoff = initial_delay;
                report("online");
                messages
            }
            Err(ChannelPollError::Retryable(error)) => {
                if backoff == initial_delay {
                    gateway_log(format!("channel={} reconnecting: {error}", channel.name()));
                }
                report("reconnecting");
                if router.closed_after(backoff) {
                    break;
                }
                backoff = (backoff * 2).min(Duration::from_secs(30));
                continue;
            }
            Err(ChannelPollError::AuthRequired) => {
                report("auth_required");
                return Err("channel authentication required".into());
            }
            Err(ChannelPollError::Permanent(error)) => {
                report("failed");
                return Err(error);
            }
        };
        for envelope in messages {
            validate_channel_envelope(channel.as_ref(), &envelope).map_err(|error| {
                report("failed");
                error
            })?;
            let key = envelope.session_key();
            if envelope.text_only() {
                let steered = sessions
                    .lock()
                    .map_err(|_| "gateway session cache lock poisoned".to_string())?
                    .try_steer(&key, &envelope.text);
                if steered {
                    continue;
                }
            }
            if router.try_enqueue(key, envelope).is_err() {
                gateway_log("inbound queue full; dropping message");
            }
        }
    }
    Ok(())
}

fn run_turn_worker(
    router: Arc<InboundRouter<InboundEnvelope>>,
    channels: Arc<ChannelRegistry>,
    sessions: Arc<Mutex<GatewaySessionCache>>,
    health: Arc<GatewayAgentHealth>,
) -> Result<(), String> {
    while let Some((key, mut envelope)) = router.acquire() {
        let turn_result = (|| -> Result<(), String> {
            let channel = channels
                .get(&envelope.channel)
                .cloned()
                .ok_or_else(|| format!("unknown gateway channel: {}", envelope.channel))?;
            channel.resolve_inbound_media(&mut envelope)?;
            let outbound = channel.outbound_for(&envelope)?;
            let media_outbound = Arc::clone(&outbound);
            let media_sender = Arc::new(move |media| media_outbound.send_media(&media));
            let mut session = {
                let mut guard = sessions
                    .lock()
                    .map_err(|_| "gateway session cache lock poisoned".to_string())?;
                guard.reload_if_changed();
                guard.take_or_create_with_metadata_and_media_sender(
                    &key,
                    BTreeMap::from([
                        ("channel".to_string(), envelope.channel.clone()),
                        ("sender_id".to_string(), envelope.sender_id.clone()),
                        ("session_key".to_string(), key.clone()),
                    ]),
                    Some(media_sender),
                )?
            };
            let ack_outbound = Arc::clone(&outbound);
            session.set_steer_notifier(Some(Arc::new(move |ack| {
                ack_outbound.send_ack(&ack);
            })));
            let response_result = session.submit_with_media(&envelope.text, envelope.media);
            {
                let mut guard = sessions
                    .lock()
                    .map_err(|_| "gateway session cache lock poisoned".to_string())?;
                guard.put_back(&key, session);
                guard.touch(&key);
            }
            let response = match response_result {
                Ok(response) => response,
                Err(error) => {
                    health.report("unhealthy");
                    return Err(error);
                }
            };
            health.report(if response.error_type.is_some() {
                "unhealthy"
            } else {
                "healthy"
            });
            if !response.text.trim().is_empty() {
                outbound.send_text(&response.text)?;
            }
            Ok(())
        })();
        router.release(&key);
        turn_result?;
    }
    Ok(())
}

fn update_gateway_agent_status(status: &str) {
    if !matches!(status, "healthy" | "unhealthy") {
        return;
    }
    let state_path = colibri_home().join("run/gateway.json");
    let Ok(text) = fs::read_to_string(&state_path) else {
        return;
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    if value.get("pid").and_then(serde_json::Value::as_u64) != Some(std::process::id() as u64) {
        return;
    }
    let Some(object) = value.as_object_mut() else {
        return;
    };
    object.insert(
        "agent_status".to_string(),
        serde_json::Value::String(status.to_string()),
    );
    let temporary = state_path.with_extension("json.tmp");
    if fs::write(&temporary, format!("{}\n", value)).is_ok() {
        let _ = fs::rename(temporary, state_path);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{
        run_channel_poll_loop, run_turn_worker, GatewayAgentHealth, GatewaySessionCache,
        InboundRouter,
    };
    use crate::channel::{
        build_channel_registry, ChannelPollError, GatewayChannel, InboundEnvelope, OutboundSink,
    };
    use crate::config::AgentConfig;
    use crate::messages::{MediaPart, Message, ModelLimits, ModelResponse};
    use crate::model::ModelClient;
    use crate::session::MODEL_UNAVAILABLE_TEXT;

    struct ReconnectingChannel(AtomicUsize);

    impl GatewayChannel for ReconnectingChannel {
        fn name(&self) -> &str {
            "fixture"
        }

        fn poll_once(&self) -> Result<Vec<InboundEnvelope>, ChannelPollError> {
            match self.0.fetch_add(1, Ordering::SeqCst) {
                0 => Err(ChannelPollError::Retryable("connection reset".into())),
                1 => Ok(Vec::new()),
                _ => Err(ChannelPollError::AuthRequired),
            }
        }

        fn resolve_inbound_media(&self, _: &mut InboundEnvelope) -> Result<(), String> {
            Ok(())
        }

        fn outbound_for(&self, _: &InboundEnvelope) -> Result<Arc<dyn OutboundSink>, String> {
            Err("unused".into())
        }
    }

    #[test]
    fn generic_poller_recovers_from_transport_error_and_stops_on_auth_error() {
        let states = Mutex::new(Vec::new());
        let channel = Arc::new(ReconnectingChannel(AtomicUsize::new(0)));
        let mut config = AgentConfig::default();
        config.session.transcript = false;
        let gateway = GatewaySessionCache::new(config).unwrap();
        let error = run_channel_poll_loop(
            channel,
            Arc::new(InboundRouter::new(1)),
            Arc::new(Mutex::new(gateway)),
            |status| states.lock().unwrap().push(status),
        )
        .unwrap_err();
        assert_eq!(error, "channel authentication required");
        assert_eq!(
            *states.lock().unwrap(),
            ["connecting", "reconnecting", "online", "auth_required"]
        );
    }

    struct RecordingSink {
        texts: Arc<Mutex<Vec<String>>>,
    }

    impl OutboundSink for RecordingSink {
        fn send_text(&self, text: &str) -> Result<(), String> {
            self.texts.lock().unwrap().push(text.to_string());
            Ok(())
        }

        fn send_media(&self, _media: &MediaPart) -> Result<(), String> {
            Ok(())
        }
    }

    struct FakeChannel {
        texts: Arc<Mutex<Vec<String>>>,
    }

    struct MismatchedChannel {
        polled: AtomicBool,
    }

    struct FailOnceModel {
        calls: usize,
    }

    #[test]
    fn agent_health_persists_only_on_state_change() {
        let writes = Arc::new(AtomicUsize::new(0));
        let reporter_writes = Arc::clone(&writes);
        let health = GatewayAgentHealth::with_reporter(Arc::new(move |_| {
            reporter_writes.fetch_add(1, Ordering::SeqCst);
        }));

        health.report("healthy");
        health.report("unhealthy");
        health.report("unhealthy");
        health.report("healthy");

        assert_eq!(writes.load(Ordering::SeqCst), 2);
    }

    impl ModelClient for FailOnceModel {
        fn complete(
            &mut self,
            _messages: &[Message],
            _tools: &[serde_json::Value],
            _system: &str,
            _limits: &ModelLimits,
        ) -> Result<ModelResponse, String> {
            self.calls += 1;
            if self.calls == 1 {
                return Err("model_error:transient_network:model failed".to_string());
            }
            Ok(ModelResponse {
                text: "recovered".to_string(),
                tool_calls: Vec::new(),
                ..ModelResponse::default()
            })
        }
    }

    impl GatewayChannel for MismatchedChannel {
        fn name(&self) -> &str {
            "expected"
        }

        fn poll_once(&self) -> Result<Vec<InboundEnvelope>, ChannelPollError> {
            if self.polled.swap(true, Ordering::SeqCst) {
                return Err(ChannelPollError::Permanent("poll stopped".into()));
            }
            Ok(vec![InboundEnvelope {
                channel: "wrong".to_string(),
                sender_id: "user-1".to_string(),
                text: "hello".to_string(),
                message_id: "message-1".to_string(),
                media: Vec::new(),
                media_refs: Vec::new(),
                context: BTreeMap::new(),
            }])
        }

        fn resolve_inbound_media(&self, _envelope: &mut InboundEnvelope) -> Result<(), String> {
            Ok(())
        }

        fn outbound_for(
            &self,
            _envelope: &InboundEnvelope,
        ) -> Result<Arc<dyn OutboundSink>, String> {
            Err("unused".to_string())
        }
    }

    impl GatewayChannel for FakeChannel {
        fn name(&self) -> &str {
            "other"
        }

        fn poll_once(&self) -> Result<Vec<InboundEnvelope>, ChannelPollError> {
            Ok(Vec::new())
        }

        fn resolve_inbound_media(&self, envelope: &mut InboundEnvelope) -> Result<(), String> {
            envelope
                .context
                .insert("resolved".to_string(), "yes".to_string());
            Ok(())
        }

        fn outbound_for(
            &self,
            _envelope: &InboundEnvelope,
        ) -> Result<Arc<dyn OutboundSink>, String> {
            Ok(Arc::new(RecordingSink {
                texts: Arc::clone(&self.texts),
            }))
        }
    }

    #[test]
    fn inbound_router_global_bound_and_fair_acquire() {
        let router = InboundRouter::new(2);
        assert!(router.try_enqueue("a".into(), 1).is_ok());
        assert!(router.try_enqueue("b".into(), 2).is_ok());
        assert!(router.try_enqueue("a".into(), 3).is_err());
        assert_eq!(router.pending_len(), 2);

        let (key, value) = router.acquire().unwrap();
        assert_eq!((key.as_str(), value), ("a", 1));
        assert!(router.try_enqueue("a".into(), 3).is_ok());
        router.release("a");

        let mut got = vec![router.acquire().unwrap(), router.acquire().unwrap()];
        got.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(got, vec![("a".into(), 3), ("b".into(), 2)]);
        router.release("a");
        router.release("b");
    }

    #[test]
    fn inbound_router_same_session_serialized() {
        let router = InboundRouter::new(4);
        assert!(router.try_enqueue("a".into(), 1).is_ok());
        assert!(router.try_enqueue("a".into(), 2).is_ok());
        let (key, value) = router.acquire().unwrap();
        assert_eq!((key.as_str(), value), ("a", 1));
        assert_eq!(router.pending_len(), 1);
        router.release("a");
        let (key, value) = router.acquire().unwrap();
        assert_eq!((key.as_str(), value), ("a", 2));
        router.release("a");
    }

    #[test]
    fn generic_turn_worker_dispatches_fake_channel_without_transport_branch() {
        let texts = Arc::new(Mutex::new(Vec::new()));
        let channel: Arc<dyn GatewayChannel> = Arc::new(FakeChannel {
            texts: Arc::clone(&texts),
        });
        let channels = Arc::new(build_channel_registry(vec![channel]).unwrap());
        let router = Arc::new(InboundRouter::new(1));
        router
            .try_enqueue(
                "other:user-1".to_string(),
                InboundEnvelope {
                    channel: "other".to_string(),
                    sender_id: "user-1".to_string(),
                    text: "hello".to_string(),
                    message_id: "message-1".to_string(),
                    media: Vec::new(),
                    media_refs: Vec::new(),
                    context: BTreeMap::new(),
                },
            )
            .unwrap();
        router.close();
        let mut config = AgentConfig::default();
        config.session.transcript = false;
        config.session.restore_transcript = false;
        let sessions = Arc::new(Mutex::new(GatewaySessionCache::new(config).unwrap()));

        run_turn_worker(
            router,
            channels,
            sessions,
            Arc::new(GatewayAgentHealth::default()),
        )
        .unwrap();

        assert_eq!(*texts.lock().unwrap(), vec!["fake: hello"]);
    }

    #[test]
    fn generic_poll_loop_rejects_envelope_from_wrong_channel() {
        let channel: Arc<dyn GatewayChannel> = Arc::new(MismatchedChannel {
            polled: AtomicBool::new(false),
        });
        let router = Arc::new(InboundRouter::new(1));
        let mut config = AgentConfig::default();
        config.session.transcript = false;
        config.session.restore_transcript = false;
        let sessions = Arc::new(Mutex::new(GatewaySessionCache::new(config).unwrap()));

        let error = run_channel_poll_loop(channel, router, sessions, |_| {}).unwrap_err();

        assert_eq!(
            error,
            "channel adapter mismatch: expected expected, got wrong"
        );
    }

    #[test]
    fn generic_turn_worker_continues_after_model_error() {
        let texts = Arc::new(Mutex::new(Vec::new()));
        let channel: Arc<dyn GatewayChannel> = Arc::new(FakeChannel {
            texts: Arc::clone(&texts),
        });
        let channels = Arc::new(build_channel_registry(vec![channel]).unwrap());
        let router = Arc::new(InboundRouter::new(2));
        router
            .try_enqueue(
                "other:user-1".to_string(),
                InboundEnvelope {
                    channel: "other".to_string(),
                    sender_id: "user-1".to_string(),
                    text: "hello".to_string(),
                    message_id: "message-1".to_string(),
                    media: Vec::new(),
                    media_refs: Vec::new(),
                    context: BTreeMap::new(),
                },
            )
            .unwrap();
        router
            .try_enqueue(
                "other:user-1".to_string(),
                InboundEnvelope {
                    channel: "other".to_string(),
                    sender_id: "user-1".to_string(),
                    text: "again".to_string(),
                    message_id: "message-2".to_string(),
                    media: Vec::new(),
                    media_refs: Vec::new(),
                    context: BTreeMap::new(),
                },
            )
            .unwrap();
        router.close();
        let mut config = AgentConfig::default();
        config.session.transcript = false;
        config.session.restore_transcript = false;
        let mut cache = GatewaySessionCache::new(config).unwrap();
        cache.model = Arc::new(Mutex::new(Box::new(FailOnceModel { calls: 0 })));
        let sessions = Arc::new(Mutex::new(cache));

        run_turn_worker(
            router,
            channels,
            Arc::clone(&sessions),
            Arc::new(GatewayAgentHealth::default()),
        )
        .unwrap();

        assert!(sessions
            .lock()
            .unwrap()
            .entries
            .contains_key("other:user-1"));
        assert_eq!(
            *texts.lock().unwrap(),
            vec![MODEL_UNAVAILABLE_TEXT.to_string(), "recovered".to_string()]
        );
    }
}

#[cfg(all(test, target_os = "linux"))]
#[test]
fn zombie_gateway_is_reported_as_stopped() {
    let mut child = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
    let pid = child.id();
    let deadline = Instant::now() + Duration::from_secs(2);
    while pid_running(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let running = pid_running(pid);
    child.wait().unwrap();
    assert!(!running);
}
