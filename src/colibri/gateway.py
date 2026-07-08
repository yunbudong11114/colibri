from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from time import monotonic
from typing import Callable

from colibri.channels.base import Channel, ChannelContext, InboundMessage
from colibri.channels.weixin import WeixinChannel, WeixinPermissionPrompter
from colibri.config import AgentConfig, ConfigError
from colibri.model.base import ModelClient
from colibri.session import AgentSession
from colibri.tools.permissions import PermissionPolicy
from colibri.tools.registry import ToolRegistry


@dataclass
class GatewaySessionEntry:
    session: AgentSession
    last_activity_at: float


class GatewaySessionCache:
    def __init__(
        self,
        config: AgentConfig,
        model: ModelClient,
        registry: ToolRegistry,
        max_sessions: int,
        idle_seconds: int,
        monotonic_func: Callable[[], float] = monotonic,
    ):
        self.config = config
        self.model = model
        self.registry = registry
        self.max_sessions = max(1, max_sessions)
        self.idle_seconds = idle_seconds
        self.monotonic = monotonic_func
        self._entries: dict[str, GatewaySessionEntry] = {}

    def get(self, key: str, policy: PermissionPolicy) -> AgentSession:
        self._evict_idle()
        now = self.monotonic()
        entry = self._entries.get(key)
        if entry is not None:
            entry.last_activity_at = now
            return entry.session

        while len(self._entries) >= self.max_sessions:
            self._evict_oldest()
        session = AgentSession(
            config=self.config,
            model=self.model,
            tools=self.registry,
            permission_policy=policy,
        )
        self._entries[key] = GatewaySessionEntry(session=session, last_activity_at=now)
        return session

    def touch(self, key: str) -> None:
        entry = self._entries.get(key)
        if entry is not None:
            entry.last_activity_at = self.monotonic()

    def close(self) -> None:
        for entry in self._entries.values():
            entry.session.close()
        self._entries.clear()

    def _evict_idle(self) -> None:
        if self.idle_seconds <= 0:
            return
        now = self.monotonic()
        for key, entry in list(self._entries.items()):
            if now - entry.last_activity_at >= self.idle_seconds:
                entry.session.close()
                del self._entries[key]

    def _evict_oldest(self) -> None:
        if not self._entries:
            return
        oldest_key = min(self._entries, key=lambda key: self._entries[key].last_activity_at)
        self._entries[oldest_key].session.close()
        del self._entries[oldest_key]


class GatewayRunner:
    def __init__(
        self,
        config: AgentConfig,
        model: ModelClient,
        *,
        registry: ToolRegistry | None = None,
        cwd: Path | None = None,
    ):
        self.config = config
        self.model = model
        self.registry = registry or ToolRegistry.from_config(config, cwd=cwd)
        self.sessions = GatewaySessionCache(
            config=config,
            model=model,
            registry=self.registry,
            max_sessions=config.gateway.max_sessions,
            idle_seconds=config.gateway.session_idle_seconds,
        )

    def run(self, stop_requested: Callable[[], bool] = lambda: False) -> None:
        channels = self._build_channels()
        if not channels:
            raise ConfigError("No gateway channels are enabled")
        context = ChannelContext(stop_requested=stop_requested)
        try:
            for channel in channels:
                channel.run(lambda message, ch=channel: self.handle_message(ch, message), context)
        finally:
            self.sessions.close()

    def handle_message(self, channel: Channel, message: InboundMessage) -> str:
        key = f"{message.channel}:{message.sender_id}"
        policy = PermissionPolicy.from_config(
            self.config,
            prompter=WeixinPermissionPrompter(channel, message.sender_id)
            if message.channel == "weixin" and isinstance(channel, WeixinChannel)
            else None,
            cwd=self.registry.cwd,
        )
        session = self.sessions.get(key, policy)
        response = session.submit(message.text)
        self.sessions.touch(key)
        return response.text

    def _build_channels(self) -> list[Channel]:
        channels: list[Channel] = []
        enabled = set(self.config.gateway.enabled_channels)
        if "weixin" in enabled and self.config.channels.weixin.enabled:
            channels.append(WeixinChannel(self.config.channels.weixin))
        return channels
