use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::messages::MediaPart;
use crate::permissions::{PermissionPrompter, PermissionRequest};

/// Channel-agnostic inbound work item. Media bytes stay in `media_refs` until resolve.
#[derive(Clone, Debug)]
pub struct InboundEnvelope {
    pub channel: String,
    pub sender_id: String,
    pub text: String,
    pub message_id: String,
    pub media: Vec<MediaPart>,
    pub media_refs: Vec<serde_json::Value>,
    pub context: BTreeMap<String, String>,
}

impl InboundEnvelope {
    pub fn session_key(&self) -> String {
        format!("{}:{}", self.channel, self.sender_id)
    }

    pub fn text_only(&self) -> bool {
        self.media.is_empty() && self.media_refs.is_empty() && !self.text.trim().is_empty()
    }

    pub fn context_token(&self) -> &str {
        self.context.get("context_token").map(String::as_str).unwrap_or("")
    }
}

/// Serial outbound path for one recipient (ack / text / media / permission prompt).
pub trait OutboundSink: Send + Sync {
    fn send_text(&self, text: &str) -> Result<(), String>;
    fn send_media(&self, media: &MediaPart) -> Result<(), String>;

    fn send_ack(&self, text: &str) {
        let _ = self.send_text(text);
    }

    fn send_permission_prompt(&self, text: &str) -> Result<(), String> {
        self.send_text(text)
    }
}

/// Transport-agnostic permission UX: prompt on channel, wait for numeric text reply.
pub struct ChannelTextPermissionPrompter {
    outbound: Arc<dyn OutboundSink>,
    sender_id: String,
    waiters: Arc<Mutex<HashMap<String, mpsc::SyncSender<String>>>>,
    timeout_seconds: u64,
}

impl ChannelTextPermissionPrompter {
    pub fn new(
        outbound: Arc<dyn OutboundSink>,
        sender_id: String,
        waiters: Arc<Mutex<HashMap<String, mpsc::SyncSender<String>>>>,
        timeout_seconds: u64,
    ) -> Self {
        Self {
            outbound,
            sender_id,
            waiters,
            timeout_seconds,
        }
    }
}

impl PermissionPrompter for ChannelTextPermissionPrompter {
    fn confirm(&mut self, request: PermissionRequest) -> String {
        let (tx, rx) = mpsc::sync_channel(1);
        if let Ok(mut waiters) = self.waiters.lock() {
            waiters.insert(self.sender_id.clone(), tx);
        }
        let prompt = format_channel_permission_prompt(&request);
        if self.outbound.send_permission_prompt(&prompt).is_err() {
            if let Ok(mut waiters) = self.waiters.lock() {
                waiters.remove(&self.sender_id);
            }
            return "0".to_string();
        }
        let reply = rx
            .recv_timeout(Duration::from_secs(self.timeout_seconds))
            .ok();
        if let Ok(mut waiters) = self.waiters.lock() {
            waiters.remove(&self.sender_id);
        }
        reply
            .as_deref()
            .map(parse_permission_choice)
            .unwrap_or_else(|| "0".to_string())
    }
}

pub fn deliver_text_waiter(
    waiters: &Arc<Mutex<HashMap<String, mpsc::SyncSender<String>>>>,
    sender_id: &str,
    text: &str,
) -> bool {
    let cleaned = text.trim();
    if cleaned.is_empty() {
        return false;
    }
    let waiter = waiters
        .lock()
        .ok()
        .and_then(|map| map.get(sender_id).cloned());
    let Some(waiter) = waiter else {
        return false;
    };
    waiter.try_send(cleaned.to_string()).is_ok()
}

pub fn format_channel_permission_prompt(request: &PermissionRequest) -> String {
    let mut lines = vec![format!("Colibri wants to run {}.", request.tool_name)];
    for line in permission_detail_lines(request) {
        if request.subject_kind == "file_path" && line.starts_with("file: ") {
            let path = line
                .strip_prefix("file: ")
                .and_then(|text| text.split_once(' ').map(|(_, path)| path))
                .unwrap_or_else(|| line.strip_prefix("file: ").unwrap_or(&line));
            lines.push(format!("path: {}", path));
        } else {
            lines.push(line);
        }
    }
    lines.push(String::new());
    lines.push("choose:".to_string());
    match request.subject_kind.as_str() {
        "shell" => lines.extend(
            [
                "1. once",
                "2. session-command",
                "3. session-executable",
                "4. user-command",
                "5. user-executable",
                "0. deny",
            ]
            .into_iter()
            .map(str::to_string),
        ),
        "file_path" => lines.extend(
            ["1. once", "2. session-dir", "4. user-dir", "0. deny"]
                .into_iter()
                .map(str::to_string),
        ),
        _ => lines.extend(
            ["1. once", "2. session", "4. user", "0. deny"]
                .into_iter()
                .map(str::to_string),
        ),
    }
    lines.join("\n")
}

pub fn parse_permission_choice(reply: &str) -> String {
    let first = reply
        .trim()
        .split_whitespace()
        .next()
        .unwrap_or("0");
    if matches!(first, "0" | "1" | "2" | "3" | "4" | "5") {
        first.to_string()
    } else {
        "0".to_string()
    }
}

fn permission_detail_lines(request: &PermissionRequest) -> Vec<String> {
    match request.subject_kind.as_str() {
        "shell" => vec![format!(
            "shell: {}",
            request.shell_command.as_deref().unwrap_or("")
        )],
        "file_path" => {
            let mut lines = vec![format!(
                "file: {} {}",
                request.tool_name,
                request.file_path.as_deref().unwrap_or("")
            )];
            if let Some(command) = &request.shell_command {
                lines.push(format!("command: {}", command));
            }
            if request.tool_name == "files.write" {
                lines.push(permission_content_summary(request.arguments.get("content")));
            }
            lines
        }
        _ if request.tool_name == "memory.write" => {
            let mut lines = vec![format!("tool: {}", request.tool_name)];
            if let Some(target) = request
                .arguments
                .get("file")
                .or_else(|| request.arguments.get("topic"))
            {
                lines.push(format!("file: {}", target));
            }
            if let Some(mode) = request.arguments.get("mode") {
                lines.push(format!("mode: {}", mode));
            }
            lines.push(permission_content_summary(request.arguments.get("content")));
            lines
        }
        _ => {
            let pairs = request
                .arguments
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(",");
            vec![format!("tool: {} {{{pairs}}}", request.tool_name)]
        }
    }
}

fn permission_content_summary(value: Option<&String>) -> String {
    let value = value.map(String::as_str).unwrap_or("");
    let char_count = value.chars().count();
    let byte_count = value.len();
    let mut preview = value.replace('\n', "\\n");
    if preview.chars().count() > 40 {
        preview = preview.chars().take(37).collect::<String>() + "...";
    }
    format!("content: {char_count} chars, {byte_count} bytes, preview='{preview}'")
}
