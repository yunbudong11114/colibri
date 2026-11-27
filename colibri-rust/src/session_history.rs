use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use crate::config::colibri_home;
use crate::messages::{Message, ToolCall};
use crate::model_history::{ContextUpdate, ModelHistory};

const ATTACHMENT_MARKER: &str = "Attachments saved locally:";

pub struct TranscriptHistoryLoader {
    transcript_dir: PathBuf,
}

impl TranscriptHistoryLoader {
    pub fn for_home(home: PathBuf) -> Self {
        Self {
            transcript_dir: home.join("transcripts"),
        }
    }
    pub fn default() -> Self {
        Self::for_home(colibri_home())
    }

    /// Replay only the latest model stream, without display limits or prefix rebasing.
    pub fn load_model(&self, source: &str) -> ModelHistory {
        let mut files = fs::read_dir(&self.transcript_dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect::<Vec<_>>();
        files.sort();
        // First pass retains only stream identifiers and ordering, not conversation bodies.
        let mut streams = BTreeMap::new();
        for (order, line) in model_lines(&files).enumerate() {
            let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
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
            let started = stream
                .split('-')
                .nth(1)
                .and_then(|v| v.parse::<u128>().ok())
                .unwrap_or(0);
            let state =
                streams
                    .entry(stream.to_owned())
                    .or_insert((String::new(), started, 0, false));
            state.0 = event["ts"].as_str().unwrap_or_default().to_owned();
            state.2 = order;
            state.3 |= kind == "user_message"
                || payload["history"]["messages"]
                    .as_array()
                    .is_some_and(|messages| messages.iter().any(|m| m["role"] == "user"));
        }
        let selected = streams
            .into_iter()
            .filter(|(_, state)| state.3)
            .max_by(|(_, a), (_, b)| (&a.0, a.1, a.2).cmp(&(&b.0, b.1, b.2)))
            .map(|(stream, _)| stream);
        let Some(selected) = selected else {
            return ModelHistory::default();
        };
        let mut history = replay_model_history(model_lines(&files), source, &selected);
        history.repair_tool_results();
        history
    }
}

// Stream files twice rather than retaining every JSONL record or every model stream.
fn model_lines(files: &[PathBuf]) -> impl Iterator<Item = String> + '_ {
    files
        .iter()
        .filter_map(|path| fs::File::open(path).ok())
        .flat_map(|file| BufReader::new(file).lines().map_while(Result::ok))
}

pub(crate) fn strip_attachment_paths(text: &str) -> String {
    text.split_once(ATTACHMENT_MARKER)
        .map(|(head, _)| head.trim_end().to_string())
        .unwrap_or_else(|| text.to_string())
}

pub(crate) fn event_source(payload: &serde_json::Value) -> String {
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

fn replay_model_history(
    lines: impl IntoIterator<Item = String>,
    source: &str,
    selected_stream: &str,
) -> ModelHistory {
    let mut history = ModelHistory::default();
    for line in lines {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
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
        if stream != selected_stream {
            continue;
        }
        match kind {
            "model_seed" | "context_compact" => {
                if let Ok(checkpoint) =
                    serde_json::from_value::<ModelHistory>(payload["history"].clone())
                {
                    history = checkpoint;
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
    history
        .context_updates
        .sort_by_key(|update| update.before_message);
    history
}
