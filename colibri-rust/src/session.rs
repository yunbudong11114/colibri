use crate::config::AgentConfig;
use crate::memory::MemoryContext;
use crate::messages::{AgentResponse, MediaPart, Message, ModelLimits, ToolCall, ToolResult};
use crate::model::ModelClient;
use crate::permissions::{PermissionPolicy, PermissionPrompter};
use crate::skills::relevant_skill_context;
use crate::tools::{run_tool_map, string_arguments, tool_info, tool_specs_for_config, ToolContext};
use crate::transcript::TranscriptWriter;
use crate::vision::analyze_image;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub const SYSTEM_PROMPT: &str = "Your name is Colibri. You are a lightweight personal agent running on the CardputerZero, a multi-interface device powered by the CM0 chip. Prefer short, practical responses and respect low memory, battery, and tool limits. ";
const SUMMARY_HEADER: &str = "Compacted conversation summary:";
const COMPACT_SYSTEM_PROMPT: &str =
    "You are a helpful AI assistant tasked with summarizing conversations.";

pub struct AgentSession {
    pub config: Arc<AgentConfig>,
    model: Arc<Mutex<Box<dyn ModelClient>>>,
    pub messages: Vec<Message>,
    pub summary: String,
    transcript: Option<Arc<Mutex<TranscriptWriter>>>,
    transcript_metadata: BTreeMap<String, String>,
    media_sender: Option<Arc<dyn Fn(MediaPart) -> Result<(), String> + Send + Sync>>,
    history_loader: Option<Box<dyn Fn() -> Vec<Message> + Send>>,
    history_loaded: bool,
}

impl AgentSession {
    pub fn new(config: AgentConfig, model: Box<dyn ModelClient>) -> Self {
        Self::new_with_transcript_metadata(config, model, BTreeMap::new())
    }

    pub fn new_with_transcript_metadata(
        config: AgentConfig,
        model: Box<dyn ModelClient>,
        transcript_metadata: BTreeMap<String, String>,
    ) -> Self {
        let config = Arc::new(config);
        let transcript = owned_transcript(&config);
        Self::from_shared(
            config,
            Arc::new(Mutex::new(model)),
            transcript,
            transcript_metadata,
        )
    }

    pub fn from_shared(
        config: Arc<AgentConfig>,
        model: Arc<Mutex<Box<dyn ModelClient>>>,
        transcript: Option<Arc<Mutex<TranscriptWriter>>>,
        transcript_metadata: BTreeMap<String, String>,
    ) -> Self {
        Self {
            config,
            model,
            messages: Vec::new(),
            summary: String::new(),
            transcript,
            transcript_metadata,
            media_sender: None,
            history_loader: None,
            history_loaded: false,
        }
    }

    pub fn with_history_loader(mut self, loader: Box<dyn Fn() -> Vec<Message> + Send>) -> Self {
        self.history_loader = Some(loader);
        self
    }

    pub fn submit(&mut self, text: &str) -> Result<AgentResponse, String> {
        self.submit_with_permission_prompter(text, None)
    }

    pub fn submit_with_media(
        &mut self,
        text: &str,
        media: Vec<MediaPart>,
    ) -> Result<AgentResponse, String> {
        self.submit_inner(text, media, None)
    }

    pub fn set_media_sender(
        &mut self,
        sender: Option<Arc<dyn Fn(MediaPart) -> Result<(), String> + Send + Sync>>,
    ) {
        self.media_sender = sender;
    }

    pub fn submit_with_permission_prompter(
        &mut self,
        text: &str,
        prompter: Option<&mut dyn PermissionPrompter>,
    ) -> Result<AgentResponse, String> {
        self.submit_inner(text, Vec::new(), prompter)
    }

    pub fn submit_with_media_and_permission_prompter(
        &mut self,
        text: &str,
        media: Vec<MediaPart>,
        prompter: Option<&mut dyn PermissionPrompter>,
    ) -> Result<AgentResponse, String> {
        self.submit_inner(text, media, prompter)
    }

