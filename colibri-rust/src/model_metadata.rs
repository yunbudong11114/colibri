//! Bounded, credential-scoped model metadata. Kept separate from user configuration.
use crate::config::ModelConfig;
use crate::model_capabilities::{self, ModelCapabilities};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Condvar, Mutex, OnceLock,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_AGE: u64 = 600;
const MAX_ENTRIES: usize = 32;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Default, Serialize, Deserialize)]
struct Record {
    checked_at: u64,
    fetched_at: u64,
    context_window: Option<usize>,
    max_output_tokens: Option<usize>,
    failed: bool,
}

struct State {
    record: Option<Record>,
    inflight: bool,
}

struct Slot {
    state: Mutex<State>,
    ready: Condvar,
}

struct Permit;
impl Permit {
    fn acquire() -> Option<Self> {
        ACTIVE
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < 2).then_some(count + 1)
            })
            .ok()
            .map(|_| Self)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn fresh(record: &Record) -> bool {
    record.checked_at <= now() && now().saturating_sub(record.checked_at) < MAX_AGE
}

fn slots() -> &'static Mutex<BTreeMap<PathBuf, Arc<Slot>>> {
    static CACHE: OnceLock<Mutex<BTreeMap<PathBuf, Arc<Slot>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn location(config: &ModelConfig, home: &Path) -> Option<(String, PathBuf)> {
    let mut url = crate::http::http_endpoint(&config.base_url).ok()?;
    if url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    let path = if url.host_str() == Some("api.deepseek.com") {
        "/models".to_string()
    } else {
        format!("{}/models", url.path().trim_end_matches('/'))
    };
    url.set_path(&path);
    let mut digest = Sha256::new();
    digest.update(url.as_str().as_bytes());
    digest.update([0]);
    digest.update(config.api_key.as_bytes());
    digest.update([0]);
    digest.update(config.model.as_bytes());
    let key = format!("{:x}", digest.finalize());
    Some((
        url.to_string(),
        home.join("cache/model-capabilities")
            .join(format!("{key}.json")),
    ))
}

fn read_record(path: &Path) -> Option<Record> {
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(4097)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 4096 {
        return None;
    }
    let record: Record = serde_json::from_slice(&bytes).ok()?;
    if record.context_window == Some(0) || record.max_output_tokens == Some(0) {
        return None;
    }
    Some(record)
}

fn cached_caps(
    config: &ModelConfig,
    record: Option<&Record>,
    source: &'static str,
) -> ModelCapabilities {
    let Some(record) = record
        .filter(|record| record.context_window.is_some() || record.max_output_tokens.is_some())
    else {
        return model_capabilities::resolve(config);
    };
    let fallback = model_capabilities::resolve(config);
    let window = record.context_window.unwrap_or(fallback.context_window);
    let output = record
        .max_output_tokens
        .unwrap_or(fallback.max_output_tokens);
    model_capabilities::apply(
        config,
        Some((Some(window), Some(output))),
        if record.context_window.is_some() {
            source
        } else {
            fallback.source
        },
    )
}

fn positive(value: Option<&serde_json::Value>) -> Option<usize> {
    value?
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
}

fn fetch(config: &ModelConfig, url: &str) -> Option<(Option<usize>, Option<usize>)> {
    let headers = if config.api_key.is_empty() {
        Vec::new()
    } else {
        vec![("Authorization", format!("Bearer {}", config.api_key))]
    };
    let body = crate::http::get_json_limited(url, &headers, 5, MAX_RESPONSE_BYTES).ok()?;
    let body: serde_json::Value = serde_json::from_str(&body).ok()?;
    let mut matches = body
        .get("data")?
        .as_array()?
        .iter()
        .filter(|row| row.get("id").and_then(|id| id.as_str()) == Some(config.model.as_str()));
    let row = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let top = row.get("top_provider");
    let window = positive(row.get("context_window"))
        .or_else(|| positive(row.get("context_length")))
        .or_else(|| positive(top.and_then(|top| top.get("context_length"))));
    let output = positive(row.get("max_output_tokens"))
        .or_else(|| positive(row.get("max_completion_tokens")))
        .or_else(|| positive(top.and_then(|top| top.get("max_completion_tokens"))));
    (window.is_some() || output.is_some()).then_some((window, output))
}

fn persist(path: &Path, record: &Record) {
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    // Only this dedicated cache directory is pruned.
    if let Ok(entries) = fs::read_dir(parent) {
        let mut files = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|file| file.extension().is_some_and(|ext| ext == "json"))
            .collect::<Vec<_>>();
        files.sort_by_key(|file| fs::metadata(file).and_then(|meta| meta.modified()).ok());
        let excess = files.len().saturating_sub(MAX_ENTRIES - 1);
        for file in files.into_iter().filter(|file| file != path).take(excess) {
            let _ = fs::remove_file(file);
        }
    }
    if let Ok(bytes) = serde_json::to_vec(record) {
        let _ = crate::config_io::atomic_write(path, &bytes);
    }
}

