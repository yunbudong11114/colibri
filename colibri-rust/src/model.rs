use std::collections::BTreeMap;
use std::thread;
use std::time::Duration;

use crate::config::ModelConfig;
use crate::http::request_json;
use crate::messages::{Message, ModelLimits, ModelResponse, ToolCall};

pub trait ModelClient: Send {
    fn complete(
        &mut self,
        messages: &[Message],
        tools: &[serde_json::Value],
        system: &str,
        limits: &ModelLimits,
    ) -> Result<ModelResponse, String>;

    fn complete_image(
        &mut self,
        prompt: &str,
        image_data_url: &str,
        limits: &ModelLimits,
    ) -> Result<ModelResponse, String> {
        let _ = (prompt, image_data_url, limits);
        Err("Vision model is unavailable".to_string())
    }
}

pub struct FakeModel;

impl FakeModel {
    pub fn new() -> Self {
        Self
    }
}

impl ModelClient for FakeModel {
    fn complete(
        &mut self,
        messages: &[Message],
        _tools: &[serde_json::Value],
        _system: &str,
        _limits: &ModelLimits,
    ) -> Result<ModelResponse, String> {
        if let Some(tool_message) = messages.iter().rev().find(|message| message.role == "tool") {
            return Ok(ModelResponse {
                text: format!("final: {}", tool_message.content),
                tool_calls: Vec::new(),
            });
        }
        let user_text = messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(|message| message.content.as_str())
            .unwrap_or("");
        if let Some(call) = scripted_tool_call(user_text) {
            return Ok(ModelResponse {
                text: String::new(),
                tool_calls: vec![call],
            });
        }
        Ok(ModelResponse {
            text: format!("fake: {}", user_text),
            tool_calls: Vec::new(),
        })
    }

    fn complete_image(
        &mut self,
        prompt: &str,
        _image_data_url: &str,
        _limits: &ModelLimits,
    ) -> Result<ModelResponse, String> {
        Ok(ModelResponse {
            text: format!("fake image: {}", prompt),
            tool_calls: Vec::new(),
        })
    }
}

pub struct OpenAiCompatibleModel {
    config: ModelConfig,
}

impl OpenAiCompatibleModel {
    pub fn from_config(config: &ModelConfig) -> Result<Self, String> {
        let mut cloned = config.clone();
        if cloned.api_key.is_empty() {
            cloned.api_key = std::env::var("COLIBRI_API_KEY").unwrap_or_default();
        }
        if cloned.api_key.is_empty() {
            return Err("model.api_key or COLIBRI_API_KEY is required".to_string());
        }
        Ok(Self { config: cloned })
    }

    fn request_with_retry(
        &self,
        url: &str,
        payload: &str,
        timeout_seconds: u64,
    ) -> Result<String, String> {
        for attempt in 0..=self.config.max_retries {
            let result = request_json(
                "POST",
                url,
                &[("Authorization", format!("Bearer {}", self.config.api_key))],
                Some(payload),
                timeout_seconds,
            );
            let failure = match result {
                Ok(response) if response.status.is_none_or(|status| status < 400) => {
                    return Ok(response.body);
                }
                Ok(response) => model_failure_for_status(
                    response.status.unwrap_or(500),
                    format!("model request failed: {}", response.body),
                ),
                Err(error) => model_failure_for_transport(error),
            };
            if !failure.retryable || attempt >= self.config.max_retries {
                return Err(failure.encoded());
            }
            let delay_ms = retry_delay_ms(self.config.retry_backoff_ms, attempt);
            if delay_ms > 0 {
                thread::sleep(Duration::from_millis(delay_ms));
            }
        }
        Err("model_error:invalid_response:unreachable".to_string())
    }
}

fn retry_delay_ms(base_ms: u64, retry_index: usize) -> u64 {
    base_ms.saturating_mul(
        1u64
            .checked_shl(retry_index as u32)
            .unwrap_or(u64::MAX),
    )
}

impl ModelClient for OpenAiCompatibleModel {
    fn complete(
        &mut self,
        messages: &[Message],
        _tools: &[serde_json::Value],
        system: &str,
        limits: &ModelLimits,
    ) -> Result<ModelResponse, String> {
        let payload = chat_payload(
            &self.config.model,
            system,
            messages,
            _tools,
            limits.max_output_tokens,
        );
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let body = self.request_with_retry(&url, &payload, limits.timeout_seconds)?;
        parse_chat_response(&body)
    }

    fn complete_image(
        &mut self,
        prompt: &str,
        image_data_url: &str,
        limits: &ModelLimits,
    ) -> Result<ModelResponse, String> {
        let payload = image_payload(
            &self.config.model,
            prompt,
            image_data_url,
            limits.max_output_tokens,
        );
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let body = self.request_with_retry(&url, &payload, limits.timeout_seconds)?;
        parse_chat_response(&body)
    }
}

struct ModelFailure {
    category: &'static str,
    message: String,
    retryable: bool,
}

impl ModelFailure {
    fn encoded(self) -> String {
        format!("model_error:{}:{}", self.category, self.message)
    }
}

