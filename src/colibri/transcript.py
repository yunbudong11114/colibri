from __future__ import annotations

from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import threading
import time
from typing import Any, Callable, Protocol, TextIO

BEIJING_TZ = timezone(timedelta(hours=8))


def beijing_now() -> datetime:
    return datetime.now(BEIJING_TZ)


def format_beijing_timestamp(moment: datetime | None = None) -> str:
    return (moment or beijing_now()).isoformat(timespec="seconds")


def beijing_date(moment: datetime | None = None) -> str:
    return (moment or beijing_now()).strftime("%Y-%m-%d")


class TranscriptSink(Protocol):
    def write(self, event_type: str, payload: dict[str, Any]) -> None:
        ...

    def close(self) -> None:
        ...


class TranscriptWriter:
    def __init__(
        self,
        path: Path,
        *,
        retention_days: int = 0,
        max_total_bytes: int = 0,
        cleanup_interval_seconds: float = 60,
        time_func: Callable[[], float] = time.time,
    ):
        self.path = path
        self.retention_days = max(0, retention_days)
        self.max_total_bytes = max(0, max_total_bytes)
        self.cleanup_interval_seconds = max(0, cleanup_interval_seconds)
        self._time = time_func
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._file: TextIO | None = self.path.open("a", encoding="utf-8")
        self._lock = threading.Lock()
        self._last_cleanup_at = self._time()
        self._cleanup()

    @classmethod
    def default(cls, *, retention_days: int = 0, max_total_bytes: int = 0) -> "TranscriptWriter":
        home = Path(os.environ.get("COLIBRI_HOME", "~/.colibri")).expanduser()
        today = beijing_date()
        return cls(
            home / "transcripts" / f"{today}.jsonl",
            retention_days=retention_days,
            max_total_bytes=max_total_bytes,
        )

    def write(self, event_type: str, payload: dict[str, Any]) -> None:
        with self._lock:
            if self._file is None:
                return
            now = self._time()
            if now - self._last_cleanup_at >= self.cleanup_interval_seconds:
                self._last_cleanup_at = now
                self._cleanup(now)
            event = {
                "ts": format_beijing_timestamp(),
                "type": event_type,
                "payload": payload,
            }
            self._file.write(json.dumps(event, ensure_ascii=False, separators=(",", ":")) + "\n")
            self._file.flush()

    def close(self) -> None:
        with self._lock:
            if self._file is not None:
                self._file.close()
                self._file = None

    def _cleanup(self, now: float | None = None) -> None:
        if self.retention_days == 0 and self.max_total_bytes == 0:
            return
        current_time = self._time() if now is None else now
        try:
            paths = list(self.path.parent.glob("*.jsonl"))
        except OSError:
            return

        if self.retention_days > 0:
            cutoff = current_time - self.retention_days * 86400
            for path in paths:
                if path == self.path:
                    continue
                try:
                    if path.stat().st_mtime < cutoff:
                        path.unlink()
                except OSError:
                    continue

        if self.max_total_bytes <= 0:
            return
        files: list[tuple[float, Path, int]] = []
        total_bytes = 0
        for path in paths:
            try:
                stat = path.stat()
            except OSError:
                continue
            total_bytes += stat.st_size
            if path != self.path:
                files.append((stat.st_mtime, path, stat.st_size))
        for _mtime, path, size in sorted(files):
            if total_bytes <= self.max_total_bytes:
                break
            try:
                path.unlink()
            except OSError:
                continue
            total_bytes -= size


class ScopedTranscriptWriter:
    def __init__(self, transcript: TranscriptSink, metadata: dict[str, Any]):
        self.transcript = transcript
        self.metadata = dict(metadata)

    def write(self, event_type: str, payload: dict[str, Any]) -> None:
        self.transcript.write(event_type, {**payload, **self.metadata})

    def close(self) -> None:
        return
