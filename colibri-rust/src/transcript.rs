use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct TranscriptWriter {
    path: PathBuf,
    file: Option<File>,
    metadata: BTreeMap<String, String>,
    retention_days: u64,
    max_total_bytes: usize,
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
        let root = colibri_home().join("transcripts");
        let path = root.join(format!("{}.jsonl", beijing_date(SystemTime::now())));
        Self::new(path, metadata, retention_days, max_total_bytes)
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
        let mut writer = Self {
            path,
            file: Some(file),
            metadata,
            retention_days,
            max_total_bytes,
        };
        writer.cleanup(SystemTime::now());
        Ok(writer)
    }

    pub fn write(&mut self, event_type: &str, payload: serde_json::Value) -> Result<(), String> {
        let Some(file) = self.file.as_mut() else {
            return Ok(());
        };
        let mut payload = payload.as_object().cloned().unwrap_or_default();
        for (key, value) in &self.metadata {
            payload.insert(key.clone(), serde_json::Value::String(value.clone()));
        }
        let event = serde_json::json!({
            "ts": format_beijing_timestamp(SystemTime::now()),
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

fn colibri_home() -> PathBuf {
    std::env::var_os("COLIBRI_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".colibri")
        })
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

pub fn transcript_path_is_current(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == format!("{}.jsonl", beijing_date(SystemTime::now())))
}

pub fn beijing_timestamp_now() -> String {
    format_beijing_timestamp(SystemTime::now())
}
