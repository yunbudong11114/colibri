from __future__ import annotations

from collections import defaultdict, deque
import json
import os
from pathlib import Path
from typing import Any

from colibri.config import SessionConfig
from colibri.messages import Message


_ATTACHMENT_MARKER = "Attachments saved locally:"


class TranscriptHistoryLoader:
    def __init__(
        self,
        colibri_home: Path,
        *,
        message_limit: int,
        char_limit: int,
        scan_bytes: int,
    ):
        self.transcript_dir = colibri_home.expanduser() / "transcripts"
        self.message_limit = max(0, message_limit)
        self.char_limit = max(0, char_limit)
        self.scan_bytes = max(0, scan_bytes)

    @classmethod
    def default(cls, config: SessionConfig) -> "TranscriptHistoryLoader":
        home = Path(os.environ.get("COLIBRI_HOME", "~/.colibri")).expanduser()
        return cls(
            home,
            message_limit=config.restore_message_limit,
            char_limit=config.restore_char_limit,
            scan_bytes=config.restore_scan_bytes,
        )

    def __call__(self) -> list[Message]:
        return self.load()

    def load(self) -> list[Message]:
        if self.message_limit < 2 or self.char_limit == 0 or self.scan_bytes == 0:
            return []
        turns = self._completed_turns(self._recent_lines())
        selected: list[tuple[str, str]] = []
        selected_chars = 0
        for user_text, assistant_text in reversed(turns):
            turn_chars = len(user_text) + len(assistant_text)
            if len(selected) * 2 + 2 > self.message_limit:
                break
            if selected_chars + turn_chars > self.char_limit:
                break
            selected.append((user_text, assistant_text))
            selected_chars += turn_chars
        messages: list[Message] = []
        for user_text, assistant_text in reversed(selected):
            messages.append(Message(role="user", content=user_text))
            messages.append(Message(role="assistant", content=assistant_text))
        return messages

    def _recent_lines(self) -> list[str]:
        try:
            files = sorted(self.transcript_dir.glob("*.jsonl"), reverse=True)
        except OSError:
            return []
        remaining = self.scan_bytes
        chunks: list[list[str]] = []
        for path in files:
            if remaining <= 0:
                break
            try:
                size = path.stat().st_size
                read_size = min(size, remaining)
                start = size - read_size
                with path.open("rb") as handle:
                    starts_mid_line = False
                    if start > 0:
                        handle.seek(start - 1)
                        starts_mid_line = handle.read(1) != b"\n"
                    handle.seek(start)
                    data = handle.read(read_size)
            except OSError:
                continue
            remaining -= read_size
            lines = data.decode("utf-8", errors="ignore").splitlines()
            if starts_mid_line and lines:
                lines.pop(0)
            chunks.append(lines)
        lines: list[str] = []
        for chunk in reversed(chunks):
            lines.extend(chunk)
        return lines

    @staticmethod
    def _completed_turns(lines: list[str]) -> list[tuple[str, str]]:
        pending: dict[str, deque[str]] = defaultdict(deque)
        completed: list[tuple[str, str]] = []
        for line in lines:
            event = _parse_event(line)
            if event is None:
                continue
            event_type, payload = event
            source = _source_key(payload)
            text = payload.get("text")
            if not isinstance(text, str) or not text.strip():
                continue
            if event_type == "user_message":
                cleaned = _strip_attachment_paths(text)
                if cleaned:
                    pending[source].append(cleaned)
            elif event_type == "assistant_message" and payload.get("tool_call_count") == 0:
                if pending[source]:
                    completed.append((pending[source].popleft(), text))
        return completed


def _parse_event(line: str) -> tuple[str, dict[str, Any]] | None:
    try:
        event = json.loads(line)
    except (json.JSONDecodeError, TypeError):
        return None
    if not isinstance(event, dict):
        return None
    event_type = event.get("type")
    payload = event.get("payload")
    if not isinstance(event_type, str) or not isinstance(payload, dict):
        return None
    return event_type, payload


def _source_key(payload: dict[str, Any]) -> str:
    session_key = payload.get("session_key")
    if isinstance(session_key, str) and session_key:
        return session_key
    channel = payload.get("channel")
    sender_id = payload.get("sender_id")
    if isinstance(channel, str) and isinstance(sender_id, str) and channel and sender_id:
        return f"{channel}:{sender_id}"
    return "local"


def _strip_attachment_paths(text: str) -> str:
    return text.partition(_ATTACHMENT_MARKER)[0].rstrip()
