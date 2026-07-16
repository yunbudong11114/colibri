use crate::config::AgentConfig;
use crate::context::{
    append_summary, compact_prompt_message, format_model_summary, retain_recent_message_groups,
    round_limit_text, summarize_messages, summary_context, COMPACT_SYSTEM_PROMPT,
};
use crate::memory::MemoryContext;
use crate::messages::{AgentResponse, MediaPart, Message, ModelLimits, ToolCall, ToolResult};
use crate::model::ModelClient;
use crate::permissions::{PermissionPolicy, PermissionPrompter};
use crate::skills::skill_catalog;
use crate::steering::{format_steering_ack, SteerHandle, SteeringState, SKIPPED_TOOL_RESULT};
use crate::tools::{run_tool_map, string_arguments, tool_info, tool_specs_for_config, ToolContext};
use crate::transcript::TranscriptWriter;
use crate::vision::analyze_image;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub const SYSTEM_PROMPT: &str = "Your name is Colibri. You are a lightweight personal agent running on the CardputerZero, a multi-interface device powered by the CM0 chip. Prefer short, practical responses and respect low memory, battery, and tool limits. ";
pub const MODEL_UNAVAILABLE_TEXT: &str = "模型暂时不可用，请检查网络后重试。";

pub struct AgentSession {
    pub config: Arc<AgentConfig>,
    model: Arc<Mutex<Box<dyn ModelClient>>>,
    pub messages: Vec<Message>,
    pub summary: String,
    transcript: Option<Arc<Mutex<TranscriptWriter>>>,
    transcript_metadata: BTreeMap<String, String>,
    status_enabled: bool,
    status_callback: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    media_sender: Option<Arc<dyn Fn(MediaPart) -> Result<(), String> + Send + Sync>>,
    history_loader: Option<Box<dyn Fn() -> Vec<Message> + Send>>,
    history_loaded: bool,
    permission_policy: PermissionPolicy,
    steering: Arc<SteeringState>,
    steer_notifier: Option<Arc<dyn Fn(String) + Send + Sync>>,
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
        let permission_policy = PermissionPolicy::from_config(&config, std::path::PathBuf::new());
        Self {
            config,
            model,
            messages: Vec::new(),
            summary: String::new(),
            transcript,
            transcript_metadata,
            status_enabled: false,
            status_callback: None,
            media_sender: None,
            history_loader: None,
            history_loaded: false,
            permission_policy,
            steering: Arc::new(SteeringState::new()),
            steer_notifier: None,
        }
    }

    pub fn with_history_loader(mut self, loader: Box<dyn Fn() -> Vec<Message> + Send>) -> Self {
        self.history_loader = Some(loader);
        self
    }

    pub fn with_status_callback(
        mut self,
        enabled: bool,
        callback: Arc<dyn Fn(&str) + Send + Sync>,
    ) -> Self {
        self.status_enabled = enabled;
        self.status_callback = Some(callback);
        self
    }

    pub fn set_steer_notifier(&mut self, notifier: Option<Arc<dyn Fn(String) + Send + Sync>>) {
        self.steer_notifier = notifier;
    }

    pub fn with_steer_notifier(mut self, notifier: Arc<dyn Fn(String) + Send + Sync>) -> Self {
        self.steer_notifier = Some(notifier);
        self
    }

    pub fn steer_handle(&self) -> SteerHandle {
        SteerHandle::new(Arc::clone(&self.steering))
    }

    pub fn steer(&self, text: &str) -> bool {
        self.steering.steer(text)
    }

    pub fn is_turn_active(&self) -> bool {
        self.steering.is_turn_active()
    }

    pub fn is_permission_pending(&self) -> bool {
        self.steering.is_permission_pending()
    }

    /// Close transcript resources. Shared gateway transcripts stay open until
    /// `GatewaySessionCache::close` (matches Python ScopedTranscriptWriter).
    pub fn close(&mut self) {
        if let Some(transcript) = self.transcript.take() {
            if let Ok(mutex) = Arc::try_unwrap(transcript) {
                if let Ok(mut writer) = mutex.into_inner() {
                    writer.close();
                }
            }
        }
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
        self.messages.push(Message::new("user", &user_text));
        self.write_transcript(
            "user_message",
            serde_json::json!({
                "text": user_text,
                "media": media.iter().map(media_payload).collect::<Vec<_>>()
            }),
        );

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
        let memory = MemoryContext::new(Arc::clone(&self.config)).load()?;
        if !memory.text.is_empty() {
            self.write_transcript(
                "memory_context",
                serde_json::json!({"files":memory.files,"truncated":memory.truncated}),
            );
        }
        let (skill_text, skill_names, _truncated) = skill_catalog(&context);
        if !skill_text.is_empty() {
            self.write_transcript(
                "skill_catalog",
                serde_json::json!({"skills":skill_names,"truncated":_truncated}),
            );
        }
        let tools = tool_specs_for_config(&self.config);
        let steering = Arc::clone(&self.steering);
        steering.set_turn_active(true);
        let _turn_guard = TurnGuard { steering };
        let mut prompter = prompter;
        self.run_tool_rounds(&memory.text, &skill_text, &tools, &context, &mut prompter)
    }

    fn run_tool_rounds(
        &mut self,
        memory_text: &str,
        skill_text: &str,
        tools: &[serde_json::Value],
        context: &ToolContext,
        prompter: &mut Option<&mut dyn PermissionPrompter>,
    ) -> Result<AgentResponse, String> {
        for _ in 0..self.config.session.max_tool_rounds {
            let model_messages = self.model_messages_for_completion(memory_text, skill_text);
            let response_result = {
                let mut model = self
                    .model
                    .lock()
                    .map_err(|_| "model lock poisoned".to_string())?;
                model.complete(
                    &model_messages,
                    tools,
                    SYSTEM_PROMPT,
                    &ModelLimits {
                        timeout_seconds: self.config.model.timeout_seconds,
                        max_output_tokens: self.config.model.max_output_tokens,
                    },
                )
            };
            let response = match response_result {
                Ok(response) => response,
                Err(error) => return Ok(self.finish_model_error(&error)),
            };
            let assistant_text = bound_text(&response.text, self.config.tools.max_result_chars);
            let mut assistant = Message::new("assistant", &assistant_text);
            assistant.tool_calls = response.tool_calls.clone();
            self.messages.push(assistant);
            self.write_transcript(
                "assistant_message",
                serde_json::json!({"text":assistant_text,"tool_call_count":response.tool_calls.len()}),
            );

            if response.tool_calls.is_empty() {
                if let Some(steered) = self.steering.drain_one() {
                    self.apply_steering(&steered, 0);
                    continue;
                }
                return Ok(AgentResponse {
                    text: assistant_text,
                    error_type: None,
                });
            }

            let calls = response.tool_calls;
            for index in 0..calls.len() {
                self.execute_tool_call(&calls[index], context, prompter)?;
                if let Some(steered) = self.steering.drain_one() {
                    let skipped = calls.len() - index - 1;
                    for skipped_call in &calls[index + 1..] {
                        self.record_skipped_tool(skipped_call);
                    }
                    self.apply_steering(&steered, skipped);
                    break;
                }
            }
        }

        let text = round_limit_text(
            &self.messages,
            self.config.session.max_tool_rounds,
            self.config.tools.max_result_chars,
        );
        self.messages.push(Message::new("assistant", &text));
        self.write_transcript(
            "round_limit",
            serde_json::json!({"max_tool_rounds":self.config.session.max_tool_rounds,"text":text}),
        );
        Ok(AgentResponse {
            text,
            error_type: None,
        })
    }

    fn finish_model_error(&mut self, error: &str) -> AgentResponse {
        let category = model_error_category(error);
        self.write_transcript(
            "model_error",
            serde_json::json!({"error_type":category,"message":error}),
        );
        self.messages
            .push(Message::new("assistant", MODEL_UNAVAILABLE_TEXT));
        self.write_transcript(
            "assistant_message",
            serde_json::json!({
                "text":MODEL_UNAVAILABLE_TEXT,
                "tool_call_count":0,
                "model_error":category
            }),
        );
        AgentResponse {
            text: MODEL_UNAVAILABLE_TEXT.to_string(),
            error_type: Some(category.to_string()),
        }
    }

    fn execute_tool_call(
        &mut self,
        call: &ToolCall,
        context: &ToolContext,
        prompter: &mut Option<&mut dyn PermissionPrompter>,
    ) -> Result<(), String> {
        let execution_arguments = string_arguments(&call.arguments);
        self.write_transcript(
            "tool_call",
            serde_json::json!({"id":call.id,"name":call.name,"arguments":call.arguments}),
        );
        self.steering.set_permission_pending(true);
        let _permission_guard = PermissionPendingGuard {
            steering: Arc::clone(&self.steering),
        };
        let decision = if let Some(prompter) = prompter.as_mut() {
            self.permission_policy.decide(
                &tool_info(&call.name),
                &execution_arguments,
                context,
                Some(&mut **prompter),
            )
        } else {
            self.permission_policy.decide(
                &tool_info(&call.name),
                &execution_arguments,
                context,
                None,
            )
        };
        drop(_permission_guard);
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
            crate::messages::ToolResult::error("permission_denied", denied_tool_text(call))
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
        self.messages.push(Message::tool(content, call.id.clone()));
        Ok(())
    }

    fn record_skipped_tool(&mut self, call: &ToolCall) {
        self.write_transcript(
            "tool_result",
            serde_json::json!({
                "id": call.id,
                "name": call.name,
                "ok": false,
                "error_type": "steered_skip",
                "text": SKIPPED_TOOL_RESULT,
                "truncated": false,
                "media": serde_json::Value::Null
            }),
        );
        let content = format!("steered_skip: {SKIPPED_TOOL_RESULT}");
        self.messages.push(Message::tool(content, call.id.clone()));
    }

    fn apply_steering(&mut self, text: &str, skipped: usize) {
        let chars = text.chars().count();
        let transcript_text: String = text.chars().take(200).collect();
        self.write_transcript(
            "steered",
            serde_json::json!({
                "skipped": skipped,
                "chars": chars,
                "text": transcript_text
            }),
        );
        if let Some(notifier) = &self.steer_notifier {
            notifier(format_steering_ack(skipped, text));
        }
        self.messages.push(Message::new("user", text));
        self.write_transcript(
            "user_message",
            serde_json::json!({
                "text": text,
                "media": [],
                "steering": true
            }),
        );
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

    fn model_messages_for_completion(
        &mut self,
        memory_text: &str,
        skill_text: &str,
    ) -> Vec<Message> {
        self.compact_for_completion_if_needed(memory_text, skill_text);
        self.model_messages(memory_text, skill_text)
    }

    fn model_messages(&self, memory_text: &str, skill_text: &str) -> Vec<Message> {
        let mut messages = Vec::new();
        let summary_text = summary_context(&self.summary);
        if !summary_text.is_empty() {
            messages.push(Message::new("system", summary_text));
        }
        if !memory_text.is_empty() {
            messages.push(Message::new("system", memory_text));
        }
        if !skill_text.is_empty() {
            messages.push(Message::new("system", skill_text));
        }
        messages.extend(self.messages.iter().cloned());
        messages
    }

    /// Estimate tokens without cloning messages / tool_calls JSON.
    fn estimate_completion_input_tokens(&self, memory_text: &str, skill_text: &str) -> usize {
        let mut byte_count = 0usize;
        let summary_text = summary_context(&self.summary);
        if !summary_text.is_empty() {
            byte_count += "system".len() + summary_text.len();
        }
        if !memory_text.is_empty() {
            byte_count += "system".len() + memory_text.len();
        }
        if !skill_text.is_empty() {
            byte_count += "system".len() + skill_text.len();
        }
        for message in &self.messages {
            byte_count += message.role.len() + message.content.len();
        }
        (byte_count + 3) / 4
    }

    fn compact_for_completion_if_needed(&mut self, memory_text: &str, skill_text: &str) {
        let trigger_limit = self.config.session.trigger_message_limit.max(1);
        let token_limit = self.config.model.input_context_tokens;
        let token_threshold = if token_limit == 0 {
            0
        } else {
            token_limit.saturating_mul(8) / 10
        };
        let mut should_compact = self.messages.len() >= trigger_limit;
        if !should_compact && token_threshold > 0 {
            should_compact =
                self.estimate_completion_input_tokens(memory_text, skill_text) >= token_threshold;
        }
        if !should_compact || self.messages.is_empty() {
            return;
        }
        self.compact_now();
    }

    fn compact_now(&mut self) {
        if self.messages.is_empty() {
            return;
        }
        let messages_to_compact = std::mem::take(&mut self.messages);
        let compacted_len = messages_to_compact.len();
        let (addition, mode) = self.compact_messages(&messages_to_compact);
        self.summary = append_summary(
            &self.summary,
            &addition,
            self.config.session.summary_max_chars,
        );
        self.messages = retain_recent_message_groups(
            messages_to_compact,
            self.config.session.recent_message_limit,
        );
        self.write_transcript(
            "context_compact",
            serde_json::json!({
                "removed_messages": compacted_len.saturating_sub(self.messages.len()),
                "compacted_messages": compacted_len,
                "kept_messages": self.messages.len(),
                "mode": mode,
                "summary_chars": self.summary.chars().count()
            }),
        );
    }

    fn compact_messages(&mut self, messages: &[Message]) -> (String, String) {
        if self.should_model_compact() {
            match self.try_model_compact(messages) {
                Ok(addition) => return (addition, "model".to_string()),
                Err(error) => {
                    self.write_transcript(
                        "context_compact_error",
                        serde_json::json!({
                            "error_type": error.error_type,
                            "message": error.message,
                            "fallback": true,
                        }),
                    );
                }
            }
        }
        (summarize_messages(messages, 160), "fallback".to_string())
    }

    fn should_model_compact(&self) -> bool {
        self.config.session.model_compact && self.config.model.provider != "fake"
    }

    fn try_model_compact(&mut self, messages: &[Message]) -> Result<String, CompactError> {
        let prompt = compact_prompt_message(&self.summary, messages);
        let mut model = self
            .model
            .lock()
            .map_err(|_| CompactError::new("PoisonError", "model lock poisoned"))?;
        let response = model
            .complete(
                &[prompt],
                &[],
                COMPACT_SYSTEM_PROMPT,
                &ModelLimits {
                    timeout_seconds: self.config.model.timeout_seconds,
                    max_output_tokens: self.config.model.max_output_tokens,
                },
            )
            .map_err(|error| CompactError::new("RuntimeError", error))?;
        if !response.tool_calls.is_empty() {
            return Err(CompactError::new(
                "RuntimeError",
                "compact response included tool calls",
            ));
        }
        let addition = format_model_summary(&response.text);
        if addition.is_empty() {
            return Err(CompactError::new(
                "RuntimeError",
                "compact response was empty",
            ));
        }
        Ok(addition)
    }

    fn write_transcript(&mut self, event_type: &str, mut payload: serde_json::Value) {
        if self.status_enabled {
            if let Some(line) = crate::console::status_line_for_event(event_type, &payload) {
                if let Some(callback) = &self.status_callback {
                    callback(&line);
                }
            }
        }
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

fn model_error_category(error: &str) -> &str {
    error
        .strip_prefix("model_error:")
        .and_then(|rest| rest.split_once(':'))
        .map(|(category, _)| category)
        .filter(|category| !category.is_empty())
        .unwrap_or("invalid_response")
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

struct TurnGuard {
    steering: Arc<SteeringState>,
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.steering.set_turn_active(false);
        self.steering.clear();
    }
}

struct PermissionPendingGuard {
    steering: Arc<SteeringState>,
}

impl Drop for PermissionPendingGuard {
    fn drop(&mut self) {
        self.steering.set_permission_pending(false);
    }
}

struct CompactError {
    error_type: String,
    message: String,
}

impl CompactError {
    fn new(error_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error_type: error_type.into(),
            message: message.into(),
        }
    }
}
