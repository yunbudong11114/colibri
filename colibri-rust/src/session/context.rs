//! Context snapshots, bounded recovery checkpoints, usage budgets and compaction.
use super::{AgentSession, CompactError, UsageAnchor, SYSTEM_PROMPT};
use crate::context::{
    append_summary, compact_prompt_message, estimate_completion_input_tokens,
    estimate_model_input_tokens, format_model_summary, summarize_messages, summary_context,
    COMPACT_SYSTEM_PROMPT,
};
use crate::messages::{Message, ModelLimits};
use crate::model_history::MAX_SNAPSHOT_HISTORY_BYTES;
use crate::model_history::{ContextUpdate, ModelHistory};
use std::hash::{DefaultHasher, Hash, Hasher};

impl AgentSession {
    pub(super) fn sync_runtime_context(
        &mut self,
        memory: &str,
        skills: &str,
        before_message: usize,
    ) {
        if memory.is_empty() && skills.is_empty() && self.context_updates.is_empty() {
            return;
        }
        let mut text = crate::model_history::runtime_context(memory, skills);
        crate::secrets::redact_tool_output(&self.config, &mut text);
        if self
            .context_updates
            .last()
            .is_some_and(|update| update.text == text)
        {
            return;
        }
        let update = ContextUpdate {
            before_message,
            text,
        };
        self.write_transcript("runtime_context", serde_json::json!({"update": &update}));
        self.context_updates.push(update);
        self.usage_anchor = None;
    }

    pub(super) fn history_snapshot(&self) -> ModelHistory {
        ModelHistory {
            messages: self.messages.clone(),
            summary: self.summary.clone(),
            context_updates: self.context_updates.clone(),
        }
    }

    pub(super) fn write_model_checkpoint(&mut self, name: &str, mut payload: serde_json::Value) {
        if self.transcript.is_none() {
            return;
        }
        let limit = self.config.session.restore_scan_bytes.saturating_sub(4096);
        let approximate = estimate_model_input_tokens(&self.messages)
            .saturating_mul(4)
            .saturating_add(self.summary.len())
            .saturating_add(crate::model_history::context_bytes(&self.context_updates));
        if approximate > limit {
            self.write_transcript(name, payload);
            return;
        }
        let snapshot = self.history_snapshot();
        if snapshot.encoded_len() <= limit {
            payload["history"] = serde_json::to_value(snapshot).unwrap_or_default();
        }
        self.write_transcript(name, payload);
    }

    pub(super) fn restore_history_once(&mut self) {
        if self.history_loaded {
            return;
        }
        self.history_loaded = true;
        if let Some(loader) = self.history_loader.take() {
            let history = loader();
            let offset = self.messages.len();
            self.messages.extend(history.messages);
            if self.summary.is_empty() {
                self.summary = history.summary;
            }
            self.context_updates
                .extend(history.context_updates.into_iter().map(|mut update| {
                    update.before_message += offset;
                    update
                }));
        }
        self.write_model_checkpoint("model_seed", serde_json::json!({}));
    }

    pub(super) fn context_memory_exceeded(&self, tools: &[serde_json::Value]) -> bool {
        self.config
            .session
            .max_context_memory_mib
            .is_some_and(|mib| {
                let bytes = estimate_completion_input_tokens(&self.messages, SYSTEM_PROMPT, tools)
                    .saturating_mul(4)
                    .saturating_add(self.summary.len())
                    .saturating_add(crate::model_history::context_bytes(&self.context_updates));
                bytes >= mib.saturating_mul(1024 * 1024)
            })
    }

    pub(super) fn model_messages(&self) -> Vec<Message> {
        crate::model_history::assemble(&self.messages, &self.summary, &self.context_updates)
    }

    pub(super) fn estimate_completion_input_tokens(&self, tools: &[serde_json::Value]) -> usize {
        let fingerprint = self.completion_prefix_fingerprint(tools);
        if let Some(anchor) = &self.usage_anchor {
            if anchor.prefix_fingerprint == fingerprint
                && anchor.message_count <= self.messages.len()
            {
                return anchor
                    .total_tokens
                    .saturating_add(estimate_model_input_tokens(
                        &self.messages[anchor.message_count..],
                    ));
            }
        }
        estimate_completion_input_tokens(&self.model_messages(), SYSTEM_PROMPT, tools)
    }

