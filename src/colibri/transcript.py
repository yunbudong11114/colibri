from __future__ import annotations

from datetime import datetime, timezone
import json
import os
from pathlib import Path
import threading
from typing import Any, Protocol, TextIO


class TranscriptSink(Protocol):
    def write(self, event_type: str, payload: dict[str, Any]) -> None:
        ...

    def close(self) -> None:
        ...


class TranscriptWriter:
    def __init__(self, path: Path):
        self.path = path
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._file: TextIO | None = self.path.open("a", encoding="utf-8")
        self._lock = threading.Lock()

    @classmethod
    def default(cls) -> "TranscriptWriter":
        home = Path(os.environ.get("COLIBRI_HOME", "~/.colibri")).expanduser()
        today = datetime.now(timezone.utc).strftime("%Y-%m-%d")
        return cls(home / "transcripts" / f"{today}.jsonl")

    def write(self, event_type: str, payload: dict[str, Any]) -> None:
        with self._lock:
            if self._file is None:
                return
            event = {
                "ts": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
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


class ScopedTranscriptWriter:
    def __init__(self, transcript: TranscriptSink, metadata: dict[str, Any]):
        self.transcript = transcript
        self.metadata = dict(metadata)

    def write(self, event_type: str, payload: dict[str, Any]) -> None:
        self.transcript.write(event_type, {**payload, **self.metadata})

    def close(self) -> None:
        return
