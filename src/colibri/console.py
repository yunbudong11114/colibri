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
        if event_type == "memory_context":
            self.status.write("memory", files=",".join(payload.get("files", [])))
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
                removed=payload.get("removed_messages"),
                summary_chars=payload.get("summary_chars"),
            )
        elif event_type == "model_error":
            self.status.write("model_error", type=payload.get("error_type"))


def format_answer_for_console(text: str, plain_answer: bool) -> str:
    if plain_answer:
        return f"\n{format_plain_answer(text)}\n"
    return text


def format_plain_answer(text: str) -> str:
    out_lines: list[str] = []
    table_rows: list[list[str]] = []
    for raw in text.splitlines():
        line = raw.rstrip()
        if _is_table_separator(line):
            continue
        if _is_table_row(line):
            table_rows.append(_split_table_row(line))
            continue
        _flush_table_rows(out_lines, table_rows)
        out_lines.append(_strip_inline_markdown(line))
    _flush_table_rows(out_lines, table_rows)
    return "\n".join(out_lines).rstrip("\n")


def _status_value(value: object) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    return str(value).replace("\n", " ")


def _is_table_row(line: str) -> bool:
    trimmed = line.strip()
    return trimmed.startswith("|") and trimmed.count("|") >= 2


def _is_table_separator(line: str) -> bool:
    trimmed = line.strip()
    if not trimmed.startswith("|"):
        return False
    return all(ch in "|-: \t" for ch in trimmed) and "-" in trimmed


def _split_table_row(line: str) -> list[str]:
    cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
    return [_strip_inline_markdown(cell) for cell in cells]


def _flush_table_rows(out_lines: list[str], rows: list[list[str]]) -> None:
    if not rows:
        return
    for row in rows:
        out_lines.append(" / ".join(row))
    rows.clear()


def _strip_inline_markdown(line: str) -> str:
    text = line.lstrip("#").lstrip()
    return text.replace("**", "").replace("__", "").replace("`", "")