    fn submit_inner(
        &mut self,
        text: &str,
        media: Vec<MediaPart>,
        prompter: Option<&mut dyn PermissionPrompter>,
    ) -> Result<AgentResponse, String> {
        self.restore_history_once();
        let user_text = user_text_with_media(text, &media);
        let bounded = bound_text(&user_text, self.config.session.model_input_char_limit);
        self.messages.push(Message::new("user", &bounded));
        self.write_transcript(
            "user_message",
            serde_json::json!({
                "text": bounded,
                "media": media.iter().map(media_payload).collect::<Vec<_>>()
            }),
        );
        self.compact_if_needed();

        let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
        let analyzer_config = Arc::clone(&self.config);
        let analyzer = Arc::new(move |path: &Path, prompt: &str| {
            analyze_image(&analyzer_config, path, prompt)
        });
        let mut context =
            ToolContext::new(Arc::clone(&self.config), cwd).with_image_analyzer(analyzer);
        if let Some(sender) = &self.media_sender {
            context = context.with_media_sender(Arc::clone(sender));
        }
        let mut policy =
            PermissionPolicy::from_config(&self.config, context.cwd.clone(), prompter);
        let memory = MemoryContext::new(Arc::clone(&self.config)).load()?;
        if !memory.text.is_empty() {
            self.write_transcript(
                "memory_context",
                serde_json::json!({"files":memory.files,"truncated":memory.truncated}),
            );
        }
        let (skill_text, skill_names, _truncated) = relevant_skill_context(&bounded, &context);
        if !skill_text.is_empty() {
            self.write_transcript(
                "skill_recall",
                serde_json::json!({"skills":skill_names,"truncated":_truncated}),
            );
        }
        let tools = tool_specs_for_config(&self.config);

        for _ in 0..self.config.session.max_tool_rounds {
            let model_messages = self.model_messages(&memory.text, &skill_text);
            let response = {
                let mut model = self
                    .model
                    .lock()
                    .map_err(|_| "model lock poisoned".to_string())?;
                model.complete(
                    &model_messages,
                    &tools,
                    SYSTEM_PROMPT,
                    &ModelLimits {
                        timeout_seconds: self.config.model.timeout_seconds,
                        max_output_tokens: self.config.model.max_output_tokens,
                    },
                )?
            };
            let assistant_text = bound_text(&response.text, self.config.tools.max_result_chars);
            let mut assistant = Message::new("assistant", &assistant_text);
            assistant.tool_calls = response.tool_calls.clone();
            self.messages.push(assistant);
            self.write_transcript(
                "assistant_message",
                serde_json::json!({"text":assistant_text,"tool_call_count":response.tool_calls.len()}),
            );
            self.compact_if_needed();

            if response.tool_calls.is_empty() {
                return Ok(AgentResponse {
                    text: assistant_text,
                });
            }

            for call in response.tool_calls {
                let execution_arguments = string_arguments(&call.arguments);
                self.write_transcript(
                    "tool_call",
                    serde_json::json!({"id":call.id,"name":call.name,"arguments":call.arguments}),
                );
                let decision =
                    policy.decide(&tool_info(&call.name), &execution_arguments, &context);
                self.write_transcript(
                    "permission_decision",
                    serde_json::json!({
                        "tool_name":call.name,
                        "subject_kind":decision.subject_kind,
                        "decision":decision.decision,
                        "scope":decision.scope,
                        "allowed":decision.allowed,
                        "reason":decision.reason,
                        "shell_command":execution_arguments.get("command"),
                        "file_path":decision.file_path,
                        "file_root":decision.file_root
                    }),
                );
                let result = if decision.allowed {
                    let run_context = decision
                        .file_root
                        .as_ref()
                        .map(|root| context.with_allowed_file_root(std::path::PathBuf::from(root)))
                        .unwrap_or_else(|| context.clone());
                    run_tool_map(&call.name, &execution_arguments, &run_context)?
                } else {
                    crate::messages::ToolResult::error("permission_denied", denied_tool_text(&call))
                };
                let result = self.send_media_result_if_needed(result);
                self.write_transcript(
                    "tool_result",
                    serde_json::json!({
                        "id":call.id,
                        "name":call.name,
                        "ok":result.ok,
                        "error_type":result.error_type,
                        "text":bound_text(&result.text, self.config.tools.max_result_chars),
                        "truncated":result.truncated,
                        "media":result.media.as_ref().map(media_payload)
                    }),
                );
                let content = if result.ok {
                    result.text
                } else {
                    format!(
                        "{}: {}",
                        result
                            .error_type
                            .unwrap_or_else(|| "tool_error".to_string()),
                        result.text
                    )
                };
                self.messages.push(Message::tool(content, call.id));
                self.compact_if_needed();
            }
        }

        let text = format!(
            "Tool round limit reached after {} rounds. Recent tool results:\n{}",
            self.config.session.max_tool_rounds,
            self.messages
                .iter()
                .rev()
                .filter(|message| message.role == "tool")
                .take(3)
                .map(|message| message.content.clone())
                .collect::<Vec<_>>()
                .join("\n")
        );
        self.messages.push(Message::new("assistant", &text));
        self.write_transcript(
            "round_limit",
            serde_json::json!({"max_tool_rounds":self.config.session.max_tool_rounds,"text":text}),
        );
        Ok(AgentResponse { text })
    }

