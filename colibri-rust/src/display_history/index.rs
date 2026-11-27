//! Disposable per-file display index. Never stores conversation bodies or model context.
use super::Record;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};
const CACHE_LIMIT: u64 = 4 * 1024 * 1024;
#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: u128,
    floor: u64,
    identity: (u64, u64),
}
#[derive(Serialize, Deserialize)]
struct Entry {
    offset: u64,
    ts: String,
}
#[derive(Serialize, Deserialize)]
struct FileIndex {
    name: String,
    stamp: Stamp,
    entries: Vec<Entry>,
}
#[derive(Serialize, Deserialize)]
struct Snapshot {
    format: u8,
    source: String,
    files: Vec<FileIndex>,
}
fn cache_path(home: &Path, source: &str) -> PathBuf {
    home.join("cache/display-history")
        .join(format!("{:x}.json", Sha256::digest(source.as_bytes())))
}
fn stamp(path: &Path, floor: u64) -> Result<Stamp, String> {
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), meta.ino())
    };
    #[cfg(not(unix))]
    let identity = (0, 0);
    Ok(Stamp {
        size: meta.len(),
        modified: meta
            .modified()
            .ok()
            .and_then(|v| v.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |v| v.as_nanos()),
        floor,
        identity,
    })
}
pub(super) fn load(
    home: &Path,
    paths: Vec<PathBuf>,
    floors: &BTreeMap<String, u64>,
    source: &str,
    rebuild: bool,
) -> Result<Vec<Record>, String> {
    let path = cache_path(home, source);
    let previous = if rebuild {
        None
    } else {
        File::open(&path)
            .ok()
            .and_then(|f| {
                if f.metadata().ok()?.len() > CACHE_LIMIT {
                    return None;
                }
                serde_json::from_reader::<_, Snapshot>(f.take(CACHE_LIMIT)).ok()
            })
            .filter(|v| v.format == 1 && v.source == source)
    };
    let mut previous = previous
        .map(|v| {
            v.files
                .into_iter()
                .map(|f| (f.name.clone(), f))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut dirty = false;
    let mut snapshot_stable = true;
    let mut files = Vec::new();
    for file in paths {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        let floor = floors.get(&name).copied().unwrap_or(0);
        let current = stamp(&file, floor)?;
        if let Some(old) = previous.remove(&name).filter(|v| {
            v.stamp == current
                && v.entries
                    .iter()
                    .all(|e| e.offset >= floor && e.offset < current.size)
                && v.entries.windows(2).all(|e| e[0].offset < e[1].offset)
        }) {
            files.push(old);
            continue;
        }
        dirty = true;
        let entries = scan(&file, floor, source)?;
        // Concurrent writes must not publish a snapshot stamped as unchanged.
        // The observed records remain usable; the next request notices the new stamp.
        if stamp(&file, floor)? != current {
            snapshot_stable = false;
        }
        files.push(FileIndex {
            name,
            stamp: current,
            entries,
        });
    }
    dirty |= !previous.is_empty();
    let snapshot = Snapshot {
        format: 1,
        source: source.into(),
        files,
    };
    if dirty && snapshot_stable {
        persist(&path, &snapshot);
    }
    let mut records = snapshot
        .files
        .into_iter()
        .flat_map(|file| {
            let path = home.join("transcripts").join(file.name);
            file.entries.into_iter().map(move |entry| {
                (
                    entry.ts,
                    Record {
                        file: path.clone(),
                        offset: entry.offset,
                    },
                )
            })
        })
        .collect::<Vec<_>>();
    records.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(records.into_iter().map(|(_, r)| r).collect())
}
#[cfg(unix)]
fn persist(path: &Path, snapshot: &Snapshot) {
    use std::os::unix::fs::DirBuilderExt;
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .is_err()
    {
        return;
    }
    // Bounded serializer avoids a second unbounded copy for very large history indexes.
    struct Bounded(Vec<u8>);
    impl std::io::Write for Bounded {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            if self.0.len().saturating_add(b.len()) > CACHE_LIMIT as usize {
                return Err(std::io::Error::other("display cache budget"));
            }
            self.0.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut out = Bounded(Vec::new());
    if serde_json::to_writer(&mut out, snapshot).is_ok() {
        let _ = crate::config_io::atomic_write(path, &out.0);
    }
}
#[cfg(not(unix))]
fn persist(_path: &Path, _snapshot: &Snapshot) {}

fn scan(path: &Path, floor: u64, source: &str) -> Result<Vec<Entry>, String> {
    let mut reader = BufReader::new(File::open(path).map_err(|e| e.to_string())?);
    reader
        .seek(SeekFrom::Start(floor))
        .map_err(|e| e.to_string())?;
    let mut offset = floor;
    let mut line = String::new();
    let mut entries = Vec::new();
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if bytes == 0 {
            break;
        }
        let start = offset;
        offset += bytes as u64;
        if let Ok(event) = serde_json::from_str::<Event>(&line) {
            if event.visible(source) {
                entries.push(Entry {
                    offset: start,
                    ts: event.ts,
                });
            }
        }
    }
    Ok(entries)
}

#[derive(Default, Deserialize)]
struct Payload {
    session_key: Option<String>,
    channel: Option<String>,
    sender_id: Option<String>,
    tool_call_count: Option<u64>,
    tool_calls: Option<Vec<serde::de::IgnoredAny>>,
    #[serde(default, rename = "text", deserialize_with = "has_text")]
    text: bool,
}
fn has_text<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    struct Text;
    impl<'de> serde::de::Visitor<'de> for Text {
        type Value = bool;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("text or null")
        }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<bool, E> {
            Ok(!v.is_empty())
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<bool, E> {
            Ok(false)
        }
    }
    d.deserialize_any(Text)
}
#[derive(Default)]
struct Event {
    kind: String,
    ts: String,
    payload: Payload,
}
impl Event {
    fn visible(&self, source: &str) -> bool {
        let p = &self.payload;
        let scope = p
            .session_key
            .as_deref()
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| match (p.channel.as_deref(), p.sender_id.as_deref()) {
                (Some(c), Some(s)) if !c.is_empty() && !s.is_empty() => format!("{c}:{s}"),
                _ => "local".into(),
            });
        scope == source
            && p.text
            && (self.kind == "user_message"
                || (self.kind == "assistant_message"
                    && p.tool_call_count.unwrap_or(0) == 0
                    && p.tool_calls.as_ref().is_none_or(Vec::is_empty)))
    }
}
impl<'de> Deserialize<'de> for Event {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Event;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("transcript event")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Event, M::Error> {
                let mut event = Event::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "type" => event.kind = map.next_value()?,
                        "ts" => event.ts = map.next_value()?,
                        "payload"
                            if event.kind.is_empty()
                                || matches!(
                                    event.kind.as_str(),
                                    "user_message" | "assistant_message"
                                ) =>
                        {
                            event.payload = map.next_value()?
                        }
                        _ => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(event)
            }
        }
        d.deserialize_map(Visitor)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn event(text: &str) -> String {
        serde_json::json!({"ts":"2026-11-27T12:00:00","type":"user_message","payload":{"text":text}}).to_string()+"\n"
    }
    #[test]
    fn persistent_index_reuses_unchanged_files_and_invalidates_changed_metadata() {
        let scratch = crate::config_io::Scratch::new(&std::env::temp_dir()).unwrap();
        let home = &scratch.0;
        let logs = home.join("transcripts");
        fs::create_dir(&logs).unwrap();
        let file = logs.join("2026-11-27.jsonl");
        fs::write(&file, event("private body not in cache")).unwrap();
        let paths = vec![file.clone()];
        let floors = BTreeMap::new();
        assert_eq!(
            load(home, paths.clone(), &floors, "local", false)
                .unwrap()
                .len(),
            1
        );
        let path = cache_path(home, "local");
        let saved = fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&saved).contains("private body"));
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(
            load(home, paths.clone(), &floors, "local", false)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(before, fs::metadata(&path).unwrap().modified().unwrap());
        fs::write(&file, event("second")).unwrap();
        assert_eq!(
            load(home, paths.clone(), &floors, "local", false)
                .unwrap()
                .len(),
            1
        );
        assert_ne!(saved, fs::read(&path).unwrap());
        fs::write(&path, "broken cache").unwrap();
        assert_eq!(
            load(home, paths.clone(), &floors, "local", false)
                .unwrap()
                .len(),
            1
        );
        let mut floors = BTreeMap::new();
        floors.insert(
            "2026-11-27.jsonl".into(),
            fs::metadata(&file).unwrap().len(),
        );
        assert!(load(home, paths, &floors, "local", false)
            .unwrap()
            .is_empty());
        assert!(load(home, vec![], &floors, "local", false)
            .unwrap()
            .is_empty());
        let cache: Snapshot = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert!(cache.files.is_empty());
    }
    #[test]
    fn parser_handles_reordered_fields_and_ignores_non_chat_payloads() {
        let chat =
            r#"{"payload":{"text":"ok","session_key":"local"},"type":"user_message","ts":"time"}"#;
        assert!(serde_json::from_str::<Event>(chat)
            .unwrap()
            .visible("local"));
        let tool = r#"{"type":"tool_result","payload":{"text":{"not":"a string"}}}"#;
        assert!(!serde_json::from_str::<Event>(tool)
            .unwrap()
            .visible("local"));
        let internal = r#"{"type":"assistant_message","payload":{"text":"hidden","tool_call_count":0,"tool_calls":[{"arguments":{"large":"ignored"}}]}}"#;
        assert!(!serde_json::from_str::<Event>(internal)
            .unwrap()
            .visible("local"));
    }
    #[test]
    fn unavailable_cache_never_blocks_history() {
        let scratch = crate::config_io::Scratch::new(&std::env::temp_dir()).unwrap();
        let home = &scratch.0;
        fs::create_dir(home.join("transcripts")).unwrap();
        let file = home.join("transcripts/2026-11-27.jsonl");
        fs::write(&file, event("visible")).unwrap();
        fs::write(home.join("cache"), "blocked").unwrap();
        assert_eq!(
            load(home, vec![file], &BTreeMap::new(), "local", false)
                .unwrap()
                .len(),
            1
        );
    }
}