fn model_failure_for_status(status: u16, message: String) -> ModelFailure {
    let (category, retryable) = match status {
        408 => ("timeout", true),
        429 => ("rate_limit", true),
        500..=599 => ("server_error", true),
        _ => ("client_error", false),
    };
    ModelFailure {
        category,
        message,
        retryable,
    }
}

fn model_failure_for_transport(message: String) -> ModelFailure {
    let lower = message.to_ascii_lowercase();
    let category = if lower.contains("timed out") || lower.contains("timeout") {
        "timeout"
    } else {
        "transient_network"
    };
    ModelFailure {
        category,
        message,
        retryable: true,
    }
}

pub fn build_model(config: &ModelConfig) -> Result<Box<dyn ModelClient>, String> {
    match config.provider.as_str() {
        "fake" => Ok(Box::new(FakeModel::new())),
        "openai_compatible" => Ok(Box::new(OpenAiCompatibleModel::from_config(config)?)),
        other => Err(format!("Unsupported model provider: {}", other)),
    }
}

fn scripted_tool_call(text: &str) -> Option<ToolCall> {
    let rest = text.strip_prefix("tool:")?;
    let (name, args) = rest.split_once(' ').unwrap_or((rest, "{}"));
    let arguments = serde_json::from_str::<serde_json::Value>(args)
        .ok()?
        .as_object()
        .cloned()?;
    Some(ToolCall {
        id: "call_1".to_string(),
        name: name.trim().to_string(),
        arguments,
    })
}

fn chat_payload(
    model: &str,
    system: &str,
    messages: &[Message],
    tools: &[serde_json::Value],
    max_tokens: usize,
) -> String {
    let mut api_messages = Vec::new();
    if !system.is_empty() {
        api_messages.push(serde_json::json!({"role": "system", "content": system}));
    }
    for message in messages {
        api_messages.push(api_message(message));
    }
    let mut payload = serde_json::json!({
        "model": model,
        "messages": api_messages,
        "max_completion_tokens": max_tokens,
    });
    if !tools.is_empty() {
        payload["tools"] = serde_json::Value::Array(tools.to_vec());
    }
    serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
}

fn image_payload(model: &str, prompt: &str, image_data_url: &str, max_tokens: usize) -> String {
    let payload = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": prompt},
                    {"type": "image_url", "image_url": {"url": image_data_url}},
                ],
            }
        ],
        "max_completion_tokens": max_tokens,
    });
    serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
}

fn api_message(message: &Message) -> serde_json::Value {
    let mut value = serde_json::json!({
        "role": message.role,
        "content": message.content,
    });
    if let Some(tool_call_id) = &message.tool_call_id {
        value["tool_call_id"] = serde_json::json!(tool_call_id);
    }
    if !message.tool_calls.is_empty() {
        value["tool_calls"] = serde_json::Value::Array(
            message
                .tool_calls
                .iter()
                .map(|call| {
                    serde_json::json!({
                        "id": call.id,
                        "type": "function",
                        "function": {
                            "name": call.name,
                            "arguments": serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_string())
                        }
                    })
                })
                .collect(),
        );
    }
    value
}

fn parse_chat_response(body: &str) -> Result<ModelResponse, String> {
    let data: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| format!("model_error:invalid_response:Model response was not valid JSON: {}", error))?;
    let message = data
        .get("choices")
        .and_then(|choices| choices.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .ok_or_else(|| "model_error:invalid_response:Model response missing choices".to_string())?;
    let text = message
        .get("content")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let tool_calls = message
        .get("tool_calls")
        .and_then(|value| value.as_array())
        .map(|items| items.iter().map(parse_tool_call).collect())
        .unwrap_or_else(Vec::new);
    Ok(ModelResponse { text, tool_calls })
}

fn parse_tool_call(value: &serde_json::Value) -> ToolCall {
    let function = value.get("function").unwrap_or(&serde_json::Value::Null);
    let raw_arguments = function
        .get("arguments")
        .and_then(|value| value.as_str())
        .unwrap_or("{}");
    let arguments = serde_json::from_str::<serde_json::Value>(raw_arguments)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_else(|| {
            let mut fallback = serde_json::Map::new();
            fallback.insert(
                "raw".to_string(),
                serde_json::Value::String(raw_arguments.to_string()),
            );
            fallback
        });
    ToolCall {
        id: value
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        name: function
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        arguments,
    }
}

#[cfg(test)]
mod tests {
    use super::retry_delay_ms;

    #[test]
    fn retry_delay_uses_deterministic_exponential_backoff() {
        assert_eq!(retry_delay_ms(500, 0), 500);
        assert_eq!(retry_delay_ms(500, 1), 1000);
        assert_eq!(retry_delay_ms(500, 2), 2000);
        assert_eq!(retry_delay_ms(0, 4), 0);
    }
}

pub fn escape_json(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| match ch {
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect::<Vec<_>>(),
            '\t' => "\\t".chars().collect::<Vec<_>>(),
            other => vec![other],
        })
        .collect()
}

pub fn map_to_json(map: &BTreeMap<String, String>) -> String {
    let entries = map
        .iter()
        .map(|(key, value)| format!("\"{}\":\"{}\"", escape_json(key), escape_json(value)))
        .collect::<Vec<_>>();
    format!("{{{}}}", entries.join(","))
}