    fn send_media_result_if_needed(&self, result: ToolResult) -> ToolResult {
        if !result.ok {
            return result;
        }
        let Some(media) = result.media.clone() else {
            return result;
        };
        let Some(sender) = &self.media_sender else {
            return ToolResult::error(
                "media_unavailable",
                "No active channel can send files in this session",
            );
        };
        match sender(media) {
            Ok(()) => result,
            Err(error) => ToolResult::error("media_send_error", error),
        }
    }

    fn restore_history_once(&mut self) {
        if self.history_loaded {
            return;
        }
        self.history_loaded = true;
        let Some(loader) = &self.history_loader else {
            return;
        };
        self.messages.extend(loader());
    }

    fn model_messages(&self, memory_text: &str, skill_text: &str) -> Vec<Message> {
        let mut prefix = Vec::new();
        if !self.summary.is_empty() {
            prefix.push(Message::new(
                "system",
                format!("{}\n\n{}", SUMMARY_HEADER, self.summary),
            ));
        }
        if !memory_text.is_empty() {
            prefix.push(Message::new("system", memory_text));
        }
        if !skill_text.is_empty() {
            prefix.push(Message::new("system", skill_text));
        }
        budget_model_messages_parts(
            prefix,
            &self.messages,
            self.config.session.model_input_char_limit,
        )
        .0
    }

    fn compact_if_needed(&mut self) {
        if self.messages.len() < self.config.session.trigger_message_limit {
            return;
        }
        let removed_count = self
            .messages
            .len()
            .saturating_sub(self.config.session.recent_message_limit);
        if removed_count == 0 {
            return;
        }
        let messages_to_compact = std::mem::take(&mut self.messages);
        let compacted_len = messages_to_compact.len();
        let addition = self.compact_messages(&messages_to_compact);
        self.summary = append_summary(
            &self.summary,
            &addition,
            self.config.session.summary_max_chars,
        );
        self.messages = retained_messages_after_compact(
            messages_to_compact,
            self.config.session.recent_message_limit,
        );
        self.write_transcript(
            "context_compact",
            serde_json::json!({
                "removed_messages":removed_count,
                "compacted_messages":compacted_len,
                "kept_messages":self.messages.len(),
                "summary_chars":self.summary.chars().count()
            }),
        );
    }

    fn compact_messages(&mut self, messages: &[Message]) -> String {
        if self.config.session.model_compact && self.config.model.provider != "fake" {
            let prompt = compact_prompt_message(&self.summary, messages);
            if let Ok(mut model) = self.model.lock() {
                if let Ok(response) = model.complete(
                    &[prompt],
                    &[],
                    COMPACT_SYSTEM_PROMPT,
                    &ModelLimits {
                        timeout_seconds: self.config.model.timeout_seconds,
                        max_output_tokens: self.config.model.max_output_tokens,
                    },
                ) {
                    if response.tool_calls.is_empty() {
                        let formatted = format_model_summary(&response.text);
                        if !formatted.is_empty() {
                            return formatted;
                        }
                    }
                }
            }
        }
        summarize_messages(messages, 160)
    }

    fn write_transcript(&mut self, event_type: &str, mut payload: serde_json::Value) {
        let Some(transcript) = &self.transcript else {
            return;
        };
        if let Some(object) = payload.as_object_mut() {
            for (key, value) in &self.transcript_metadata {
                object.insert(key.clone(), serde_json::Value::String(value.clone()));
            }
        }
        if let Ok(mut writer) = transcript.lock() {
            let _ = writer.write(event_type, payload);
        }
    }
}

fn owned_transcript(config: &AgentConfig) -> Option<Arc<Mutex<TranscriptWriter>>> {
    if !config.session.transcript {
        return None;
    }
    TranscriptWriter::default_with_metadata_and_limits(
        BTreeMap::new(),
        config.session.transcript_retention_days,
        config.session.transcript_max_total_bytes,
    )
    .ok()
    .map(|writer| Arc::new(Mutex::new(writer)))
}