fn load(config: &ModelConfig, url: &str, path: &Path, previous: Option<Record>) -> Record {
    let force = previous
        .as_ref()
        .is_some_and(|record| record.checked_at == 0);
    let previous = read_record(path).or(previous);
    if let Some(record) = previous.as_ref().filter(|record| !force && fresh(record)) {
        return record.clone();
    }
    let mut record = previous.unwrap_or_default();
    record.checked_at = now();
    if let Some((window, output)) = fetch(config, url) {
        record.context_window = window.or(record.context_window);
        record.max_output_tokens = output.or(record.max_output_tokens);
        record.fetched_at = now();
        record.failed = false;
    } else {
        if !record.failed {
            eprintln!("[model-metadata] lookup unavailable; retaining cached or bundled limits");
        }
        record.failed = true;
    }
    persist(path, &record);
    record
}

fn finish(slot: &Slot, record: Record) {
    let mut state = slot.state.lock().unwrap_or_else(|error| error.into_inner());
    state.record = Some(record);
    state.inflight = false;
    slot.ready.notify_all();
}

/// First use waits at most the metadata timeout. Stale success is returned while refreshing.
pub fn resolve_online(config: &ModelConfig, home: &Path) -> ModelCapabilities {
    let Some((url, path)) = location(config, home) else {
        return model_capabilities::resolve(config);
    };
    let slot = {
        let mut cache = slots().lock().unwrap_or_else(|error| error.into_inner());
        if !cache.contains_key(&path) && cache.len() >= MAX_ENTRIES {
            let victim = cache
                .iter()
                .find(|(_, slot)| {
                    Arc::strong_count(slot) == 1
                        && slot.state.try_lock().is_ok_and(|state| !state.inflight)
                })
                .map(|(key, _)| key.clone());
            if let Some(victim) = victim {
                cache.remove(&victim);
            } else {
                return model_capabilities::resolve(config);
            }
        }
        Arc::clone(cache.entry(path.clone()).or_insert_with(|| {
            Arc::new(Slot {
                state: Mutex::new(State {
                    record: read_record(&path),
                    inflight: false,
                }),
                ready: Condvar::new(),
            })
        }))
    };
    let mut state = slot.state.lock().unwrap_or_else(|error| error.into_inner());
    if state.record.as_ref().is_some_and(fresh) {
        return cached_caps(config, state.record.as_ref(), "cache");
    }
    if state.inflight {
        if state.record.is_some() {
            return cached_caps(config, state.record.as_ref(), "stale_cache");
        }
        let (state, _) = slot
            .ready
            .wait_timeout_while(state, Duration::from_secs(5), |state| state.inflight)
            .unwrap_or_else(|error| error.into_inner());
        return cached_caps(config, state.record.as_ref(), "cache");
    }
    let Some(permit) = Permit::acquire() else {
        return cached_caps(config, state.record.as_ref(), "stale_cache");
    };
    let previous = state.record.clone();
    state.inflight = true;
    drop(state);
    let stale = cached_caps(config, previous.as_ref(), "stale_cache");
    let has_previous = previous.is_some();
    let background_config = config.clone();
    let background = Arc::clone(&slot);
    let spawned = std::thread::Builder::new()
        .name("model-metadata".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let _permit = permit;
            finish(&background, load(&background_config, &url, &path, previous));
        });
    if spawned.is_err() {
        let mut state = slot.state.lock().unwrap_or_else(|error| error.into_inner());
        state.inflight = false;
        slot.ready.notify_all();
        return stale;
    }
    if has_previous {
        return stale;
    }
    let state = slot.state.lock().unwrap_or_else(|error| error.into_inner());
    let (state, _) = slot
        .ready
        .wait_timeout_while(state, Duration::from_secs(5), |state| state.inflight)
        .unwrap_or_else(|error| error.into_inner());
    cached_caps(config, state.record.as_ref(), "network")
}

/// Read diagnostics without network I/O or changing user configuration.
pub fn peek(config: &ModelConfig, home: &Path) -> ModelCapabilities {
    let Some((_, path)) = location(config, home) else {
        return model_capabilities::resolve(config);
    };
    let record = read_record(&path);
    cached_caps(
        config,
        record.as_ref(),
        if record.as_ref().is_some_and(fresh) {
            "cache"
        } else {
            "stale_cache"
        },
    )
}

