use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaPart {
    pub media_type: String,
    pub path: PathBuf,
    pub filename: String,
    pub content_type: String,
    pub caption: String,
}

impl MediaPart {
    pub fn new(
        media_type: impl Into<String>,
        path: PathBuf,
        filename: impl Into<String>,
        content_type: impl Into<String>,
        caption: impl Into<String>,
    ) -> Self {
        Self {
            media_type: media_type.into(),
            path,
            filename: filename.into(),
            content_type: content_type.into(),
            caption: caption.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub tool_call_id: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

impl Message {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn tool(content: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelLimits {
    pub timeout_seconds: u64,
    pub max_output_tokens: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentResponse {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolResult {
    pub ok: bool,
    pub text: String,
    pub error_type: Option<String>,
    pub truncated: bool,
    pub media: Option<MediaPart>,
}

impl ToolResult {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            ok: true,
            text: text.into(),
            error_type: None,
            truncated: false,
            media: None,
        }
    }

    pub fn error(kind: &str, text: impl Into<String>) -> Self {
        Self {
            ok: false,
            text: text.into(),
            error_type: Some(kind.to_string()),
            truncated: false,
            media: None,
        }
    }

    pub fn with_media(mut self, media: MediaPart) -> Self {
        self.media = Some(media);
        self
    }
}
