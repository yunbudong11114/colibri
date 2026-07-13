use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use crate::config::{colibri_home, SessionConfig};
use crate::messages::Message;

const ATTACHMENT_MARKER: &str = "Attachments saved locally:";

pub struct TranscriptHistoryLoader {
    transcript_dir: PathBuf,
    message_limit: usize,
    char_limit: usize,
    scan_bytes: usize,
}

impl TranscriptHistoryLoader {
    pub fn new(
        colibri_home: PathBuf,
        message_limit: usize,
        char_limit: usize,
        scan_bytes: usize,
    ) -> Self {
        Self {
            transcript_dir: colibri_home.join("transcripts"),
            message_limit,
            char_limit,
            scan_bytes,
        }
    }

    pub fn default(config: &SessionConfig) -> Self {
        Self::new(
            colibri_home(),
            config.restore_message_limit,
            config.restore_char_limit,
            config.restore_scan_bytes,
        )
    }

    pub fn load(&self) -> Vec<Message> {
        if self.message_limit < 2 || self.char_limit == 0 || self.scan_bytes == 0 {
            return Vec::new();
        }
        let turns = completed_turns(&self.recent_lines());
        let mut selected = Vec::new();
        let mut selected_chars = 0usize;
        for (user, assistant) in turns.into_iter().rev() {
            let turn_chars = user.chars().count() + assistant.chars().count();
            if selected.len() * 2 + 2 > self.message_limit {
                break;
            }
            if selected_chars + turn_chars > self.char_limit {
                break;
            }
            selected.push((user, assistant));
            selected_chars += turn_chars;
        }
        let mut messages = Vec::new();
        for (user, assistant) in selected.into_iter().rev() {
            messages.push(Message::new("user", user));
            messages.push(Message::new("assistant", assistant));
        }
        messages
    }

    fn recent_lines(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&self.transcript_dir) else {
            return Vec::new();
        };
        let mut files = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("jsonl"))
            .collect::<Vec<_>>();
        files.sort();
        files.reverse();
        let mut remaining = self.scan_bytes as u64;
        let mut chunks = Vec::new();
        for path in files {
            if remaining == 0 {
                break;
            }
            let Ok(mut file) = fs::File::open(&path) else {
                continue;
            };
            let Ok(size) = file.metadata().map(|meta| meta.len()) else {
                continue;
            };
            let read_size = remaining.min(size);
            let start = size.saturating_sub(read_size);
            let mut starts_mid_line = false;
            if start > 0 && file.seek(SeekFrom::Start(start - 1)).is_ok() {
                let mut one = [0u8; 1];
                starts_mid_line = file.read_exact(&mut one).is_ok() && one[0] != b'\n';
            }
            if file.seek(SeekFrom::Start(start)).is_err() {
                continue;
            }
            let mut data = vec![0u8; read_size as usize];
            if file.read_exact(&mut data).is_err() {
                continue;
            }
            remaining -= read_size;
            let mut lines = String::from_utf8_lossy(&data)
                .lines()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            if starts_mid_line && !lines.is_empty() {
                lines.remove(0);
            }
            chunks.push(lines);
        }
        chunks.reverse();
        chunks.into_iter().flatten().collect()
    }
}

fn completed_turns(lines: &[String]) -> Vec<(String, String)> {
    let mut pending: BTreeMap<String, VecDeque<String>> = BTreeMap::new();
    let mut completed = Vec::new();
    for line in lines {
        let Some((event_type, payload)) = parse_event(line) else {
            continue;
        };
        let Some(text) = payload.get("text").filter(|value| !value.trim().is_empty()) else {
            continue;
        };
        let source = source_key(&payload);
        if event_type == "user_message" {
            let cleaned = strip_attachment_paths(text);
            if !cleaned.is_empty() {
                pending.entry(source).or_default().push_back(cleaned);
            }
        } else if event_type == "assistant_message"
            && payload.get("tool_call_count").map(String::as_str) == Some("0")
        {
            if let Some(queue) = pending.get_mut(&source) {
                if let Some(user) = queue.pop_front() {
                    completed.push((user, text.clone()));
                }
            }
        }
    }
    completed
}

fn parse_event(line: &str) -> Option<(String, BTreeMap<String, String>)> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let event_type = value.get("type")?.as_str()?.to_string();
    let payload = value.get("payload")?.as_object()?;
    let mut out = BTreeMap::new();
    for (key, value) in payload {
        if let Some(text) = value.as_str() {
            out.insert(key.clone(), text.to_string());
        } else if let Some(number) = value.as_u64() {
            out.insert(key.clone(), number.to_string());
        } else if let Some(boolean) = value.as_bool() {
            out.insert(key.clone(), boolean.to_string());
        }
    }
    Some((event_type, out))
}

fn source_key(payload: &BTreeMap<String, String>) -> String {
    if let Some(session_key) = payload.get("session_key").filter(|value| !value.is_empty()) {
        return session_key.clone();
    }
    match (payload.get("channel"), payload.get("sender_id")) {
        (Some(channel), Some(sender_id)) if !channel.is_empty() && !sender_id.is_empty() => {
            format!("{}:{}", channel, sender_id)
        }
        _ => "local".to_string(),
    }
}

fn strip_attachment_paths(text: &str) -> String {
    text.split_once(ATTACHMENT_MARKER)
        .map(|(head, _)| head.trim_end().to_string())
        .unwrap_or_else(|| text.to_string())
}
