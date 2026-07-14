from __future__ import annotations

from colibri.channels.base import Channel
from colibri.channels.weixin import WeixinChannel
from colibri.config import AgentConfig, ConfigError


def build_channel_registry(channels: list[Channel]) -> dict[str, Channel]:
    registry: dict[str, Channel] = {}
    for channel in channels:
        if channel.name in registry:
            raise ConfigError(f"duplicate gateway channel: {channel.name}")
        registry[channel.name] = channel
    return registry


def build_enabled_channels(config: AgentConfig) -> list[Channel]:
    """Assemble gateway channels from config. Add new adapters here only."""
    channels: list[Channel] = []
    enabled = set(config.gateway.enabled_channels)
    if "weixin" in enabled and config.channels.weixin.enabled:
        channels.append(WeixinChannel(config.channels.weixin))
    return channels
