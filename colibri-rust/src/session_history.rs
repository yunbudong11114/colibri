use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use crate::config::{colibri_home, SessionConfig};
use crate::messages::{Message, ToolCall};
use crate::model_history::{ContextUpdate, ModelHistory};

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
        self.load_after(&BTreeMap::new())
    }

    pub(crate) fn load_after(&self, offsets: &BTreeMap<String, u64>) -> Vec<Message> {
        if self.message_limit < 2 || self.char_limit == 0 || self.scan_bytes == 0 {
            return Vec::new();
        }
        self.text_messages(&self.recent_lines(offsets), false)
    }

    fn text_messages(&self, lines: &[String], model_text: bool) -> Vec<Message> {
        let turns = completed_turns(lines, model_text);
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

    /// Model recovery is separate from the display-only conversation loader.
    pub fn load_model(&self, source: &str) -> ModelHistory {
        if self.message_limit == 0 || self.char_limit == 0 || self.scan_bytes == 0 {
            return ModelHistory::default();
        }
        let lines = self.recent_lines(&BTreeMap::new());
        if let Some(history) = replay_model_history(&lines, source).and_then(|history| {
            history.bounded(self.message_limit, self.char_limit, self.scan_bytes)
        }) {
            return history;
        }
        let filtered = lines
            .into_iter()
            .filter(|line| {
                parse_event(line).is_some_and(|(_, payload)| source_key(&payload) == source)
            })
            .collect::<Vec<_>>();
        ModelHistory {
            messages: self.text_messages(&filtered, true),
            ..Default::default()
        }
    }

    fn recent_lines(&self, offsets: &BTreeMap<String, u64>) -> Vec<String> {
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
            let floor = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| offsets.get(name))
                .copied()
                .unwrap_or(0);
            let read_size = remaining.min(size.saturating_sub(floor));
            if read_size == 0 {
                continue;
            }
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

fn completed_turns(lines: &[String], model_text: bool) -> Vec<(String, String)> {
    let mut pending: BTreeMap<String, VecDeque<String>> = BTreeMap::new();
    let mut completed = Vec::new();
    for line in lines {
        let Some((event_type, payload)) = parse_event(line) else {
            continue;
        };
        let text = if model_text && event_type == "user_message" {
            payload.get("model_text").or_else(|| payload.get("text"))
        } else {
            payload.get("text")
        };
        let Some(text) = text.filter(|value| !value.trim().is_empty()) else {
            continue;
        };
        let source = source_key(&payload);
        if event_type == "user_message" {
            let cleaned = if model_text {
                text.clone()
            } else {
                strip_attachment_paths(text)
            };
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

fn event_source(payload: &serde_json::Value) -> String {
    if let Some(key) = payload["session_key"]
        .as_str()
        .filter(|key| !key.is_empty())
    {
        return key.to_string();
    }
    match (payload["channel"].as_str(), payload["sender_id"].as_str()) {
        (Some(channel), Some(sender)) if !channel.is_empty() && !sender.is_empty() => {
            format!("{channel}:{sender}")
        }
        _ => "local".into(),
    }
}

struct ReplayedHistory {
    timestamp: String,
    started: u128,
    order: usize,
    history: ModelHistory,
}

fn replay_model_history(lines: &[String], source: &str) -> Option<ModelHistory> {
    let mut streams: BTreeMap<String, ReplayedHistory> = BTreeMap::new();
    for (order, line) in lines.iter().enumerate() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let payload = &event["payload"];
        if event_source(payload) != source {
            continue;
        }
        let Some(stream) = payload["model_stream"].as_str() else {
            continue;
        };
        let kind = event["type"].as_str().unwrap_or_default();
        if !matches!(
            kind,
            "model_seed"
                | "user_message"
                | "assistant_message"
                | "tool_result"
                | "runtime_context"
                | "runtime_context_rebased"
                | "context_compact"
        ) {
            continue;
        }
        let state = streams
            .entry(stream.to_string())
            .or_insert_with(|| ReplayedHistory {
                timestamp: String::new(),
                started: stream
                    .split('-')
                    .nth(1)
                    .and_then(|part| part.parse().ok())
                    .unwrap_or(0),
                order: 0,
                history: ModelHistory::default(),
            });
        state.timestamp = event["ts"].as_str().unwrap_or_default().to_string();
        state.order = order;
        let history = &mut state.history;
        match kind {
            "model_seed" | "context_compact" => {
                if let Ok(checkpoint) =
                    serde_json::from_value::<ModelHistory>(payload["history"].clone())
                {
                    *history = checkpoint;
                } else if kind == "context_compact" {
                    let kept = payload["kept_messages"].as_u64().unwrap_or(0) as usize;
                    let start = history.messages.len().saturating_sub(kept);
                    history.retain_from(start);
                    history.summary = payload["summary"].as_str().unwrap_or_default().to_string();
                }
            }
            "user_message" => {
                if let Some(text) = payload["model_text"]
                    .as_str()
                    .or_else(|| payload["text"].as_str())
                {
                    history.messages.push(Message::new("user", text));
                }
            }
            "assistant_message" => {
                if history
                    .messages
                    .iter()
                    .any(|message| message.role == "user")
                {
                    let mut message =
                        Message::new("assistant", payload["text"].as_str().unwrap_or_default());
                    message.tool_calls =
                        serde_json::from_value::<Vec<ToolCall>>(payload["tool_calls"].clone())
                            .unwrap_or_default();
                    history.messages.push(message);
                }
            }
            "tool_result" => {
                if let Some(id) = payload["id"].as_str() {
                    let text = payload["text"].as_str().unwrap_or_default();
                    let content = if payload["model_history_truncated"].as_bool() == Some(true) {
                        format!("recovery_output_truncated: This tool finished, but its full output exceeded the recovery budget. Do not repeat the operation just to recover its output; use a targeted read if needed. Recorded output:\n{text}")
                    } else {
                        payload["model_text"]
                            .as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| {
                                if payload["ok"].as_bool() == Some(false) {
                                    format!(
                                        "{}: {text}",
                                        payload["error_type"].as_str().unwrap_or("tool_error")
                                    )
                                } else {
                                    text.to_string()
                                }
                            })
                    };
                    let mut message = Message::new("tool", content);
                    message.tool_call_id = Some(id.to_string());
                    history.messages.push(message);
                }
            }
            "runtime_context" => {
                if let Ok(mut update) =
                    serde_json::from_value::<ContextUpdate>(payload["update"].clone())
                {
                    update.before_message = update.before_message.min(history.messages.len());
                    history.context_updates.push(update);
                }
            }
            "runtime_context_rebased" => {
                if let Ok(updates) = serde_json::from_value(payload["updates"].clone()) {
                    history.context_updates = updates;
                }
            }
            _ => {}
        }
    }
    streams
        .into_values()
        .filter(|state| {
            state
                .history
                .messages
                .iter()
                .any(|message| message.role == "user")
        })
        .max_by(|left, right| {
            (&left.timestamp, left.started, left.order).cmp(&(
                &right.timestamp,
                right.started,
                right.order,
            ))
        })
        .map(|state| {
            let mut history = state.history;
            history
                .context_updates
                .retain(|update| update.before_message <= history.messages.len());
            history
                .context_updates
                .sort_by_key(|update| update.before_message);
            history
        })
}
