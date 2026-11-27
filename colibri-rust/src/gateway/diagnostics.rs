//! Metadata-only, in-memory diagnostics. Normal polling never writes a heartbeat.
use super::*;
use crate::channel::OutboundSink;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::time::Instant;

struct Poll {
    since: Instant,
    completed: Option<Instant>,
    in_flight: bool,
    budget: Duration,
    successes: u64,
    errors: u64,
    received_messages: u64,
    last_message: Option<Instant>,
    stalled: bool,
}
#[derive(Default)]
pub(super) struct Diagnostics {
    polls: Mutex<BTreeMap<String, Poll>>,
    workers: Mutex<BTreeMap<String, (&'static str, Instant)>>,
}
impl Diagnostics {
    fn begin(&self, channel: &str, budget: Duration) {
        let mut polls = self.polls.lock().unwrap_or_else(|e| e.into_inner());
        let p = polls.entry(channel.into()).or_insert(Poll {
            since: Instant::now(),
            completed: None,
            in_flight: false,
            budget,
            successes: 0,
            errors: 0,
            received_messages: 0,
            last_message: None,
            stalled: false,
        });
        p.since = Instant::now();
        p.in_flight = true;
        p.budget = budget;
    }
    fn end(&self, channel: &str, success: bool, messages: usize) {
        let mut polls = self.polls.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = polls.get_mut(channel) {
            p.received_messages += messages as u64;
            if messages > 0 {
                p.last_message = Some(Instant::now());
            }
            p.in_flight = false;
            p.completed = Some(Instant::now());
            if success {
                p.successes += 1;
            } else {
                p.errors += 1;
            }
            if p.stalled {
                gateway_log(format!("channel={channel} poll_resumed success={success}"));
                p.stalled = false;
            }
        }
    }
    fn stage(&self, stage: &'static str) {
        let key = format!("{:?}", thread::current().id());
        let mut workers = self.workers.lock().unwrap_or_else(|e| e.into_inner());
        if stage == "idle" {
            workers.remove(&key);
        } else {
            workers.insert(key, (stage, Instant::now()));
        }
    }
    pub(super) fn snapshot(&self, pending: usize, active: usize) -> Value {
        let mut polls = self.polls.lock().unwrap_or_else(|e| e.into_inner());
        let mut channels = serde_json::Map::new();
        for (name, p) in polls.iter_mut() {
            let stalled =
                p.in_flight && p.since.elapsed() > p.budget.saturating_add(Duration::from_secs(10));
            if stalled && !p.stalled {
                gateway_log(format!(
                    "channel={name} poll_stalled elapsed_secs={} budget_secs={}",
                    p.since.elapsed().as_secs(),
                    p.budget.as_secs()
                ));
                p.stalled = true;
            }
            channels.insert(name.clone(),json!({"poll_in_flight":p.in_flight,"poll_age_secs":p.since.elapsed().as_secs(),
                "last_completion_age_secs":p.completed.map(|t|t.elapsed().as_secs()),"poll_stalled":stalled,
                "successes":p.successes,"errors":p.errors,"received_messages":p.received_messages,"last_message_age_secs":p.last_message.map(|t|t.elapsed().as_secs())}));
        }
        let workers = self.workers.lock().unwrap_or_else(|e|e.into_inner()).iter()
            .map(|(id,(stage,since))|json!({"worker":id,"stage":stage,"age_secs":since.elapsed().as_secs()})).collect::<Vec<_>>();
        json!({"pid":std::process::id(),"pending":pending,"active":active,"channels":channels,"workers":workers})
    }
}
struct ObservedChannel {
    inner: Arc<dyn GatewayChannel>,
    diagnostics: Arc<Diagnostics>,
}
impl GatewayChannel for ObservedChannel {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn poll_timeout(&self) -> Duration {
        self.inner.poll_timeout()
    }
    fn diagnostic_stage(&self, stage: &'static str) {
        self.diagnostics.stage(stage);
    }
    fn poll_once(&self) -> Result<Vec<InboundEnvelope>, ChannelPollError> {
        self.diagnostics.begin(self.name(), self.poll_timeout());
        let result = self.inner.poll_once();
        self.diagnostics.end(
            self.name(),
            result.is_ok(),
            result.as_ref().map_or(0, Vec::len),
        );
        result
    }
    fn resolve_inbound_media(&self, envelope: &mut InboundEnvelope) -> Result<(), String> {
        self.inner.resolve_inbound_media(envelope)
    }
    fn outbound_for(&self, envelope: &InboundEnvelope) -> Result<Arc<dyn OutboundSink>, String> {
        self.inner.outbound_for(envelope)
    }
}
pub(super) fn observe(
    channels: ChannelRegistry,
    diagnostics: &Arc<Diagnostics>,
) -> ChannelRegistry {
    channels
        .into_iter()
        .map(|(name, inner)| {
            (
                name,
                Arc::new(ObservedChannel {
                    inner,
                    diagnostics: Arc::clone(diagnostics),
                }) as Arc<dyn GatewayChannel>,
            )
        })
        .collect()
}

#[cfg(unix)]
pub(super) struct Server {
    listener: std::os::unix::net::UnixListener,
    path: PathBuf,
}
#[cfg(unix)]
impl Server {
    pub(super) fn bind(home: &Path) -> Result<Self, String> {
        use std::os::unix::fs::PermissionsExt;
        let path = home.join("run/gateway-diagnostics.sock");
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        if path.exists() {
            if connect(&path).is_some() {
                return Err("diagnostic socket already active".into());
            }
            fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        let listener = std::os::unix::net::UnixListener::bind(&path).map_err(|e| e.to_string())?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(Self { listener, path })
    }
    pub(super) fn serve(&self, value: &Value) {
        // Bound work even if a local client connects repeatedly or never reads.
        if let Ok((mut stream, _)) = self.listener.accept() {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
            let _ = serde_json::to_writer(&mut stream, value);
            let _ = stream.write_all(b"\n");
        }
    }
}
#[cfg(unix)]
impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn query(state_path: &Path, pid: Option<&str>) -> Option<Value> {
    #[cfg(unix)]
    {
        let stream = connect(&state_path.with_file_name("gateway-diagnostics.sock"))?;
        stream
            .set_read_timeout(Some(Duration::from_millis(250)))
            .ok()?;
        let mut text = String::new();
        stream.take(65536).read_to_string(&mut text).ok()?;
        let value: Value = serde_json::from_str(&text).ok()?;
        if value["pid"].as_u64()? != pid?.parse::<u64>().ok()? {
            return None;
        }
        Some(value)
    }
    #[cfg(not(unix))]
    {
        let _ = (state_path, pid);
        None
    }
}

pub(super) fn capture(status: &GatewayStatus, action: &str) {
    let live = query(&status.state_path, status.pid.as_deref());
    let process = status
        .pid
        .as_ref()
        .and_then(|pid| fs::read_to_string(format!("/proc/{pid}/status")).ok())
        .map(|text| {
            text.lines()
                .filter(|line| {
                    ["State:", "PPid:", "Threads:", "VmRSS:", "VmSwap:"]
                        .iter()
                        .any(|key| line.starts_with(key))
                })
                .collect::<Vec<_>>()
                .join("\n")
        });
    let value = json!({"time":beijing_timestamp_now(),"action":action,"pid":status.pid,"running":status.running,
        "agent_status":status.agent_status,"reason":status.reason,"rss_kb":status.rss_kb,"channels":status.channel_status,
        "process":process,"live":live,"live_available":live.is_some()});
    let path = status
        .log_path
        .with_file_name("gateway-last-diagnostic.json");
    #[cfg(unix)]
    if matches!(action, "stop" | "restart")
        && crate::config_io::atomic_write(&path, value.to_string().as_bytes()).is_err()
    {
        gateway_log("diagnostic_snapshot_write_failed");
    }
    let line = format_gateway_log(&format!("control_snapshot {}", value));
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&status.log_path)
    {
        let _ = writeln!(file, "{line}");
    }
}

pub(super) fn install_exit_observers() {
    std::panic::set_hook(Box::new(|info| {
        // Panic payloads can contain HTTP bodies or tool arguments.
        if let Some(location) = info.location() {
            gateway_log(format!(
                "panic file={} line={}",
                location.file(),
                location.line()
            ));
        } else {
            gateway_log("panic location=unknown");
        }
    }));
    #[cfg(unix)]
    unsafe {
        for signal in [
            libc::SIGTERM,
            libc::SIGINT,
            libc::SIGABRT,
            libc::SIGSEGV,
            libc::SIGBUS,
            libc::SIGILL,
            libc::SIGFPE,
        ] {
            libc::signal(signal, termination as *const () as libc::sighandler_t);
        }
    }
}
#[cfg(unix)]
extern "C" fn termination(signal: libc::c_int) {
    let message: &[u8] = match signal {
        libc::SIGTERM => b"[gateway] termination_signal=SIGTERM\n",
        libc::SIGINT => b"[gateway] termination_signal=SIGINT\n",
        libc::SIGABRT => b"[gateway] termination_signal=SIGABRT\n",
        libc::SIGSEGV => b"[gateway] termination_signal=SIGSEGV\n",
        libc::SIGBUS => b"[gateway] termination_signal=SIGBUS\n",
        libc::SIGILL => b"[gateway] termination_signal=SIGILL\n",
        _ => b"[gateway] termination_signal=SIGFPE\n",
    };
    unsafe {
        libc::write(2, message.as_ptr().cast(), message.len());
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

#[cfg(unix)]
fn connect(path: &Path) -> Option<std::os::unix::net::UnixStream> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    unsafe {
        let mut addr: libc::sockaddr_un = std::mem::zeroed();
        if bytes.len() >= addr.sun_path.len() {
            return None;
        }
        addr.sun_family = libc::AF_UNIX as _;
        for (target, byte) in addr.sun_path.iter_mut().zip(bytes) {
            *target = *byte as _;
        }
        #[cfg(target_os = "macos")]
        {
            addr.sun_len = std::mem::size_of::<libc::sockaddr_un>() as _;
        }
        let raw = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
        if raw < 0 {
            return None;
        }
        let fd = OwnedFd::from_raw_fd(raw);
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        if flags < 0 || libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return None;
        }
        let result = libc::connect(
            raw,
            (&addr as *const libc::sockaddr_un).cast(),
            std::mem::size_of_val(&addr) as _,
        );
        if result != 0 {
            let error = std::io::Error::last_os_error().raw_os_error();
            if error != Some(libc::EINPROGRESS) && error != Some(libc::EAGAIN) {
                return None;
            }
            let mut event = libc::pollfd {
                fd: raw,
                events: libc::POLLOUT,
                revents: 0,
            };
            if libc::poll(&mut event, 1, 250) <= 0 {
                return None;
            }
            let mut error: libc::c_int = 0;
            let mut size = std::mem::size_of_val(&error) as libc::socklen_t;
            if libc::getsockopt(
                raw,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut error as *mut libc::c_int).cast(),
                &mut size,
            ) < 0
                || error != 0
            {
                return None;
            }
        }
        if libc::fcntl(raw, libc::F_SETFL, flags) < 0 {
            return None;
        }
        Some(std::os::unix::net::UnixStream::from(fd))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn poll_stall_is_budget_aware_and_recovery_is_visible() {
        let d = Diagnostics::default();
        d.begin("test", Duration::from_secs(300));
        d.polls.lock().unwrap().get_mut("test").unwrap().since =
            Instant::now() - Duration::from_secs(60);
        assert_eq!(d.snapshot(2, 1)["channels"]["test"]["poll_stalled"], false);
        d.polls.lock().unwrap().get_mut("test").unwrap().since =
            Instant::now() - Duration::from_secs(311);
        assert_eq!(d.snapshot(2, 1)["channels"]["test"]["poll_stalled"], true);
        assert!(d.polls.lock().unwrap()["test"].stalled);
        d.end("test", false, 0);
        let snapshot = d.snapshot(0, 0);
        assert_eq!(snapshot["channels"]["test"]["poll_stalled"], false);
        assert_eq!(snapshot["channels"]["test"]["errors"], 1);
        d.stage("model_or_tool");
        assert_eq!(d.snapshot(0, 1)["workers"][0]["stage"], "model_or_tool");
        d.stage("idle");
        assert!(d.snapshot(0, 0)["workers"].as_array().unwrap().is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn live_query_checks_pid_and_does_not_write_state() {
        let home = std::env::temp_dir().join(format!("cz-gd-{}", std::process::id()));
        let server = Server::bind(&home).unwrap();
        let path = home.join("run/gateway.json");
        let pid = std::process::id().to_string();
        let query_path = path.clone();
        let query_pid = pid.clone();
        let reader = thread::spawn(move || query(&query_path, Some(&query_pid)));
        let end = Instant::now() + Duration::from_secs(1);
        while !reader.is_finished() && Instant::now() < end {
            server.serve(&json!({"pid":std::process::id(),"pending":0}));
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(reader.join().unwrap().unwrap()["pending"], 0);
        assert!(!path.exists());
        let mismatch_path = path.clone();
        let mismatch = thread::spawn(move || query(&mismatch_path, Some("0")));
        let end = Instant::now() + Duration::from_secs(1);
        while !mismatch.is_finished() && Instant::now() < end {
            server.serve(&json!({"pid":std::process::id()}));
            thread::sleep(Duration::from_millis(5));
        }
        assert!(mismatch.join().unwrap().is_none());
        let started = Instant::now();
        assert!(query(&path, Some(&pid)).is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(server);
        assert!(!home.join("run/gateway-diagnostics.sock").exists());
        fs::remove_dir_all(home).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn stop_result_does_not_replace_pre_stop_evidence() {
        let home = std::env::temp_dir().join(format!("cz-gds-{}", std::process::id()));
        fs::create_dir_all(&home).unwrap();
        let status = GatewayStatus {
            running: false,
            agent_status: "unhealthy".into(),
            channel_status: BTreeMap::new(),
            pid: None,
            rss_kb: None,
            config_path: "secret-token-not-for-diagnostics".into(),
            cwd: String::new(),
            log_path: home.join("gateway.log"),
            state_path: home.join("gateway.json"),
            started_at: String::new(),
            reason: "not_running".into(),
        };
        capture(&status, "restart");
        let before = fs::read(home.join("gateway-last-diagnostic.json")).unwrap();
        capture(&status, "stop_result");
        assert_eq!(
            before,
            fs::read(home.join("gateway-last-diagnostic.json")).unwrap()
        );
        assert!(!String::from_utf8(before).unwrap().contains("secret-token"));
        fs::remove_dir_all(home).unwrap();
    }
}
