//! Bounded, non-interactive background jobs, independent of any UI or device.
use crate::{messages::ToolResult, tools::ToolContext};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime},
};
const LOG_CAP: usize = 1024 * 1024;
const TAIL_CAP: usize = 32 * 1024;
#[derive(Serialize, Deserialize)]
struct Spec {
    command: String,
    cwd: PathBuf,
    timeout: f64,
    secrets: Vec<String>,
}
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Job {
    pub job_id: String,
    pub status: String,
    pub pid: u32,
    pub identity: String,
    pub exit_code: Option<i32>,
    pub truncated: bool,
    pub bytes: usize,
    pub created: u64,
    pub label: String,
    pub error: Option<String>,
}
pub(crate) fn list(c: &ToolContext) -> Vec<Job> {
    collect_jobs(&c.runtime_home).into_iter().take(8).collect()
}

pub(crate) fn start(a: &BTreeMap<String, String>, c: &ToolContext) -> ToolResult {
    let result = (|| -> Result<String, String> {
        let command = a
            .get("command")
            .filter(|s| !s.trim().is_empty())
            .ok_or("Missing command")?
            .clone();
        if command.len() > 65536 {
            return Err("Command exceeds 64 KiB".into());
        }
        let timeout = a
            .get("timeout_seconds")
            .map(|s| s.parse::<f64>())
            .transpose()
            .map_err(|_| "Invalid timeout_seconds")?
            .unwrap_or(c.config.tools.max_shell_seconds);
        if !timeout.is_finite() || timeout <= 0.0 || timeout > 86400.0 {
            return Err("timeout_seconds must be within (0, 86400]".into());
        }
        fs::create_dir_all(root(&c.runtime_home)).map_err(|e| e.to_string())?;
        let _lock = crate::config_io::lock(&root(&c.runtime_home).join(".start.lock"))
            .map_err(|e| e.to_string())?;
        let all = collect_jobs(&c.runtime_home);
        if all.iter().any(|j| j.status == "running") {
            return Err(
                "One background job is already running; poll or cancel it before starting another"
                    .into(),
            );
        }
        for old in all.iter().skip(7).filter(|j| j.status != "running") {
            let _ = fs::remove_dir_all(root(&c.runtime_home).join(&old.job_id));
        }
        let seed = format!(
            "{}-{:?}-{}",
            std::process::id(),
            SystemTime::now(),
            command.len()
        );
        let id = format!("{:x}", Sha256::digest(seed.as_bytes()))[..24].to_string();
        let dir = root(&c.runtime_home).join(&id);
        fs::create_dir(&dir).map_err(|e| e.to_string())?;
        use std::os::unix::{fs::PermissionsExt, process::CommandExt};
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
        let mut label = command.clone();
        crate::secrets::redact_tool_output(&c.config, &mut label);
        let label = crate::memory::truncate(label, 160).0;
        let mut j = Job {
            job_id: id.clone(),
            status: "running".into(),
            pid: 0,
            identity: String::new(),
            exit_code: None,
            truncated: false,
            bytes: 0,
            created: now(),
            label,
            error: None,
        };
        save(&dir, &j)?;
        let exe = c
            .execution_host
            .clone()
            .map(Ok)
            .unwrap_or_else(std::env::current_exe)
            .map_err(|e| e.to_string())?;
        let mut process = Command::new(exe);
        process
            .arg("__job-run")
            .arg(&dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            process.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = match process.spawn() {
            Ok(child) => child,
            Err(e) => {
                j.status = "failed".into();
                let _ = save(&dir, &j);
                return Err(e.to_string());
            }
        };
        j.pid = child.id();
        j.identity = identity(j.pid);
        if let Err(e) = save(&dir, &j) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        let raw = a
            .get("cwd")
            .map(|v| crate::config::expand_user_path(v))
            .unwrap_or_else(|| c.cwd.clone());
        let cwd = if raw.is_absolute() {
            raw
        } else {
            c.cwd.join(raw)
        };
        let spec = Spec {
            command,
            cwd,
            timeout,
            secrets: crate::secrets::values(&c.config),
        };
        let sent =
            serde_json::to_writer(child.stdin.as_mut().unwrap(), &spec).map_err(|e| e.to_string());
        drop(child.stdin.take());
        if let Err(e) = sent {
            let _ = child.kill();
            let _ = child.wait();
            j.status = "failed".into();
            let _ = save(&dir, &j);
            return Err(e);
        }
        // Reap our supervisor without making the tool call wait for the job.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(serde_json::json!({"job_id":id,"status":"running","next_offset":0,"note":"Started, not completed. Use shell.poll to observe the final exit code and output."}).to_string())
    })();
    match result {
        Ok(s) => ToolResult::ok(s),
        Err(e) => ToolResult::error("job_start_failed", e),
    }
}

pub(crate) fn poll(a: &BTreeMap<String, String>, c: &ToolContext) -> ToolResult {
    let result = (|| -> Result<String, String> {
        let id = a.get("job_id").ok_or("Missing job_id")?;
        let dir = directory(&c.runtime_home, id)?;
        let wait = a
            .get("wait_ms")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
            .min(5000);
        let deadline = Instant::now() + Duration::from_millis(wait);
        let j = loop {
            let j = load(&dir)?;
            if j.status != "running" || Instant::now() >= deadline {
                break j;
            }
            std::thread::sleep(Duration::from_millis(30))
        };
        let tail = a.get("tail").is_some_and(|v| v == "true");
        let mut offset = if tail {
            0
        } else {
            a.get("offset").and_then(|v| v.parse().ok()).unwrap_or(0u64)
        };
        let mut bytes = Vec::new();
        let output = dir.join(if tail { "tail.log" } else { "output.log" });
        if let Ok(mut file) = fs::File::open(output) {
            let limit = c.config.tools.max_result_chars.max(4).min(65536) as u64;
            if tail {
                offset = file
                    .metadata()
                    .map_err(|e| e.to_string())?
                    .len()
                    .saturating_sub(limit);
            }
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| e.to_string())?;
            file.take(c.config.tools.max_result_chars.max(4).min(65536) as u64)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
        }
        if let Err(e) = std::str::from_utf8(&bytes) {
            if e.error_len().is_none() {
                bytes.truncate(e.valid_up_to());
            }
        }
        let decoded = String::from_utf8_lossy(&bytes);
        let used = decoded
            .char_indices()
            .nth(c.config.tools.max_result_chars.max(1))
            .map(|(n, _)| n)
            .unwrap_or(decoded.len());
        let next = offset + used.min(bytes.len()) as u64;
        let mut text = decoded[..used].to_string();
        crate::secrets::redact_tool_output(&c.config, &mut text);
        Ok(serde_json::json!({"job_id":j.job_id,"status":j.status,"exit_code":j.exit_code,"output":text,"next_offset":next,"truncated":j.truncated,"tail_available":j.truncated,"supervisor_pid":j.pid,"error":j.error}).to_string())
    })();
    match result {
        Ok(s) => ToolResult::ok(s),
        Err(e) => ToolResult::error("job_poll_failed", e),
    }
}
pub(crate) fn cancel(a: &BTreeMap<String, String>, c: &ToolContext) -> ToolResult {
    let result = (|| -> Result<String, String> {
        let dir = directory(&c.runtime_home, a.get("job_id").ok_or("Missing job_id")?)?;
        let j = load(&dir)?;
        if j.status == "running" || j.status == "interrupted" {
            crate::config_io::atomic_write(&dir.join("cancel"), b"cancel")
                .map_err(|e| e.to_string())?;
        }
        Ok(serde_json::json!({"job_id":j.job_id,"status":if j.status=="running" || j.status=="interrupted"{"stopping"}else{&j.status}}).to_string())
    })();
    match result {
        Ok(s) => ToolResult::ok(s),
        Err(e) => ToolResult::error("job_cancel_failed", e),
    }
}

mod output;
mod process;
mod store;
pub(crate) use process::run_worker;
use store::*;
