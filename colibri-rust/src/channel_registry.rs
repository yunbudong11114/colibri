use std::sync::Arc;

use crate::channel::{build_channel_registry, ChannelRegistry, GatewayChannel};
use crate::config::AgentConfig;
use crate::weixin::WeixinGatewayChannel;

pub fn build_enabled_channels(config: &AgentConfig) -> Result<ChannelRegistry, String> {
    let mut channels: Vec<Arc<dyn GatewayChannel>> = Vec::new();
    if config
        .gateway
        .enabled_channels
        .iter()
        .any(|name| name == "weixin")
        && config.channels_weixin.enabled
    {
        channels.push(Arc::new(WeixinGatewayChannel::new(config.clone())));
    }
    build_channel_registry(channels)
}
