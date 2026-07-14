from __future__ import annotations

from colibri.channels.base import Channel
from colibri.channels.weixin import WeixinChannel
from colibri.config import AgentConfig


def build_enabled_channels(config: AgentConfig) -> list[Channel]:
    """Assemble gateway channels from config. Add new adapters here only."""
    channels: list[Channel] = []
    enabled = set(config.gateway.enabled_channels)
    if "weixin" in enabled and config.channels.weixin.enabled:
        channels.append(WeixinChannel(config.channels.weixin))
    return channels