/// A provider-confirmed overflow forces a refresh on the next admitted request.
pub fn invalidate(config: &ModelConfig, home: &Path) {
    let Some((_, path)) = location(config, home) else {
        return;
    };
    let cache = slots().lock().unwrap_or_else(|error| error.into_inner());
    if let Some(slot) = cache.get(&path) {
        let mut state = slot.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(record) = state.record.as_mut() {
            record.checked_at = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentConfig;
    use crate::model::OpenAiCompatibleModel;
    use crate::session::AgentSession;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicBool;

    static TEST_LOCK: Mutex<()> = Mutex::new(());
    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Fixture {
        home: PathBuf,
        address: String,
        hits: Arc<AtomicUsize>,
        window: Arc<AtomicUsize>,
        mode: Arc<AtomicUsize>,
        output_seen: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Fixture {
        fn new() -> Self {
            let home = std::env::temp_dir().join(format!(
                "colibri-metadata-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&home).unwrap();
            fs::write(
                home.join("config.toml"),
                "# User configuration stays unchanged\n",
            )
            .unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = format!("http://{}", listener.local_addr().unwrap());
            let hits = Arc::new(AtomicUsize::new(0));
            let window = Arc::new(AtomicUsize::new(32_000));
            let mode = Arc::new(AtomicUsize::new(0));
            let output_seen = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let (h, w, m, o, s) = (
                hits.clone(),
                window.clone(),
                mode.clone(),
                output_seen.clone(),
                stop.clone(),
            );
            let thread = std::thread::spawn(move || {
                while !s.load(Ordering::Acquire) {
                    let Ok((mut socket, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    socket
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut data = Vec::new();
                    let mut byte = [0u8; 1];
                    while !data.ends_with(b"\r\n\r\n") {
                        if socket.read(&mut byte).unwrap_or(0) == 0 {
                            break;
                        }
                        data.push(byte[0]);
                    }
                    let headers = String::from_utf8_lossy(&data);
                    let (status, extra, body) = if headers.starts_with("GET ") {
                        h.fetch_add(1, Ordering::AcqRel);
                        std::thread::sleep(Duration::from_millis(50));
                        match m.load(Ordering::Acquire) {
                            1 => ("500 Internal Server Error", "", "{}".into()),
                            2 => ("200 OK", "", r#"{"data":[{"id":"fixture-model"}]}"#.into()),
                            3 => ("302 Found", "Location: http://127.0.0.1:1/secret\r\n", "{}".into()),
                            4 => ("200 OK", "", "x".repeat(MAX_RESPONSE_BYTES + 1)),
                            5 => {
                                std::thread::sleep(Duration::from_secs(7));
                                ("200 OK", "", r#"{"data":[]}"#.into())
                            }
                            _ => ("200 OK", "", serde_json::json!({"data":[{
                                "id":"fixture-model","context_window":w.load(Ordering::Acquire),"max_output_tokens":1024
                            }]}).to_string()),
                        }
                    } else {
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.split_once(':')
                                    .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        let mut input = vec![0; length];
                        socket.read_exact(&mut input).unwrap();
                        let input: serde_json::Value = serde_json::from_slice(&input).unwrap();
                        o.store(
                            input["max_completion_tokens"].as_u64().unwrap() as usize,
                            Ordering::Release,
                        );
                        ("200 OK", "", r#"{"choices":[{"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}]}"#.into())
                    };
                    let response = format!("HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = socket.write_all(response.as_bytes());
                }
            });
            Self {
                home,
                address,
                hits,
                window,
                mode,
                output_seen,
                stop,
                thread: Some(thread),
            }
        }

        fn config(&self) -> ModelConfig {
            let mut config = AgentConfig::default().model;
            config.provider = "openai_compatible".into();
            config.model = "fixture-model".into();
            config.base_url = self.address.clone();
            config.api_key = "secret-fixture-a".into();
            config
        }

        fn record(&self) -> Record {
            let (_, path) = location(&self.config(), &self.home).unwrap();
            read_record(&path).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            let _ = self.thread.take().unwrap().join();
            let _ = fs::remove_dir_all(&self.home);
        }
    }

    fn wait_for(mut ready: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !ready() {
            assert!(
                std::time::Instant::now() < deadline,
                "background refresh timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn network_limits_feed_generation_without_writing_user_settings() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = Fixture::new();
        let before = fs::read(fixture.home.join("config.toml")).unwrap();
        let mut config = AgentConfig::default();
        config.model = fixture.config();
        config.memory.enabled = false;
        config.session.transcript = false;
        config.tools.enabled.clear();
        config.skills.max_catalog = 0;
        let model = OpenAiCompatibleModel::from_config(&config.model).unwrap();
        let mut session =
            AgentSession::new(config, Box::new(model)).with_runtime_home(fixture.home.clone());
        assert_eq!(fixture.hits.load(Ordering::Acquire), 0);
        assert_eq!(session.submit("hello").unwrap().text, "done");
        assert_eq!(
            peek(&fixture.config(), &fixture.home).context_window,
            32_000
        );
        assert_eq!(fixture.output_seen.load(Ordering::Acquire), 1024);
        assert_eq!(fs::read(fixture.home.join("config.toml")).unwrap(), before);
        let (_, path) = location(&fixture.config(), &fixture.home).unwrap();
        let stored = fs::read_to_string(&path).unwrap();
        assert!(!stored.contains("secret-fixture"));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn cold_concurrent_queries_share_one_load_and_credentials_are_isolated() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = Fixture::new();
        std::thread::scope(|scope| {
            let jobs = (0..8)
                .map(|_| {
                    let config = fixture.config();
                    let home = &fixture.home;
                    scope.spawn(move || resolve_online(&config, home).context_window)
                })
                .collect::<Vec<_>>();
            for job in jobs {
                assert_eq!(job.join().unwrap(), 32_000);
            }
        });
        assert_eq!(fixture.hits.load(Ordering::Acquire), 1);
        fixture.window.store(64_000, Ordering::Release);
        let mut other = fixture.config();
        other.api_key = "secret-fixture-b".into();
        assert_eq!(resolve_online(&other, &fixture.home).context_window, 64_000);
        assert_eq!(fixture.hits.load(Ordering::Acquire), 2);
    }

    #[test]
    fn failed_refresh_keeps_success_and_later_refresh_updates_next_request() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = Fixture::new();
        let config = fixture.config();
        assert_eq!(
            resolve_online(&config, &fixture.home).context_window,
            32_000
        );
        fixture.mode.store(1, Ordering::Release);
        invalidate(&config, &fixture.home);
        assert_eq!(
            resolve_online(&config, &fixture.home).context_window,
            32_000
        );
        wait_for(|| {
            fixture.record().failed
                && resolve_online(&fixture.config(), &fixture.home).source == "cache"
        });
        assert_eq!(
            resolve_online(&config, &fixture.home).context_window,
            32_000
        );
        assert_eq!(fixture.hits.load(Ordering::Acquire), 2);
        fixture.mode.store(0, Ordering::Release);
        fixture.window.store(64_000, Ordering::Release);
        invalidate(&config, &fixture.home);
        assert_eq!(
            resolve_online(&config, &fixture.home).context_window,
            32_000
        );
        wait_for(|| resolve_online(&config, &fixture.home).context_window == 64_000);
        assert_eq!(
            resolve_online(&config, &fixture.home).context_window,
            64_000
        );
    }

    #[test]
    fn persisted_cache_survives_restart_and_expired_values_refresh_in_background() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = Fixture::new();
        let config = fixture.config();
        assert_eq!(
            resolve_online(&config, &fixture.home).context_window,
            32_000
        );
        let record = fixture.record();
        let restarted = fixture.home.join("restarted");
        let (_, path) = location(&config, &restarted).unwrap();
        persist(&path, &record);
        assert_eq!(resolve_online(&config, &restarted).context_window, 32_000);
        assert_eq!(fixture.hits.load(Ordering::Acquire), 1);
        let expired_home = fixture.home.join("expired");
        let (_, path) = location(&config, &expired_home).unwrap();
        let mut expired = record;
        expired.checked_at = now() - MAX_AGE - 1;
        persist(&path, &expired);
        fixture.window.store(64_000, Ordering::Release);
        assert_eq!(
            resolve_online(&config, &expired_home).context_window,
            32_000
        );
        wait_for(|| resolve_online(&config, &expired_home).context_window == 64_000);
        assert_eq!(fixture.hits.load(Ordering::Acquire), 2);
    }

    #[test]
    fn first_use_wait_is_bounded_when_the_service_stalls() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = Fixture::new();
        fixture.mode.store(5, Ordering::Release);
        let started = std::time::Instant::now();
        let caps = resolve_online(&fixture.config(), &fixture.home);
        assert_eq!(caps.context_window, 48_000);
        assert!(started.elapsed() < Duration::from_secs(6));
    }

    #[test]
    fn unsupported_redirected_and_oversized_metadata_fall_back() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        for mode in [2, 3, 4] {
            let fixture = Fixture::new();
            fixture.mode.store(mode, Ordering::Release);
            let caps = resolve_online(&fixture.config(), &fixture.home);
            assert_eq!(caps.context_window, 48_000);
            assert_eq!(caps.source, "fallback");
            assert_eq!(fixture.hits.load(Ordering::Acquire), 1);
        }
    }
}
