use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{colibri_home, SessionConfig};

pub struct TranscriptWriter {
    path: PathBuf,
    file: Option<File>,
    metadata: BTreeMap<String, String>,
    retention_days: u64,
    max_total_bytes: usize,
    daily: bool,
}

impl TranscriptWriter {
    pub fn default() -> Result<Self, String> {
        Self::default_with_metadata(BTreeMap::new())
    }

    pub fn default_with_metadata(metadata: BTreeMap<String, String>) -> Result<Self, String> {
        Self::default_with_metadata_and_limits(metadata, 0, 0)
    }

    pub fn default_with_metadata_and_limits(
        metadata: BTreeMap<String, String>,
        retention_days: u64,
        max_total_bytes: usize,
    ) -> Result<Self, String> {
        Self::daily_in_home(
            &colibri_home(),
            metadata,
            retention_days,
            max_total_bytes,
            SystemTime::now(),
        )
    }

    /// Shared policy for App, CLI/REPL and gateway; the caller supplies its runtime home.
    pub fn for_session(home: &Path, config: &SessionConfig) -> Result<Option<Self>, String> {
        if !config.transcript {
            return Ok(None);
        }
        Self::daily_in_home(
            home,
            BTreeMap::new(),
            config.transcript_retention_days,
            config.transcript_max_total_bytes,
            SystemTime::now(),
        )
        .map(Some)
    }

    fn daily_in_home(
        home: &Path,
        metadata: BTreeMap<String, String>,
        retention_days: u64,
        max_total_bytes: usize,
        now: SystemTime,
    ) -> Result<Self, String> {
        let path = home
            .join("transcripts")
            .join(format!("{}.jsonl", beijing_date(now)));
        let mut writer = Self::new(path, metadata, retention_days, max_total_bytes)?;
        writer.daily = true;
        Ok(writer)
    }

    pub fn new(
        path: PathBuf,
        metadata: BTreeMap<String, String>,
        retention_days: u64,
        max_total_bytes: usize,
    ) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|error| error.to_string())?;
        }
        let mut writer = Self {
            path,
            file: Some(file),
            metadata,
            retention_days,
            max_total_bytes,
            daily: false,
        };
        writer.cleanup(SystemTime::now());
        Ok(writer)
    }

    pub fn write(&mut self, event_type: &str, payload: serde_json::Value) -> Result<(), String> {
        self.write_at(event_type, payload, SystemTime::now())
    }

    fn write_at(
        &mut self,
        event_type: &str,
        payload: serde_json::Value,
        now: SystemTime,
    ) -> Result<(), String> {
        if self.file.is_none() {
            return Ok(());
        }
        if self.daily {
            let path = self
                .path
                .parent()
                .ok_or("Transcript directory missing")?
                .join(format!("{}.jsonl", beijing_date(now)));
            if path != self.path {
                // Open first: on failure the old writer is left intact and the event fails.
                let mut next = Self::new(
                    path,
                    self.metadata.clone(),
                    self.retention_days,
                    self.max_total_bytes,
                )?;
                next.daily = true;
                *self = next;
            }
        }
        let file = self.file.as_mut().unwrap();
        // Separate writers in App, CLI and gateway must append complete JSON records.
        #[cfg(unix)]
        let _append_lock = crate::config_io::lock(&self.path).map_err(|error| error.to_string())?;
        let mut payload = payload.as_object().cloned().unwrap_or_default();
        for (key, value) in &self.metadata {
            payload.insert(key.clone(), serde_json::Value::String(value.clone()));
        }
        let event = serde_json::json!({
            "ts": format_beijing_timestamp(now),
            "type": event_type,
            "payload": payload,
        });
        serde_json::to_writer(&mut *file, &event).map_err(|error| error.to_string())?;
        file.write_all(b"\n").map_err(|error| error.to_string())?;
        file.flush().map_err(|error| error.to_string())
    }

    pub fn close(&mut self) {
        self.file.take();
    }

    fn cleanup(&mut self, now: SystemTime) {
        if self.retention_days == 0 && self.max_total_bytes == 0 {
            return;
        }
        let Some(root) = self.path.parent() else {
            return;
        };
        let Ok(entries) = fs::read_dir(root) else {
            return;
        };
        let mut paths = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("jsonl"))
            .collect::<Vec<_>>();
        if self.retention_days > 0 {
            let max_age = self.retention_days.saturating_mul(86_400);
            for path in &paths {
                if path == &self.path {
                    continue;
                }
                let expired = fs::metadata(path)
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|modified| now.duration_since(modified).ok())
                    .is_some_and(|age| age.as_secs() > max_age);
                if expired {
                    let _ = fs::remove_file(path);
                }
            }
        }
        if self.max_total_bytes == 0 {
            return;
        }
        paths.retain(|path| path.exists());
        let mut files = paths
            .iter()
            .filter_map(|path| {
                let meta = fs::metadata(path).ok()?;
                let modified = meta.modified().unwrap_or(UNIX_EPOCH);
                Some((modified, path.clone(), meta.len() as usize))
            })
            .collect::<Vec<_>>();
        let mut total = files.iter().map(|(_, _, size)| *size).sum::<usize>();
        files.sort_by_key(|(modified, path, _)| (*modified, path.clone()));
        for (_, path, size) in files {
            if total <= self.max_total_bytes {
                break;
            }
            if path == self.path {
                continue;
            }
            if fs::remove_file(path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }
}

