pub(crate) mod activity;
mod context;

use crate::config::AgentConfig;
use crate::memory::MemoryContext;
use crate::messages::{AgentResponse, MediaPart, Message, ModelLimits, ToolCall, ToolResult};
use crate::model::ModelClient;
use crate::model_history::{ContextUpdate, ModelHistory};
use crate::runtime_reload::RuntimeSnapshot;
use crate::skills::skill_catalog;
use crate::steering::{format_steering_ack, SteerHandle, SteeringState, SKIPPED_TOOL_RESULT};
use crate::time_context::current_time_context;
use crate::tools::{run_tool_map, string_arguments, tool_specs_for_config, ToolContext};
use crate::transcript::TranscriptWriter;
use crate::vision::analyze_image;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const SYSTEM_PROMPT: &str = "Your name is Colibri, a practical personal agent. Use the actual runtime facts and available tool schemas; do not assume the host device or operating system. Tools have full application access as the current OS user. For a multi-step execution task, use task.start/update to preserve the objective, acceptance checks, progress and next action. On recovery, inspect the task and verify uncertain side effects before repeating commands. Use background shell jobs for long commands, poll their final exit status, and check the requested outcome before marking steps completed. Starting a job is not completing it. Prefer bounded file reads, search and unique-context patches. Do not dump credentials. Report only tests and checks that actually ran; distinguish completed work, pending work and concrete blockers. Keep simple questions direct. A cancelled task must not resume unless the user explicitly requests it. Before any dangerous, destructive, or irreversible operation, ask the user and wait for explicit approval.";
pub const MODEL_UNAVAILABLE_TEXT: &str = "模型暂时不可用，请检查网络后重试。";
pub const TRUNCATED_TOOL_CALL_TEXT: &str = "Tool call was truncated because the model reached its output limit; retry the call with complete arguments.";

struct UsageAnchor {
    total_tokens: usize,
    message_count: usize,
    prefix_fingerprint: u64,
}

pub trait SessionToolExtension: Send + Sync {
    fn specs(&self) -> Vec<serde_json::Value>;
    fn run(&self, call: &ToolCall, context: &ToolContext) -> Option<ToolResult>;
}

