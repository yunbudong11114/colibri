//! Supervisor lifecycle, cancellation and timeout cleanup.
use super::output::{reader, Output};
use super::store::{identity, load, save};
use super::{Spec, LOG_CAP};
use std::{
    fs,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
pub(crate) fn run_worker(dir: &Path) -> Result<(), String> {
    let spec: Spec = serde_json::from_reader(std::io::stdin().take(2 * 1024 * 1024))
        .map_err(|e| e.to_string())?;
    let mut job = load(dir)?;
    job.status = "running".into();
    job.pid = std::process::id();
    job.identity = identity(job.pid);
    save(dir, &job)?;
    let result = (|| -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::process::CommandExt;
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(dir.join("output.log"))
            .map_err(|e| e.to_string())?;
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&spec.command)
            .current_dir(&spec.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|e| e.to_string())?;
        let state = Arc::new(Mutex::new(Output {
            head: file,
            tail: Vec::new(),
            bytes: 0,
            total: 0,
            last_flush: Instant::now(),
            error: None,
        }));
        let stdout = reader(
            child.stdout.take().unwrap(),
            state.clone(),
            dir.into(),
            spec.secrets.clone(),
        );
        let stderr = reader(
            child.stderr.take().unwrap(),
            state.clone(),
            dir.into(),
            spec.secrets.clone(),
        );
        let deadline = Instant::now() + Duration::from_secs_f64(spec.timeout);
        let mut reason = None;
        let mut last_saved = Instant::now();
        let mut last_progress = (job.bytes, job.truncated);
        loop {
            if exited(child.id()) {
                break;
            }
            if dir.join("cancel").exists() {
                reason = Some("killed")
            } else if Instant::now() >= deadline {
                reason = Some("timed_out")
            }
            if reason.is_some() {
                break;
            }
            if let Ok(s) = state.lock() {
                job.bytes = s.bytes;
                job.truncated = s.total > LOG_CAP || s.error.is_some();
                job.error = s.error.clone();
            }
            if last_saved.elapsed() >= Duration::from_secs(1)
                && (job.bytes, job.truncated) != last_progress
            {
                if save(dir, &job).is_ok() {
                    last_progress = (job.bytes, job.truncated);
                }
                last_saved = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // Keep the main child unreaped until the process group has been cleaned up.
        // Its PID therefore cannot be reused for an unrelated group during cancellation.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        let readers_ok = stdout.join().is_ok() & stderr.join().is_ok();
        let s = state.lock().map_err(|_| "output lock poisoned")?;
        job.bytes = s.bytes;
        let tail_ok = crate::config_io::atomic_write(&dir.join("tail.log"), &s.tail).is_ok();
        job.truncated = s.total > LOG_CAP || s.error.is_some() || !readers_ok || !tail_ok;
        job.error = s.error.clone().or_else(|| {
            (!readers_ok || !tail_ok)
                .then(|| "output_capture_failed: saved log is incomplete".into())
        });
        job.exit_code = status.code();
        job.status = reason
            .unwrap_or(if status.success() {
                "completed"
            } else {
                "failed"
            })
            .into();
        Ok(())
    })();
    if let Err(error) = &result {
        job.status = "failed".into();
        let mut e = error.clone();
        for secret in &spec.secrets {
            if !secret.is_empty() {
                e = e.replace(secret, "<redacted>")
            }
        }
        job.error = Some(crate::memory::truncate(e, 1000).0);
    }
    save(dir, &job)?;
    result
}

fn exited(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| {
                s.rsplit_once(')')
                    .map(|(_, v)| v.trim_start().starts_with('Z'))
            })
            .unwrap_or(true)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "stat="])
            .output()
            .map(|o| {
                !o.status.success()
                    || String::from_utf8_lossy(&o.stdout)
                        .trim_start()
                        .starts_with('Z')
            })
            .unwrap_or(false)
    }
}
