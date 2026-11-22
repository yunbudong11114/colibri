use std::collections::BTreeMap;
use std::sync::Arc;

use crate::messages::MediaPart;

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
        self.context
            .get("context_token")
            .map(String::as_str)
            .unwrap_or("")
    }
}

/// Serial outbound path for one recipient (ack / text / media).
pub trait OutboundSink: Send + Sync {
    fn send_text(&self, text: &str) -> Result<(), String>;
    fn send_media(&self, media: &MediaPart) -> Result<(), String>;

    fn send_ack(&self, text: &str) {
        let _ = self.send_text(text);
    }
}

pub trait GatewayChannel: Send + Sync {
    fn name(&self) -> &str;
    fn poll_once(&self) -> Result<Vec<InboundEnvelope>, ChannelPollError>;
    fn resolve_inbound_media(&self, envelope: &mut InboundEnvelope) -> Result<(), String>;
    fn outbound_for(&self, envelope: &InboundEnvelope) -> Result<Arc<dyn OutboundSink>, String>;
}

#[derive(Debug)]
pub enum ChannelPollError {
    Retryable(String),
    AuthRequired,
    Permanent(String),
}

pub type ChannelRegistry = BTreeMap<String, Arc<dyn GatewayChannel>>;

pub fn build_channel_registry(
    channels: Vec<Arc<dyn GatewayChannel>>,
) -> Result<ChannelRegistry, String> {
    let mut registry = BTreeMap::new();
    for channel in channels {
        let name = channel.name().to_string();
        if registry.contains_key(&name) {
            return Err(format!("duplicate gateway channel: {name}"));
        }
        registry.insert(name, channel);
    }
    Ok(registry)
}

pub fn validate_channel_envelope(
    channel: &dyn GatewayChannel,
    envelope: &InboundEnvelope,
) -> Result<(), String> {
    if envelope.channel == channel.name() {
        Ok(())
    } else {
        Err(format!(
            "channel adapter mismatch: expected {}, got {}",
            channel.name(),
            envelope.channel
        ))
    }
}