impl Drop for TranscriptWriter {
    fn drop(&mut self) {
        self.close();
    }
}

fn format_beijing_timestamp(time: SystemTime) -> String {
    const BEIJING_OFFSET_SECS: i64 = 8 * 3600;
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
        + BEIJING_OFFSET_SECS;
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = second_of_day / 3_600;
    let minute = second_of_day % 3_600 / 60;
    let second = second_of_day % 60;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}+08:00",
        year, month, day, hour, minute, second
    )
}

fn beijing_date(time: SystemTime) -> String {
    format_beijing_timestamp(time)
        .get(0..10)
        .unwrap_or("1970-01-01")
        .to_string()
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719468;
    let era = days.div_euclid(146097);
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

pub fn beijing_timestamp_now() -> String {
    format_beijing_timestamp(SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    };
    use std::time::Duration;

    struct Home(PathBuf);
    impl Home {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "colibri-daily-log-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn daily_writer_rotates_at_beijing_midnight_and_close_does_not_reopen() {
        let home = Home::new();
        let before = UNIX_EPOCH + Duration::from_secs(86400 - 28800 - 1);
        let after = before + Duration::from_secs(2);
        let mut writer = TranscriptWriter::daily_in_home(
            &home.0,
            BTreeMap::from([("session_key".into(), "weixin:fixture".into())]),
            0,
            0,
            before,
        )
        .unwrap();
        writer
            .write_at("user_message", serde_json::json!({"text":"before"}), before)
            .unwrap();
        writer
            .write_at(
                "assistant_message",
                serde_json::json!({"text":"after"}),
                after,
            )
            .unwrap();
        let first = fs::read_to_string(home.0.join("transcripts/1970-01-01.jsonl")).unwrap();
        let second = fs::read_to_string(home.0.join("transcripts/1970-01-02.jsonl")).unwrap();
        assert_eq!(first.lines().count(), 1);
        let event: serde_json::Value = serde_json::from_str(&second).unwrap();
        assert_eq!(event["ts"], "1970-01-02T00:00:01+08:00");
        assert_eq!(event["payload"]["session_key"], "weixin:fixture");
        writer.close();
        writer
            .write_at(
                "ignored",
                serde_json::json!({}),
                after + Duration::from_secs(86400),
            )
            .unwrap();
        assert!(!home.0.join("transcripts/1970-01-03.jsonl").exists());
        assert_eq!(
            fs::read_to_string(home.0.join("transcripts/1970-01-02.jsonl")).unwrap(),
            second
        );
    }

    #[test]
    fn independent_daily_writers_append_whole_records_without_losing_metadata() {
        let home = Home::new();
        let barrier = Arc::new(Barrier::new(4));
        let threads = (0..4)
            .map(|i| {
                let home = home.0.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut writer = TranscriptWriter::daily_in_home(
                        &home,
                        BTreeMap::from([("session_key".into(), format!("writer:{i}"))]),
                        0,
                        0,
                        UNIX_EPOCH,
                    )
                    .unwrap();
                    barrier.wait();
                    for n in 0..64 {
                        writer
                            .write_at(
                                "event",
                                serde_json::json!({"n":n,"text":"内容".repeat(512)}),
                                UNIX_EPOCH,
                            )
                            .unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        let data = fs::read_to_string(home.0.join("transcripts/1970-01-01.jsonl")).unwrap();
        let mut ids = std::collections::BTreeSet::new();
        for line in data.lines() {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(
                value["payload"]["text"].as_str().unwrap().len(),
                "内容".repeat(512).len()
            );
            assert!(ids.insert((
                value["payload"]["session_key"].as_str().unwrap().to_owned(),
                value["payload"]["n"].as_u64().unwrap()
            )));
        }
        assert_eq!(ids.len(), 256);
    }
}
