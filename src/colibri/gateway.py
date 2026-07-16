from __future__ import annotations

import queue
from dataclasses import dataclass
from pathlib import Path
import os
import threading
from time import monotonic
from typing import Callable

from colibri.channels.base import Channel, ChannelContext, InboundMessage
from colibri.channels.registry import build_channel_registry, build_enabled_channels
from colibri.config import AgentConfig, ConfigError
from colibri.config import DEFAULT_USER_CONFIG, expand_user_path
from colibri.gateway_process import GatewayAgentHealth
from colibri.gateway_logging import gateway_log
from colibri.model.factory import build_model_client
from colibri.runtime_reload import PartialRuntimeReloader
from colibri.inbound_router import InboundRouter
from colibri.media import MediaPart
from colibri.messages import Message
from colibri.model.base import ModelClient
from colibri.session import AgentSession
from colibri.session_history import TranscriptHistoryLoader
from colibri.tools.permissions import PermissionPolicy
from colibri.tools.registry import ToolRegistry
from colibri.transcript import ScopedTranscriptWriter, TranscriptSink, TranscriptWriter

WORK_QUEUE_WAIT_SECONDS = 0.05
WORKER_JOIN_SECONDS = 1.0


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
        history_loader: Callable[[], list[Message]] | None = None,
        monotonic_func: Callable[[], float] = monotonic,
    ):
        self.config = config
        self.model = model
        self.registry = registry
        self.max_sessions = max(1, max_sessions)
        self.idle_seconds = idle_seconds
        self.transcript = transcript
        self.history_loader = history_loader
        self.monotonic = monotonic_func
        self._entries: dict[str, GatewaySessionEntry] = {}
        # Sessions currently mid-submit stay steerable while outside `_entries`.
        self._steer_sessions: dict[str, AgentSession] = {}
        self._lock = threading.Lock()

    def get(
        self,
        key: str,
        policy: PermissionPolicy,
        transcript_metadata: dict[str, str] | None = None,
        media_sender: Callable[[MediaPart], None] | None = None,
    ) -> AgentSession:
        with self._lock:
            return self._get_or_create_locked(key, policy, transcript_metadata, media_sender)

    def take_or_create(
        self,
        key: str,
        policy: PermissionPolicy,
        transcript_metadata: dict[str, str] | None = None,
        media_sender: Callable[[MediaPart], None] | None = None,
    ) -> AgentSession:
        with self._lock:
            self._evict_idle_locked()
            entry = self._entries.pop(key, None)
            if entry is not None:
                entry.session.media_sender = media_sender
                self._steer_sessions[key] = entry.session
                return entry.session
            session = self._create_session_locked(key, policy, transcript_metadata, media_sender)
            self._steer_sessions[key] = session
            return session

    def put_back(self, key: str, session: AgentSession) -> None:
        with self._lock:
            self._steer_sessions[key] = session
            self._entries[key] = GatewaySessionEntry(session=session, last_activity_at=self.monotonic())

    def get_existing(self, key: str) -> AgentSession | None:
        with self._lock:
            entry = self._entries.get(key)
            if entry is not None:
                return entry.session
            return self._steer_sessions.get(key)

    def try_steer(self, key: str, text: str) -> bool:
        with self._lock:
            session = self._steer_sessions.get(key)
            if session is None:
                entry = self._entries.get(key)
                session = entry.session if entry is not None else None
            if session is None:
                return False
        return session.steer(text)

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
            self._steer_sessions.clear()
            if self.transcript is not None:
                self.transcript.close()
                self.transcript = None

    def apply_runtime(self, config: AgentConfig, model: ModelClient, registry: ToolRegistry) -> None:
        with self._lock:
            self.config = config
            self.model = model
            self.registry = registry
            for entry in self._entries.values():
                _adopt_session_runtime(entry.session, config, model, registry)

    def _get_or_create_locked(
        self,
        key: str,
        policy: PermissionPolicy,
        transcript_metadata: dict[str, str] | None,
        media_sender: Callable[[MediaPart], None] | None,
    ) -> AgentSession:
        self._evict_idle_locked()
        now = self.monotonic()
        entry = self._entries.get(key)
        if entry is not None:
            entry.last_activity_at = now
            entry.session.media_sender = media_sender
            self._steer_sessions[key] = entry.session
            return entry.session
        session = self._create_session_locked(key, policy, transcript_metadata, media_sender)
        self._entries[key] = GatewaySessionEntry(session=session, last_activity_at=now)
        self._steer_sessions[key] = session
        return session

    def _create_session_locked(
        self,
        key: str,
        policy: PermissionPolicy,
        transcript_metadata: dict[str, str] | None,
        media_sender: Callable[[MediaPart], None] | None,
    ) -> AgentSession:
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
            history_loader=self.history_loader,
        )
        return session

    def _evict_idle_locked(self) -> None:
        if self.idle_seconds <= 0:
            return
        now = self.monotonic()
        for key, entry in list(self._entries.items()):
            if now - entry.last_activity_at >= self.idle_seconds:
                entry.session.close()
                del self._entries[key]
                self._steer_sessions.pop(key, None)

    def _evict_oldest_locked(self) -> None:
        if not self._entries:
            return
        oldest_key = min(self._entries, key=lambda key: self._entries[key].last_activity_at)
        self._entries[oldest_key].session.close()
        del self._entries[oldest_key]
        self._steer_sessions.pop(oldest_key, None)


