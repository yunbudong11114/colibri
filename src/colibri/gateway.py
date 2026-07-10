from __future__ import annotations

import queue
from dataclasses import dataclass
from pathlib import Path
import threading
from time import monotonic
from typing import Callable

from colibri.channels.base import Channel, ChannelContext, InboundMessage
from colibri.channels.weixin import WeixinChannel, WeixinPermissionPrompter
from colibri.config import AgentConfig, ConfigError
from colibri.media import MediaPart
from colibri.model.base import ModelClient
from colibri.session import AgentSession
from colibri.tools.permissions import PermissionPolicy
from colibri.tools.registry import ToolRegistry
from colibri.transcript import ScopedTranscriptWriter, TranscriptSink, TranscriptWriter


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
        transcript: TranscriptSink | None = None,
        monotonic_func: Callable[[], float] = monotonic,
    ):
        self.config = config
        self.model = model
        self.registry = registry
        self.max_sessions = max(1, max_sessions)
        self.idle_seconds = idle_seconds
        self.transcript = transcript
        self.monotonic = monotonic_func
        self._entries: dict[str, GatewaySessionEntry] = {}
        self._lock = threading.Lock()

    def get(
        self,
        key: str,
        policy: PermissionPolicy,
        transcript_metadata: dict[str, str] | None = None,
        media_sender: Callable[[MediaPart], None] | None = None,
    ) -> AgentSession:
        with self._lock:
            self._evict_idle_locked()
            now = self.monotonic()
            entry = self._entries.get(key)
            if entry is not None:
                entry.last_activity_at = now
                entry.session.media_sender = media_sender
                return entry.session

            while len(self._entries) >= self.max_sessions:
                self._evict_oldest_locked()
            session = AgentSession(
                config=self.config,
                model=self.model,
                tools=self.registry,
                permission_policy=policy,
                transcript=ScopedTranscriptWriter(self.transcript, transcript_metadata or {"session_key": key})
                if self.transcript is not None
                else None,
                media_sender=media_sender,
            )
            self._entries[key] = GatewaySessionEntry(session=session, last_activity_at=now)
            return session

    def touch(self, key: str) -> None:
        with self._lock:
            entry = self._entries.get(key)
            if entry is not None:
                entry.last_activity_at = self.monotonic()

    def close(self) -> None:
        with self._lock:
            for entry in self._entries.values():
                entry.session.close()
            self._entries.clear()
            if self.transcript is not None:
                self.transcript.close()
                self.transcript = None

    def _evict_idle_locked(self) -> None:
        if self.idle_seconds <= 0:
            return
        now = self.monotonic()
        for key, entry in list(self._entries.items()):
            if now - entry.last_activity_at >= self.idle_seconds:
                entry.session.close()
                del self._entries[key]

    def _evict_oldest_locked(self) -> None:
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
        transcript = TranscriptWriter.default() if config.session.transcript else None
        self.sessions = GatewaySessionCache(
            config=config,
            model=model,
            registry=self.registry,
            max_sessions=config.gateway.max_sessions,
            idle_seconds=config.gateway.session_idle_seconds,
            transcript=transcript,
        )

    def run(self, stop_requested: Callable[[], bool] = lambda: False) -> None:
        channels = self._build_channels()
        if not channels:
            raise ConfigError("No gateway channels are enabled")
        context = ChannelContext(stop_requested=stop_requested)
        errors: queue.Queue[BaseException] = queue.Queue()
        threads = [
            threading.Thread(
                target=self._run_channel,
                args=(channel, context, errors),
                name=f"colibri-{channel.name}",
                daemon=True,
            )
            for channel in channels
        ]
        try:
            for thread in threads:
                thread.start()

            while not stop_requested():
                try:
                    error = errors.get(timeout=0.2)
                except queue.Empty:
                    if not any(thread.is_alive() for thread in threads):
                        return
                    continue
                raise error
        finally:
            self.sessions.close()

    def _run_channel(
        self,
        channel: Channel,
        context: ChannelContext,
        errors: queue.Queue[BaseException],
    ) -> None:
        try:
            channel.run(lambda message, ch=channel: self.handle_message(ch, message), context)
        except BaseException as error:
            errors.put(error)

    def handle_message(self, channel: Channel, message: InboundMessage) -> str:
        key = f"{message.channel}:{message.sender_id}"
        policy = PermissionPolicy.from_config(
            self.config,
            prompter=WeixinPermissionPrompter(channel, message.sender_id)
            if message.channel == "weixin" and isinstance(channel, WeixinChannel)
            else None,
            cwd=self.registry.cwd,
        )
        session = self.sessions.get(
            key,
            policy,
            transcript_metadata={
                "channel": message.channel,
                "sender_id": message.sender_id,
                "session_key": key,
            },
            media_sender=lambda media, ch=channel, recipient=message.sender_id: ch.send_media(recipient, media),
        )
        response = session.submit(message.text)
        self.sessions.touch(key)
        return response.text

    def _build_channels(self) -> list[Channel]:
        channels: list[Channel] = []
        enabled = set(self.config.gateway.enabled_channels)
        if "weixin" in enabled and self.config.channels.weixin.enabled:
            channels.append(WeixinChannel(self.config.channels.weixin))
        return channels
