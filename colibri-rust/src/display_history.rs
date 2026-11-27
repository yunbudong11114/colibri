//! Paged conversation text. Independent from the model's recoverable context.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

type CachedIndex = (String, Arc<Vec<Record>>);
static INDEX: OnceLock<Mutex<Option<CachedIndex>>> = OnceLock::new();

mod index;

const PAGE_MESSAGES: usize = 8;
#[derive(Clone)]
struct Record {
    file: PathBuf,
    offset: u64,
}

pub struct DisplayHistory {
    home: PathBuf,
    source: String,
}
impl DisplayHistory {
    pub fn new(home: PathBuf, source: impl Into<String>) -> Self {
        Self {
            home,
            source: source.into(),
        }
    }

    pub fn page(&self, direction: &str, current: usize) -> Result<Value, String> {
        self.page_inner(direction, current, false)
            .or_else(|_| self.page_inner(direction, current, true))
    }

    fn page_inner(&self, direction: &str, current: usize, rebuild: bool) -> Result<Value, String> {
        let offsets: BTreeMap<String, u64> =
            match fs::read(self.home.join("chat-display-cursor.json")) {
                Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string())?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
                Err(e) => return Err(e.to_string()),
            };
        let dir = self.home.join("transcripts");
        let mut files = match fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|v| v == "jsonl"))
                .collect::<Vec<_>>(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.to_string()),
        };
        files.sort();
        let mut signature = format!("{:?}:{}:{offsets:?}", self.home, self.source);
        for path in &files {
            let meta = fs::metadata(path).map_err(|e| e.to_string())?;
            signature += &format!("{path:?}:{}:{:?};", meta.len(), meta.modified().ok());
        }
        let cache = INDEX.get_or_init(|| Mutex::new(None));
        let cached = cache
            .lock()
            .map_err(|_| "历史索引不可用")?
            .as_ref()
            .filter(|(key, _)| !rebuild && key == &signature)
            .map(|(_, records)| Arc::clone(records));
        let records = if let Some(records) = cached {
            records
        } else {
            let records = Arc::new(index::load(
                &self.home,
                files,
                &offsets,
                &self.source,
                rebuild,
            )?);
            *cache.lock().map_err(|_| "历史索引不可用")? = Some((signature, Arc::clone(&records)));
            records
        };
        let total = records.len();
        let last = total.saturating_sub(PAGE_MESSAGES);
        let start = match direction {
            "first" => 0,
            "before" => current.saturating_sub(PAGE_MESSAGES / 2),
            "after" => current.saturating_add(PAGE_MESSAGES / 2).min(last),
            _ => last,
        }
        .min(last);
        let result = (|| -> Result<Vec<Value>, String> {
            let mut messages = Vec::new();
            for record in records.iter().skip(start).take(PAGE_MESSAGES) {
                let mut reader =
                    BufReader::new(File::open(&record.file).map_err(|e| e.to_string())?);
                reader
                    .seek(SeekFrom::Start(record.offset))
                    .map_err(|e| e.to_string())?;
                let mut line = String::new();
                reader.read_line(&mut line).map_err(|e| e.to_string())?;
                let event: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
                let payload = &event["payload"];
                if crate::session_history::event_source(payload) != self.source
                    || !(event["type"] == "user_message"
                        || (event["type"] == "assistant_message"
                            && payload["tool_call_count"].as_u64().unwrap_or(0) == 0
                            && payload["tool_calls"].as_array().is_none_or(Vec::is_empty)))
                {
                    return Err("历史索引已失效".into());
                }
                let raw_text = event["payload"]["text"].as_str().unwrap_or_default();
                let text = if event["type"] == "user_message" {
                    crate::session_history::strip_attachment_paths(raw_text)
                } else {
                    raw_text.to_owned()
                };
                messages.push(json!({"id":format!("{}:{}",record.file.file_name().unwrap().to_string_lossy(),record.offset),
                "role":if event["type"]=="user_message" {"user"} else {"assistant"}, "text":text}));
            }
            Ok(messages)
        })();
        let messages = match result {
            Ok(v) => v,
            Err(e) => return Err(e),
        };
        Ok(
            json!({"history":messages,"start":start,"total":total,"has_before":start>0,"has_after":start+messages.len()<total}),
        )
    }
}
