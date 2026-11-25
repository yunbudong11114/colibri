//! Private bounded metadata and process identity.
use super::Job;
#[cfg(not(target_os = "linux"))]
use std::process::Command;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub(super) fn root(home: &Path) -> PathBuf {
    home.join("jobs")
}
pub(super) fn directory(home: &Path, id: &str) -> Result<PathBuf, String> {
    if id.len() != 24 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid job_id".into());
    }
    Ok(root(home).join(id))
}
pub(super) fn identity(pid: u32) -> String {
    #[cfg(target_os = "linux")]
    {
        let s = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        let fields = s
            .rsplit_once(')')
            .map(|(_, s)| s.split_whitespace().collect::<Vec<_>>())
            .unwrap_or_default();
        if fields.first().is_some_and(|v| *v == "Z") {
            return String::new();
        }
        return fields.get(19).unwrap_or(&"").to_string();
    }
    #[cfg(not(target_os = "linux"))]
    {
        Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "lstart="])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    }
}
pub(super) fn save(dir: &Path, job: &Job) -> Result<(), String> {
    crate::config_io::atomic_write(
        &dir.join("state.json"),
        &serde_json::to_vec(job).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}
fn read_metadata(dir: &Path) -> Result<Job, String> {
    let mut b = Vec::new();
    fs::File::open(dir.join("state.json"))
        .map_err(|e| e.to_string())?
        .take(8193)
        .read_to_end(&mut b)
        .map_err(|e| e.to_string())?;
    if b.len() > 8192 {
        return Err("Invalid job metadata".into());
    }
    serde_json::from_slice(&b).map_err(|e| e.to_string())
}

pub(super) fn load(dir: &Path) -> Result<Job, String> {
    load_with_probe(dir, identity)
}

fn load_with_probe(dir: &Path, probe: impl FnOnce(u32) -> String) -> Result<Job, String> {
    let mut job = read_metadata(dir)?;
    if job.status == "running" {
        let observed = probe(job.pid);
        if job.pid == 0 || job.identity.is_empty() || observed != job.identity {
            // The supervisor can save its final state and exit while identity is being read.
            let latest = read_metadata(dir)?;
            if latest.status != "running"
                || latest.pid != job.pid
                || latest.identity != job.identity
            {
                return Ok(latest);
            }
            if job.pid != 0 && observed.is_empty() {
                let alive = unsafe { libc::kill(job.pid as i32, 0) } == 0
                    || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
                if alive {
                    return Ok(latest);
                }
            }
            job.status = "interrupted".into();
            job.error = Some("Supervisor identity is no longer valid".into());
        }
    }
    Ok(job)
}
pub(super) fn collect_jobs(home: &Path) -> Vec<Job> {
    let mut jobs = fs::read_dir(root(home))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| load(&e.path()).ok())
        .collect::<Vec<_>>();
    jobs.sort_by(|a, b| (b.created, &b.job_id).cmp(&(a.created, &a.job_id)));
    jobs
}

#[cfg(test)]
mod tests {
    use super::*;
    fn running() -> Job {
        Job {
            job_id: "0123456789abcdef01234567".into(),
            status: "running".into(),
            pid: std::process::id(),
            identity: "known-process".into(),
            exit_code: None,
            truncated: false,
            bytes: 0,
            created: 0,
            label: "fixture".into(),
            error: None,
        }
    }
    #[test]
    fn completion_during_process_probe_uses_new_terminal_state() {
        let scratch = crate::config_io::Scratch::new(&std::env::temp_dir()).unwrap();
        let job = running();
        save(&scratch.0, &job).unwrap();
        let result = load_with_probe(&scratch.0, |_| {
            let mut completed = job.clone();
            completed.status = "completed".into();
            completed.exit_code = Some(0);
            save(&scratch.0, &completed).unwrap();
            String::new()
        })
        .unwrap();
        assert_eq!(result.status, "completed");
        assert_eq!(result.exit_code, Some(0));
    }
    #[test]
    fn missing_probe_result_does_not_interrupt_a_live_supervisor() {
        let scratch = crate::config_io::Scratch::new(&std::env::temp_dir()).unwrap();
        save(&scratch.0, &running()).unwrap();
        assert_eq!(
            load_with_probe(&scratch.0, |_| String::new())
                .unwrap()
                .status,
            "running"
        );
    }
}