fn denied_tool_text(call: &ToolCall) -> String {
    if call.name == "shell.run" {
        if let Some(command) = call
            .arguments
            .get("command")
            .and_then(|value| value.as_str())
        {
            let command = command.trim();
            if !command.is_empty() {
                return format!("User denied shell.run: {}", command);
            }
        }
    }
    format!("User denied {}", call.name)
}

fn user_text_with_media(text: &str, media: &[MediaPart]) -> String {
    if media.is_empty() {
        return text.to_string();
    }
    let mut lines = vec!["Attachments saved locally:".to_string()];
    for (index, part) in media.iter().enumerate() {
        let label = if part.media_type.is_empty() {
            "file"
        } else {
            part.media_type.as_str()
        };
        let filename = if part.filename.is_empty() {
            part.path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            part.filename.clone()
        };
        let content_type = if part.content_type.is_empty() {
            String::new()
        } else {
            format!(", content_type={}", part.content_type)
        };
        lines.push(format!(
            "{}. {}: {} at {}{}",
            index + 1,
            label,
            filename,
            part.path.display(),
            content_type
        ));
    }
    let attachments = lines.join("\n");
    let trimmed = text.trim();
    if trimmed.is_empty() {
        attachments
    } else {
        format!("{}\n\n{}", trimmed, attachments)
    }
}

fn media_payload(media: &MediaPart) -> serde_json::Value {
    serde_json::json!({
        "type":media.media_type,
        "path":media.path,
        "filename":media.filename,
        "content_type":media.content_type,
        "caption":media.caption
    })
}

pub fn bound_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let suffix = "\n...[truncated]";
    let keep = max_chars.saturating_sub(suffix.chars().count());
    text.chars().take(keep).collect::<String>() + suffix
}