class GatewayRunner:
    def __init__(
        self,
        config: AgentConfig,
        model: ModelClient,
        *,
        registry: ToolRegistry | None = None,
        cwd: Path | None = None,
        config_path: Path | None = None,
        health: GatewayAgentHealth | None = None,
    ):
        self.config = config
        self.model = model
        self.registry = registry or ToolRegistry.from_config(config, cwd=cwd)
        self.config_path = (config_path or expand_user_path(DEFAULT_USER_CONFIG)).expanduser()
        self._runtime_reloader = PartialRuntimeReloader(
            self.config_path,
            config,
            model,
            model_builder=build_model_client,
        )
        self._runtime_lock = threading.Lock()
        self.health = health or GatewayAgentHealth()
        transcript = (
            TranscriptWriter.default(
                retention_days=config.session.transcript_retention_days,
                max_total_bytes=config.session.transcript_max_total_bytes,
            )
            if config.session.transcript
            else None
        )
        history_loader = (
            TranscriptHistoryLoader.default(config.session)
            if config.session.restore_transcript
            else None
        )
        self.sessions = GatewaySessionCache(
            config=config,
            model=model,
            registry=self.registry,
            max_sessions=config.gateway.max_sessions,
            idle_seconds=config.gateway.session_idle_seconds,
            transcript=transcript,
            history_loader=history_loader,
        )

    def try_steer(self, channel_name: str, sender_id: str, text: str) -> bool:
        key = f"{channel_name}:{sender_id}"
        return self.sessions.try_steer(key, text)

    def run(self, stop_requested: Callable[[], bool] = lambda: False) -> None:
        gateway_log(f"started pid={os.getpid()} model={self.config.model.model}")
        channels = self._build_channels()
        if not channels:
            raise ConfigError("No gateway channels are enabled")
        channels_by_name = build_channel_registry(channels)
        router: InboundRouter[InboundMessage] = InboundRouter(
            max(1, self.config.gateway.max_pending_inbound)
        )
        errors: queue.Queue[BaseException] = queue.Queue()
        stop_event = threading.Event()
        max_turns = max(1, self.config.gateway.max_concurrent_turns)
        workers = [
            threading.Thread(
                target=self._run_turn_worker,
                args=(router, channels_by_name, errors, stop_event),
                name=f"colibri-gateway-turn-{index}",
                daemon=True,
            )
            for index in range(max_turns)
        ]
        pollers = [
            threading.Thread(
                target=self._run_channel_poll,
                args=(channel, router, stop_requested, stop_event, errors),
                name=f"colibri-{channel.name}-poll",
                daemon=True,
            )
            for channel in channels
        ]
        try:
            for worker in workers:
                worker.start()
            for poller in pollers:
                poller.start()

            while not stop_requested():
                try:
                    error = errors.get(timeout=0.2)
                except queue.Empty:
                    if not any(thread.is_alive() for thread in pollers):
                        while not router.wait_idle(timeout=WORK_QUEUE_WAIT_SECONDS):
                            if stop_event.is_set():
                                try:
                                    raise errors.get_nowait()
                                except queue.Empty:
                                    return
                        return
                    continue
                gateway_log(f"supervisor error: {error}")
                raise error
        finally:
            stop_event.set()
            router.close()
            for worker in workers:
                worker.join()
            for poller in pollers:
                poller.join(timeout=WORKER_JOIN_SECONDS)
            self.sessions.close()

    def _run_channel_poll(
        self,
        channel: Channel,
        router: InboundRouter[InboundMessage],
        stop_requested: Callable[[], bool],
        stop_event: threading.Event,
        errors: queue.Queue[BaseException],
    ) -> None:
        def offer(message: InboundMessage) -> bool:
            if message.channel != channel.name:
                raise ConfigError(
                    f"channel adapter mismatch: expected {channel.name}, got {message.channel}"
                )
            key = f"{message.channel}:{message.sender_id}"
            return router.try_enqueue(key, message)

        channel_context = ChannelContext(
            stop_requested=lambda: stop_requested() or stop_event.is_set(),
            try_steer=lambda sender_id, text, name=channel.name: self.try_steer(name, sender_id, text),
        )
        try:
            channel.run_poll(offer, channel_context)
        except BaseException as error:
            gateway_log(f"channel={channel.name} poller failed: {error}")
            errors.put(error)
            stop_event.set()
            router.close()

    def _run_turn_worker(
        self,
        router: InboundRouter[InboundMessage],
        channels_by_name: dict[str, Channel],
        errors: queue.Queue[BaseException],
        stop_event: threading.Event,
    ) -> None:
        try:
            while not stop_event.is_set():
                acquired = router.acquire(timeout=WORK_QUEUE_WAIT_SECONDS)
                if acquired is None:
                    if stop_event.is_set():
                        return
                    continue
                key, message = acquired
                try:
                    channel = channels_by_name.get(message.channel)
                    if channel is None:
                        continue
                    resolved = channel.resolve_inbound_media(message)
                    reply = self.handle_message(channel, resolved)
                    if reply.strip():
                        channel.send_text(resolved.sender_id, reply)
                finally:
                    router.release(key)
        except BaseException as error:
            gateway_log(f"turn worker failed: {error}")
            errors.put(error)
            stop_event.set()
            router.close()

    def handle_message(self, channel: Channel, message: InboundMessage) -> str:
        self._reload_config_if_changed()
        key = f"{message.channel}:{message.sender_id}"
        prompter = channel.permission_prompter(message.sender_id)
        policy = PermissionPolicy.from_config(
            self.config,
            prompter=prompter,
            cwd=self.registry.cwd,
        )
        outbound = OutboundSink(channel, message.sender_id)
        session = self.sessions.take_or_create(
            key,
            policy,
            transcript_metadata={
                "channel": message.channel,
                "sender_id": message.sender_id,
                "session_key": key,
            },
            media_sender=outbound.send_media,
        )
        if session.config is not self.config or session.model is not self.model:
            _adopt_session_runtime(session, self.config, self.model, self.registry)
        session.steer_notifier = outbound.send_ack
        try:
            if session.is_turn_active():
                if session.steer(message.text):
                    return ""
            response = session.submit(message.text, media=message.media)
            self.health.report("unhealthy" if response.error_type else "healthy")
            return response.text
        except Exception:
            self.health.report("unhealthy")
            raise
        finally:
            self.sessions.put_back(key, session)
            self.sessions.touch(key)

    def _reload_config_if_changed(self) -> None:
        with self._runtime_lock:
            result = self._runtime_reloader.reload_if_changed()
            if result.error is not None:
                gateway_log(f"config reload skipped: {result.error}")
                return
            if result.snapshot is None:
                return
            config = result.snapshot.config
            model = result.snapshot.model
            registry = ToolRegistry.from_config(config, cwd=self.registry.cwd)
            self.config = config
            self.model = model
            self.registry = registry
            self.sessions.apply_runtime(config, model, registry)
            gateway_log(f"config reloaded model={config.model.model}")

    def _build_channels(self) -> list[Channel]:
        return build_enabled_channels(self.config)


class OutboundSink:
    """Channel-agnostic outbound path for ack / text / media."""

    def __init__(self, channel: Channel, recipient_id: str):
        self.channel = channel
        self.recipient_id = recipient_id

    def send_ack(self, text: str) -> None:
        self.channel.send_text(self.recipient_id, text)

    def send_text(self, text: str) -> None:
        self.channel.send_text(self.recipient_id, text)

    def send_media(self, media: MediaPart) -> None:
        self.channel.send_media(self.recipient_id, media)


def _adopt_session_runtime(
    session: AgentSession,
    config: AgentConfig,
    model: ModelClient,
    registry: ToolRegistry,
) -> None:
    session.config = config
    session.model = model
    session.tools = registry
    session._image_analyzer = None