    pub(super) fn compact_for_completion_if_needed(&mut self, tools: &[serde_json::Value]) {
        if crate::model_history::context_bytes(&self.context_updates) > MAX_SNAPSHOT_HISTORY_BYTES {
            if !self.compact_now("runtime_context_budget") {
                if let Some(mut latest) = self.context_updates.pop() {
                    latest.before_message = 0;
                    self.context_updates.clear();
                    self.context_updates.push(latest);
                    self.usage_anchor = None;
                    self.write_transcript(
                        "runtime_context_rebased",
                        serde_json::json!({"updates": &self.context_updates}),
                    );
                }
            }
        }
        let token_threshold = self.compaction_token_threshold();
        let memory_pressure = self.context_memory_exceeded(tools);
        let should_compact = memory_pressure
            || (token_threshold > 0
                && self.estimate_completion_input_tokens(tools) >= token_threshold);
        if !should_compact || self.messages.is_empty() {
            return;
        }
        self.compact_now(if memory_pressure {
            "memory_budget"
        } else {
            "threshold"
        });
    }

    pub(super) fn compact_now(&mut self, reason: &str) -> bool {
        let latest_user = self
            .messages
            .iter()
            .rposition(|m| m.role == "user")
            .unwrap_or(0);
        if latest_user == 0 {
            return false;
        }
        self.notify_activity("压缩上下文");
        let threshold = self.compaction_token_threshold();
        let target = if reason == "manual" || reason == "overflow" {
            0
        } else {
            (threshold / 3).min(8192)
        };
        let mut start = latest_user;
        if target > 0 {
            for index in (0..latest_user).rev() {
                if self.messages[index].role == "user" {
                    if estimate_model_input_tokens(&self.messages[index..]) > target {
                        break;
                    }
                    start = index;
                }
            }
        }
        if start == 0 {
            start = latest_user;
        }
        let mut old = std::mem::take(&mut self.messages);
        self.messages = old.split_off(start);
        let removed_bytes = old.iter().map(|m| m.content.len()).sum::<usize>();
        let (addition, mode) = self.compact_messages(&old);
        self.summary = if mode == "model" {
            addition.chars().take(12000).collect()
        } else {
            append_summary(&self.summary, &addition, 12000)
        };
        drop(old);
        if let Some(mut latest) = self.context_updates.pop() {
            latest.before_message = 0;
            self.context_updates.clear();
            self.context_updates.push(latest);
        }
        self.usage_anchor = None;
        self.write_model_checkpoint(
            "context_compact",
            serde_json::json!({
                "removed_messages": start,
                "compacted_messages": start,
                "kept_messages": self.messages.len(),
                "removed_bytes": removed_bytes,
                "mode": mode,
                "reason": reason,
                "summary_chars": self.summary.chars().count(),
                "summary": &self.summary
            }),
        );
        if reason != "manual" {
            self.notify_activity("思考中");
        }
        true
    }

    pub(super) fn compaction_token_threshold(&self) -> usize {
        self.request_capabilities.compaction_threshold()
    }

    pub(super) fn completion_prefix_fingerprint(&self, tools: &[serde_json::Value]) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.config.model.provider.hash(&mut hasher);
        self.config.model.base_url.hash(&mut hasher);
        self.config.model.model.hash(&mut hasher);
        SYSTEM_PROMPT.hash(&mut hasher);
        summary_context(&self.summary).hash(&mut hasher);
        for update in &self.context_updates {
            update.before_message.hash(&mut hasher);
            update.text.hash(&mut hasher);
        }
        serde_json::to_string(tools)
            .unwrap_or_default()
            .hash(&mut hasher);
        hasher.finish()
    }

    pub(super) fn update_usage_anchor(
        &mut self,
        response: &crate::messages::ModelResponse,
        tools: &[serde_json::Value],
    ) {
        let Some(usage) = &response.usage else {
            return;
        };
        if usage.total_tokens == 0 {
            return;
        }
        self.usage_anchor = Some(UsageAnchor {
            total_tokens: usage
                .input_tokens
                .saturating_add(usage.cache_read_tokens)
                .saturating_add(usage.cache_write_tokens)
                .saturating_add(self.messages.last().map_or(0, |message| {
                    estimate_model_input_tokens(std::slice::from_ref(message))
                })),
            message_count: self.messages.len(),
            prefix_fingerprint: self.completion_prefix_fingerprint(tools),
        });
    }

    pub(super) fn compact_messages(&mut self, messages: &[Message]) -> (String, String) {
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

    pub(super) fn should_model_compact(&self) -> bool {
        self.config.model.provider != "fake"
    }

    pub(super) fn try_model_compact(
        &mut self,
        messages: &[Message],
    ) -> Result<String, CompactError> {
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
                    max_output_tokens: self.request_capabilities.max_output_tokens.min(2048),
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
}