fn summarize_messages(messages: &[Message], max_line_chars: usize) -> String {
    let mut lines = Vec::new();
    for message in messages {
        if message.role == "user" || message.role == "assistant" {
            if !message.tool_calls.is_empty() {
                let names = message
                    .tool_calls
                    .iter()
                    .map(|call| call.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(bound_line(
                    &format!("{} tool_calls: {}", message.role, names),
                    max_line_chars,
                ));
            }
            if !message.content.is_empty() {
                lines.push(bound_line(
                    &format!("{}: {}", message.role, message.content),
                    max_line_chars,
                ));
            }
        } else if message.role == "tool" {
            let status = if message.content.starts_with("permission_denied:")
                || message.content.starts_with("unknown_tool:")
                || message.content.starts_with("tool_error:")
            {
                message
                    .content
                    .split_once(':')
                    .map(|(left, _)| left)
                    .unwrap_or("ok")
            } else {
                "ok"
            };
            lines.push(format!(
                "tool unknown {}: {} chars",
                status,
                message.content.chars().count()
            ));
        }
    }
    lines.join("\n")
}

fn compact_prompt_message(existing_summary: &str, messages: &[Message]) -> Message {
    let conversation = summarize_messages(messages, 500);
    Message::new(
        "user",
        format!(
            "CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.\n\n- Do NOT use shell, file, memory, network, or any other tool.\n- You already have all the context you need below.\n- Your entire response must be plain text: an <analysis> block followed by a <summary> block.\n\nYour task is to create a detailed summary of the conversation portion below for continuing an agent session on a small Linux device.\n\nBefore providing your final summary, wrap your analysis in <analysis> tags. Then provide a <summary> block with these sections:\n\n1. Primary Request and Intent\n2. Key Technical Concepts\n3. Files and Code Sections\n4. Errors and fixes\n5. Problem Solving\n6. All user messages\n7. Pending Tasks\n8. Current Work\n9. Optional Next Step\n\nPreserve user goals, decisions, file paths, commands, tool names, memory changes, device constraints, unresolved errors, and the latest concrete next step. Keep tool outputs concise and summarize metadata rather than copying large outputs.\n\nPrevious compacted summary:\n{}\n\nConversation portion to compact:\n{}\n\nREMINDER: Do NOT call any tools. Respond with plain text only: an <analysis> block followed by a <summary> block.",
            if existing_summary.trim().is_empty() {
                "(none)"
            } else {
                existing_summary.trim()
            },
            if conversation.is_empty() {
                "(no messages)"
            } else {
                &conversation
            }
        ),
    )
}

fn format_model_summary(summary: &str) -> String {
    let without_analysis = strip_tag_block(summary, "analysis");
    if let Some(content) = extract_tag_block(&without_analysis, "summary") {
        return format!("Summary:\n{}", content.trim());
    }
    without_analysis.trim().to_string()
}

fn append_summary(existing: &str, addition: &str, max_chars: usize) -> String {
    let combined = [existing.trim(), addition.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if combined.chars().count() <= max_chars {
        return combined;
    }
    let lines = combined.lines().collect::<Vec<_>>();
    let mut kept = Vec::new();
    let mut total = 0usize;
    for line in lines.iter().rev() {
        let line_len = line.chars().count() + usize::from(!kept.is_empty());
        if !kept.is_empty() && total + line_len > max_chars {
            break;
        }
        if kept.is_empty() && line.chars().count() > max_chars {
            return line
                .chars()
                .rev()
                .take(max_chars)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
        }
        kept.push(*line);
        total += line_len;
    }
    kept.into_iter().rev().collect::<Vec<_>>().join("\n")
}

fn retained_messages_after_compact(mut messages: Vec<Message>, recent_limit: usize) -> Vec<Message> {
    let kept_start = messages.len().saturating_sub(recent_limit);
    let latest_user = latest_user_index(&messages)
        .filter(|&index| index < kept_start)
        .map(|index| messages[index].clone());
    let mut kept = if recent_limit > 0 {
        messages.split_off(kept_start)
    } else {
        Vec::new()
    };
    if let Some(message) = latest_user {
        let mut next = vec![message];
        next.append(&mut kept);
        kept = next;
    }
    kept
}

fn budget_model_messages_parts(
    mut prefix: Vec<Message>,
    history: &[Message],
    max_chars: usize,
) -> (Vec<Message>, usize) {
    let prefix_chars = message_chars(&prefix);
    let history_chars = message_chars(history);
    if prefix_chars + history_chars <= max_chars {
        prefix.reserve(history.len());
        prefix.extend(history.iter().cloned());
        return (prefix, 0);
    }
    enum Slot {
        Prefix(usize),
        History(usize),
    }
    let mut slots = (0..prefix.len())
        .map(Slot::Prefix)
        .chain((0..history.len()).map(Slot::History))
        .collect::<Vec<_>>();
    let slot_chars = |slot: &Slot| -> usize {
        match slot {
            Slot::Prefix(index) => {
                prefix[*index].role.chars().count() + prefix[*index].content.chars().count()
            }
            Slot::History(index) => {
                history[*index].role.chars().count() + history[*index].content.chars().count()
            }
        }
    };
    let slot_role = |slot: &Slot| -> &str {
        match slot {
            Slot::Prefix(index) => prefix[*index].role.as_str(),
            Slot::History(index) => history[*index].role.as_str(),
        }
    };
    let mut dropped = 0usize;
    while slots.len() > 1 {
        let total: usize = slots.iter().map(slot_chars).sum();
        if total <= max_chars {
            break;
        }
        let latest_user = slots.iter().rposition(|slot| slot_role(slot) == "user");
        let Some(index) = slots.iter().enumerate().find_map(|(index, slot)| {
            if slot_role(slot) != "system" && Some(index) != latest_user {
                Some(index)
            } else {
                None
            }
        }) else {
            break;
        };
        slots.remove(index);
        dropped += 1;
    }
    let mut out = Vec::with_capacity(slots.len());
    for slot in slots {
        match slot {
            Slot::Prefix(index) => out.push(prefix[index].clone()),
            Slot::History(index) => out.push(history[index].clone()),
        }
    }
    (out, dropped)
}

fn latest_user_index(messages: &[Message]) -> Option<usize> {
    messages.iter().rposition(|message| message.role == "user")
}

fn message_chars(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|message| message.role.chars().count() + message.content.chars().count())
        .sum()
}

fn bound_line(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let keep = max_chars.saturating_sub(" ...".chars().count());
    normalized.chars().take(keep).collect::<String>() + " ..."
}

fn strip_tag_block(text: &str, tag: &str) -> String {
    let start_marker = format!("<{}>", tag);
    let end_marker = format!("</{}>", tag);
    let Some(start) = text.find(&start_marker) else {
        return text.to_string();
    };
    let Some(end) = text.find(&end_marker) else {
        return text.to_string();
    };
    if end < start {
        return text.to_string();
    }
    format!("{}{}", &text[..start], &text[end + end_marker.len()..])
}

fn extract_tag_block(text: &str, tag: &str) -> Option<String> {
    let start_marker = format!("<{}>", tag);
    let end_marker = format!("</{}>", tag);
    let start = text.find(&start_marker)?;
    let end = text.find(&end_marker)?;
    if end < start {
        return None;
    }
    Some(text[start + start_marker.len()..end].to_string())
}
use std::collections::BTreeMap;