pub struct AgentSession {
    pub config: Arc<AgentConfig>,
    model: Arc<Mutex<Box<dyn ModelClient>>>,
    pub messages: Vec<Message>,
    pub summary: String,
    transcript: Option<Arc<Mutex<TranscriptWriter>>>,
    transcript_metadata: BTreeMap<String, String>,
    activity_callback: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    last_activity: Mutex<String>,
    status_enabled: bool,
    status_callback: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    media_sender: Option<Arc<dyn Fn(MediaPart) -> Result<(), String> + Send + Sync>>,
    history_loader: Option<Box<dyn Fn() -> ModelHistory + Send>>,
    context_updates: Vec<ContextUpdate>,
    history_loaded: bool,
    direct_tools: Option<Box<dyn SessionToolExtension>>,
    working_directory: Option<PathBuf>,
    runtime_home: PathBuf,
    last_failure: Option<(u64, usize)>,
    owned_jobs: std::collections::BTreeSet<String>,
    steering: Arc<SteeringState>,
    steer_notifier: Option<Arc<dyn Fn(String) + Send + Sync>>,
    usage_anchor: Option<UsageAnchor>,
    request_capabilities: crate::model_capabilities::ModelCapabilities,
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
        mut transcript_metadata: BTreeMap<String, String>,
    ) -> Self {
        static NEXT_STREAM: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        transcript_metadata.insert(
            "model_stream".into(),
            format!(
                "{}-{stamp}-{}",
                std::process::id(),
                NEXT_STREAM.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ),
        );
        Self {
            request_capabilities: crate::model_capabilities::resolve(&config.model),
            config,
            model,
            messages: Vec::new(),
            summary: String::new(),
            transcript,
            transcript_metadata,
            activity_callback: None,
            last_activity: Mutex::new(String::new()),
            status_enabled: false,
            status_callback: None,
            media_sender: None,
            history_loader: None,
            context_updates: Vec::new(),
            history_loaded: false,
            direct_tools: None,
            working_directory: None,
            runtime_home: crate::config::colibri_home(),
            last_failure: None,
            owned_jobs: Default::default(),
            steering: Arc::new(SteeringState::new()),
            steer_notifier: None,
            usage_anchor: None,
        }
    }

    pub fn with_working_directory(mut self, cwd: PathBuf) -> Self {
        self.working_directory = Some(cwd);
        self
    }

    #[cfg(unix)]
    fn prepare_tools(&mut self) -> Result<(), String> {
        if self.direct_tools.is_none() {
            use crate::mcp::{servers, Extensions};
            let servers = servers(&self.config.mcp_servers)?;
            let extension = Extensions::discover(&servers, |message| {
                if let Some(callback) = &self.status_callback {
                    callback(&message);
                }
            });
            self.direct_tools = Some(Box::new(extension));
        }
        Ok(())
    }

    pub fn with_runtime_home(mut self, home: PathBuf) -> Self {
        self.runtime_home = home;
        self
    }

    pub fn with_history_loader(self, loader: Box<dyn Fn() -> Vec<Message> + Send>) -> Self {
        self.with_model_history_loader(Box::new(move || ModelHistory {
            messages: loader(),
            ..Default::default()
        }))
    }

    pub fn with_model_history_loader(
        mut self,
        loader: Box<dyn Fn() -> ModelHistory + Send>,
    ) -> Self {
        self.history_loader = Some(loader);
        self
    }

    pub(crate) fn inherit_history(&mut self, mut previous: Self) {
        self.owned_jobs = std::mem::take(&mut previous.owned_jobs);
        self.messages = previous.messages;
        self.summary = previous.summary;
        self.context_updates = previous.context_updates;
        self.history_loaded = false;
    }

    /// Transient progress for frontends; never added to conversation history.
    pub fn with_activity_callback(mut self, callback: Arc<dyn Fn(&str) + Send + Sync>) -> Self {
        self.activity_callback = Some(callback);
        self
    }

    fn notify_activity(&self, stage: &str) {
        if let Ok(mut last) = self.last_activity.lock() {
            *last = stage.to_string();
        }
        if let Some(callback) = &self.activity_callback {
            callback(stage);
        }
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

    pub fn has_pending_work(&self) -> bool {
        if !self.owned_jobs.is_empty() {
            return true;
        }
        #[cfg(unix)]
        {
            let c = ToolContext::new(Arc::clone(&self.config), self.runtime_home.clone())
                .with_runtime_home(self.runtime_home.clone())
                .with_scope(
                    self.transcript_metadata
                        .get("session_key")
                        .cloned()
                        .unwrap_or_else(|| "local".into()),
                );
            return crate::tasks::read(&c)
                .ok()
                .flatten()
                .is_some_and(|t| t.status == "running" || t.status == "blocked");
        }
        #[cfg(not(unix))]
        false
    }

    pub fn is_turn_active(&self) -> bool {
        self.steering.is_turn_active()
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
        self.submit_inner(text, Vec::new())
    }

    pub fn submit_with_media(
        &mut self,
        text: &str,
        media: Vec<MediaPart>,
    ) -> Result<AgentResponse, String> {
        self.submit_inner(text, media)
    }

    pub fn set_media_sender(
        &mut self,
        sender: Option<Arc<dyn Fn(MediaPart) -> Result<(), String> + Send + Sync>>,
    ) {
        self.media_sender = sender;
    }

    pub fn adopt_runtime(
        &mut self,
        config: Arc<AgentConfig>,
        model: Arc<Mutex<Box<dyn ModelClient>>>,
    ) {
        if self.config.mcp_servers != config.mcp_servers {
            self.direct_tools = None;
        }
        self.config = config;
        self.model = model;
        self.usage_anchor = None;
    }

    pub fn runtime_snapshot(&self) -> RuntimeSnapshot {
        RuntimeSnapshot {
            config: Arc::clone(&self.config),
            model: Arc::clone(&self.model),
        }
    }

    fn submit_inner(&mut self, text: &str, media: Vec<MediaPart>) -> Result<AgentResponse, String> {
        let token = self.steering.begin_turn();
        let _scope = crate::cancellation::Scope::enter(token.clone());
        let _turn_guard = TurnGuard {
            steering: Arc::clone(&self.steering),
        };
        if text.trim() == "/stop" && media.is_empty() {
            self.restore_history_once();
            self.messages.push(Message::new("user", text));
            self.write_transcript("user_message", serde_json::json!({"text":text}));
            return Ok(self.finish_stop());
        }
        let result = self.execute_inner(text, media);
        if token.is_cancelled() {
            Ok(self.finish_stop())
        } else {
            result
        }
    }

    fn finish_stop(&mut self) -> AgentResponse {
        let context = ToolContext::new(
            Arc::clone(&self.config),
            self.working_directory
                .clone()
                .unwrap_or_else(|| self.runtime_home.clone()),
        )
        .with_runtime_home(self.runtime_home.clone())
        .with_scope(
            self.transcript_metadata
                .get("session_key")
                .cloned()
                .unwrap_or_else(|| "local".into()),
        );
        #[cfg(unix)]
        for id in &self.owned_jobs {
            let _ = crate::jobs::cancel(&BTreeMap::from([("job_id".into(), id.clone())]), &context);
        }
        self.owned_jobs.clear();
        let missing = self
            .messages
            .iter()
            .flat_map(|m| m.tool_calls.iter())
            .filter(|c| {
                !self
                    .messages
                    .iter()
                    .any(|m| m.tool_call_id.as_deref() == Some(c.id.as_str()))
            })
            .cloned()
            .collect::<Vec<_>>();
        for call in missing {
            self.messages
                .push(Message::tool("cancelled: not executed", call.id.clone()));
            self.write_transcript("tool_result",serde_json::json!({"id":call.id,"name":call.name,"ok":false,"error_type":"cancelled","text":"未执行：用户停止"}));
        }
        while let Some(text) = self.steering.drain_one() {
            self.write_transcript(
                "steering_discarded",
                serde_json::json!({"text":text,"reason":"cancelled"}),
            );
        }
        let stage = self
            .last_activity
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        let stage = if stage.is_empty() {
            "处理请求"
        } else {
            activity::label(&stage)
        };
        let mut text = format!("已停止。阶段性结论：\n停止位置：{stage}\n");
        #[cfg(unix)]
        {
            text += &crate::tasks::cancel(&context)
                .unwrap_or_else(|e| format!("任务状态保存失败：{e}"));
            if let Ok(state) = crate::tasks::snapshot(&context) {
                self.write_transcript("task_state", serde_json::json!({"state":state}));
            }
        }
        let recent = self
            .messages
            .iter()
            .rev()
            .take_while(|m| m.role != "user")
            .filter(|m| m.role == "tool")
            .take(4)
            .map(|m| {
                if m.content.starts_with("cancelled:") {
                    "当前操作已中断，部分修改可能已经发生。".into()
                } else {
                    crate::memory::truncate(m.content.clone(), 180).0
                }
            })
            .collect::<Vec<_>>();
        for item in recent.into_iter().rev() {
            text += &format!("\n执行记录：{item}");
        }
        text += "\n后续步骤未继续。已发生的修改未撤销；中断中的操作和远端请求结果需核验。";
        crate::secrets::redact_tool_output(&self.config, &mut text);
        self.messages.push(Message::new("assistant", &text));
        self.write_transcript(
            "assistant_message",
            serde_json::json!({"text":text,"tool_call_count":0,"stop_reason":"cancelled"}),
        );
        AgentResponse {
            text,
            error_type: Some("cancelled".into()),
        }
    }

    fn execute_inner(
        &mut self,
        text: &str,
        media: Vec<MediaPart>,
    ) -> Result<AgentResponse, String> {
        #[cfg(unix)]
        self.prepare_tools()?;
        self.restore_history_once();
        if text.trim() == "/compact" && media.is_empty() {
            if self
                .messages
                .iter()
                .rposition(|message| message.role == "user")
                .is_some_and(|index| index > 0)
            {
                self.notify_activity("compacting");
                self.refresh_request_capabilities()?;
            }
            let changed = self.compact_now("manual");
            return Ok(AgentResponse {
                text: if changed {
                    "上下文已压缩，近期对话和历史记录已保留。"
                } else {
                    "当前没有可压缩的旧对话。"
                }
                .into(),
                error_type: None,
            });
        }
        let mut user_text = user_text_with_media(text, &media);
        user_text.push_str("\n\n");
        user_text.push_str(&current_time_context());
        self.messages.push(Message::new("user", &user_text));
        self.write_transcript(
            "user_message",
            serde_json::json!({
                "text": crate::time_context::user_text_without_time_context(&user_text),
                "model_text": user_text,
                "media": media.iter().map(media_payload).collect::<Vec<_>>()
            }),
        );

        let cwd = match &self.working_directory {
            Some(cwd) => cwd.clone(),
            None => std::env::current_dir().map_err(|error| error.to_string())?,
        };
        let analyzer_config = Arc::clone(&self.config);
        let analyzer = Arc::new(move |path: &Path, prompt: &str| {
            analyze_image(&analyzer_config, path, prompt)
        });
        let mut context = ToolContext::new(Arc::clone(&self.config), cwd)
            .with_runtime_home(self.runtime_home.clone())
            .with_scope(
                self.transcript_metadata
                    .get("session_key")
                    .cloned()
                    .unwrap_or_else(|| "local".into()),
            )
            .with_image_analyzer(analyzer);
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
        self.sync_runtime_context(
            &memory.text,
            &skill_text,
            self.messages.len().saturating_sub(1),
        );
        let tools = tool_specs_for_config(&self.config);
        self.run_tool_rounds(&tools, &context)
    }

    fn refresh_request_capabilities(&mut self) -> Result<(), String> {
        let home = self.runtime_home.clone();
        self.request_capabilities = self
            .model
            .lock()
            .map_err(|_| "model lock poisoned".to_string())?
            .capabilities(&home)
            .unwrap_or_else(|| crate::model_capabilities::resolve(&self.config.model));
        self.write_transcript(
            "model_context",
            serde_json::json!({
                "context_window":self.request_capabilities.context_window,
                "max_output_tokens":self.request_capabilities.max_output_tokens,
                "source":self.request_capabilities.source,
            }),
        );
        Ok(())
    }

    fn run_tool_rounds(
        &mut self,
        tools: &[serde_json::Value],
        context: &ToolContext,
    ) -> Result<AgentResponse, String> {
        let mut completion_reminded = false;
        loop {
            if crate::cancellation::cancelled() {
                return Err("cancelled".into());
            }
            let mut active_tools = tools.to_vec();
            if let Some(extension) = &self.direct_tools {
                active_tools.extend(extension.specs());
            }
            let tools = active_tools.as_slice();
            self.notify_activity("thinking");
            let latest_memory = MemoryContext::new(Arc::clone(&self.config)).load()?;
            let (latest_skills, _, _) = skill_catalog(context);
            self.sync_runtime_context(&latest_memory.text, &latest_skills, self.messages.len());
            self.refresh_request_capabilities()?;
            if crate::cancellation::cancelled() {
                return Err("cancelled".into());
            }
            self.compact_for_completion_if_needed(tools);
            if self.context_memory_exceeded(tools) {
                return Ok(AgentResponse {
                    text:
                        "当前任务产生的内容超过已配置的上下文内存预算，请缩小文件或命令输出范围后继续。"
                            .into(),
                    error_type: Some("context_memory_budget".into()),
                });
            }
            let mut model_messages = self.model_messages();
            let mut overflow_retried = false;
            let response = loop {
                let request_messages = &model_messages;
                let response_result = {
                    let mut model = self
                        .model
                        .lock()
                        .map_err(|_| "model lock poisoned".to_string())?;
                    model.complete(
                        request_messages,
                        tools,
                        SYSTEM_PROMPT,
                        &ModelLimits {
                            timeout_seconds: self.config.model.timeout_seconds,
                            max_output_tokens: self.request_capabilities.max_output_tokens,
                        },
                    )
                };
                if crate::cancellation::cancelled() {
                    return Err("cancelled".into());
                }
                match response_result {
                    Ok(response) => break response,
                    Err(error)
                        if model_error_category(&error) == "context_overflow"
                            && !overflow_retried =>
                    {
                        overflow_retried = true;
                        let home = self.runtime_home.clone();
                        if let Ok(mut model) = self.model.lock() {
                            model.invalidate_capabilities(&home);
                        }
                        self.write_transcript(
                            "model_error",
                            serde_json::json!({"error_type":"context_overflow","message":error}),
                        );
                        self.compact_now("overflow");
                        model_messages = self.model_messages();
                    }
                    Err(error) => return Ok(self.finish_model_error(&error)),
                }
            };
            if crate::cancellation::cancelled() {
                return Err("cancelled".into());
            }
            let assistant_text = bound_text(&response.text, self.config.tools.max_result_chars);
            let mut assistant = Message::new("assistant", &assistant_text);
            assistant.tool_calls = response.tool_calls.clone();
            self.messages.push(assistant);
            self.write_transcript(
                "assistant_message",
                serde_json::json!({
                    "text":assistant_text,
                    "tool_call_count":response.tool_calls.len(),
                    "tool_calls": &response.tool_calls,
                    "stop_reason":response.stop_reason,
                    "usage":usage_payload(&response)
                }),
            );
            self.update_usage_anchor(&response, tools);

            if response.stop_reason.as_deref() == Some("length") && !response.tool_calls.is_empty()
            {
                for call in &response.tool_calls {
                    self.record_truncated_tool_call(call);
                }
                continue;
            }

            if response.tool_calls.is_empty() {
                if let Some(steered) = self.steering.drain_one() {
                    self.apply_steering(&steered, 0);
                    continue;
                }
                #[cfg(unix)]
                if let Some(pending) = crate::tasks::incomplete(context) {
                    if !completion_reminded {
                        completion_reminded = true;
                        self.messages.push(Message::new("user", &pending));
                        self.write_transcript(
                            "task_continuation",
                            serde_json::json!({"text":pending}),
                        );
                        continue;
                    }
                    return Ok(AgentResponse {
                        text: format!("任务尚未完成。\n{assistant_text}"),
                        error_type: Some("task_incomplete".into()),
                    });
                }
                return Ok(AgentResponse {
                    text: assistant_text,
                    error_type: None,
                });
            }

            let calls = response.tool_calls;
            for index in 0..calls.len() {
                if crate::cancellation::cancelled() {
                    return Err("cancelled".into());
                }
                self.execute_tool_call(&calls[index], context)?;
                if crate::cancellation::cancelled() {
                    return Err("cancelled".into());
                }
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
                "stop_reason":serde_json::Value::Null,
                "usage":serde_json::Value::Null,
                "model_error":category
            }),
        );
        AgentResponse {
            text: MODEL_UNAVAILABLE_TEXT.to_string(),
            error_type: Some(category.to_string()),
        }
    }

    fn execute_tool_call(&mut self, call: &ToolCall, context: &ToolContext) -> Result<(), String> {
        let execution_arguments = string_arguments(&call.arguments);
        #[cfg(unix)]
        let task_id = crate::tasks::active_id(context);
        self.notify_activity(activity::for_tool(&call.name));
        self.write_transcript(
            "tool_call",
            serde_json::json!({"id":call.id,"name":call.name,"arguments":call.arguments}),
        );
        let extension_result = self
            .direct_tools
            .as_ref()
            .and_then(|extension| extension.run(call, context));
        let mut result = match extension_result {
            Some(result) => result,
            None => run_tool_map(&call.name, &execution_arguments, context)
                .unwrap_or_else(|e| ToolResult::error("tool_error", e)),
        };
        crate::secrets::redact_tool_output(&self.config, &mut result.text);
        #[cfg(unix)]
        if let Some(id) = task_id {
            match crate::tasks::record(context, &id, call, &result) {
                Ok(Some(evidence)) => {
                    result.text = format!("evidence_id={evidence}\n{}", result.text)
                }
                Err(error) => {
                    result.text += &format!("\nTask evidence could not be saved: {error}")
                }
                _ => {}
            }
        }
        #[cfg(unix)]
        if result.ok && call.name.starts_with("task.") && call.name != "task.inspect" {
            if let Ok(state) = crate::tasks::snapshot(context) {
                let mut text = state.to_string();
                crate::secrets::redact_tool_output(&self.config, &mut text);
                self.write_transcript("task_state", serde_json::json!({"state":serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default()}));
            }
        }
        if !result.ok {
            use std::hash::{Hash, Hasher};
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            call.name.hash(&mut hash);
            serde_json::to_string(&call.arguments)
                .unwrap_or_default()
                .hash(&mut hash);
            result.error_type.hash(&mut hash);
            let key = hash.finish();
            let count = self
                .last_failure
                .filter(|(old, _)| *old == key)
                .map(|(_, n)| n + 1)
                .unwrap_or(1);
            self.last_failure = Some((key, count));
            if count == 3 {
                result.text += "\nThe same call has failed three times. Inspect the error and change the approach; do not repeat it unchanged. Record a concrete blocker if external input is required.";
            }
        } else {
            self.last_failure = None;
        }
        if call.name == "shell.run" && result.ok {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(
                result
                    .text
                    .split_once('\n')
                    .filter(|(head, _)| head.starts_with("evidence_id="))
                    .map_or(result.text.as_str(), |(_, body)| body),
            ) {
                if let Some(id) = value["job_id"].as_str() {
                    self.owned_jobs.insert(id.to_string());
                }
            }
        }
        let result = self.send_media_result_if_needed(result);
        let content = if result.ok {
            result.text
        } else {
            format!(
                "{}: {}",
                result.error_type.as_deref().unwrap_or("tool_error"),
                result.text
            )
        };
        let text = if result.ok {
            content.as_str()
        } else {
            content
                .split_once(": ")
                .map_or(content.as_str(), |(_, text)| text)
        };
        self.write_transcript(
            "tool_result",
            serde_json::json!({
                "id":call.id,
                "name":call.name,
                "ok":result.ok,
                "error_type":result.error_type,
                "text":bound_text(text, self.config.tools.max_result_chars),
                "model_text": &content,
                "model_history_truncated": false,
                "truncated":result.truncated,
                "media":result.media.as_ref().map(media_payload)
            }),
        );
        self.messages.push(Message::tool(content, call.id.clone()));
        Ok(())
    }

    fn record_truncated_tool_call(&mut self, call: &ToolCall) {
        self.write_transcript(
            "tool_result",
            serde_json::json!({
                "id":call.id,
                "name":call.name,
                "ok":false,
                "error_type":"truncated_tool_call",
                "text":TRUNCATED_TOOL_CALL_TEXT,
                "truncated":false,
                "media":serde_json::Value::Null
            }),
        );
        self.messages.push(Message::tool(
            &format!("truncated_tool_call: {}", TRUNCATED_TOOL_CALL_TEXT),
            &call.id,
        ));
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
        let model_text = format!("{text}\n\n{}", current_time_context());
        self.messages.push(Message::new("user", &model_text));
        self.write_transcript(
            "user_message",
            serde_json::json!({
                "text": text,
                "model_text": model_text,
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

fn usage_payload(response: &crate::messages::ModelResponse) -> serde_json::Value {
    let Some(usage) = &response.usage else {
        return serde_json::Value::Null;
    };
    serde_json::json!({
        "input_tokens":usage.input_tokens,
        "output_tokens":usage.output_tokens,
        "cache_read_tokens":usage.cache_read_tokens,
        "cache_write_tokens":usage.cache_write_tokens,
        "total_tokens":usage.total_tokens
    })
}

fn owned_transcript(config: &AgentConfig) -> Option<Arc<Mutex<TranscriptWriter>>> {
    TranscriptWriter::for_session(&crate::config::colibri_home(), &config.session)
        .ok()
        .flatten()
        .map(|writer| Arc::new(Mutex::new(writer)))
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
        self.steering.end_turn();
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
