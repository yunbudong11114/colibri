from __future__ import annotations

import sys
from typing import TextIO


class ConsoleStatusWriter:
    def __init__(self, enabled: bool = True, stream: TextIO | None = None):
        self.enabled = enabled
        self.stream = stream or sys.stderr

    def write(self, event: str, *parts: object, **fields: object) -> None:
        if not self.enabled:
            return
        middle = " ".join(_status_value(part) for part in parts if part is not None)
        suffix = " ".join(f"{key}={_status_value(value)}" for key, value in fields.items() if value is not None)
        line = f"[colibri] {event}"
        if middle:
            line += f" {middle}"
        if suffix:
            line += f" {suffix}"
        print(line, file=self.stream)


class StatusTranscript:
    def __init__(self, transcript, status: ConsoleStatusWriter):
        self.transcript = transcript
        self.status = status

    def write(self, event_type, payload):
        self._write_status(event_type, payload)
        if self.transcript is not None:
            self.transcript.write(event_type, payload)

    def close(self):
        if self.transcript is not None:
            self.transcript.close()

    def _write_status(self, event_type: str, payload: dict) -> None:
        if event_type == "memory_recall":
            self.status.write("memory", topics=",".join(payload.get("topics", [])))
        elif event_type == "skill_recall":
            self.status.write("skill", skills=",".join(payload.get("skills", [])))
        elif event_type == "tool_call":
            self.status.write("tool", payload.get("name"), "wait_permission")
        elif event_type == "tool_result":
            state = "ok" if payload.get("ok") else payload.get("error_type") or "error"
            self.status.write("tool", payload.get("name"), state, chars=len(str(payload.get("text", ""))))
        elif event_type == "context_compact":
            self.status.write(
                "compact",
                mode=payload.get("mode"),
                dropped=payload.get("dropped_messages"),
                summary_chars=payload.get("summary_chars"),
            )
        elif event_type == "model_error":
            self.status.write("model_error", type=payload.get("error_type"))


def _status_value(value: object) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    return str(value).replace("\n", " ")
