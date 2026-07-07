from __future__ import annotations

from colibri.messages import Message


SUMMARY_HEADER = "Compacted conversation summary:"
_TRUNCATED_SUFFIX = "\n...[truncated]"


def summarize_messages(messages: list[Message], max_line_chars: int = 160) -> str:
    tool_names_by_id = _tool_names_by_id(messages)
    lines: list[str] = []
    for message in messages:
        if message.role in {"user", "assistant"}:
            if message.tool_calls:
                names = ", ".join(call.name for call in message.tool_calls)
                lines.append(_bound_line(f"{message.role} tool_calls: {names}", max_line_chars))
            if message.content:
                lines.append(_bound_line(f"{message.role}: {message.content}", max_line_chars))
        elif message.role == "tool":
            tool_name = tool_names_by_id.get(message.tool_call_id or "", "unknown")
            status = _tool_status(message.content)
            lines.append(f"tool {tool_name} {status}: {len(message.content)} chars")
    return "\n".join(lines)


def append_summary(existing: str, addition: str, max_chars: int) -> str:
    combined = "\n".join(part for part in [existing.strip(), addition.strip()] if part)
    if len(combined) <= max_chars:
        return combined
    kept: list[str] = []
    total = 0
    for line in reversed(combined.splitlines()):
        line_len = len(line) + (1 if kept else 0)
        if kept and total + line_len > max_chars:
            break
        if not kept and len(line) > max_chars:
            return line[-max_chars:]
        kept.append(line)
        total += line_len
    return "\n".join(reversed(kept))


def summary_context(summary: str) -> str:
    if not summary:
        return ""
    return f"{SUMMARY_HEADER}\n\n{summary}"


def budget_model_messages(messages: list[Message], max_chars: int) -> tuple[list[Message], int]:
    if _message_chars(messages) <= max_chars:
        return messages, 0

    kept = list(messages)
    dropped = 0
    while len(kept) > 1 and _message_chars(kept) > max_chars:
        drop_index = _oldest_droppable_index(kept)
        if drop_index is None:
            break
        kept.pop(drop_index)
        dropped += 1
    return kept, dropped


def model_input_chars(messages: list[Message]) -> int:
    return _message_chars(messages)


def _tool_names_by_id(messages: list[Message]) -> dict[str, str]:
    names: dict[str, str] = {}
    for message in messages:
        for call in message.tool_calls:
            names[call.id] = call.name
    return names


def _tool_status(content: str) -> str:
    error_prefixes = ("permission_denied:", "unknown_tool:", "tool_error:")
    if content.startswith(error_prefixes):
        return content.split(":", 1)[0]
    return "ok"


def _bound_line(text: str, max_chars: int) -> str:
    normalized = " ".join(text.split())
    if len(normalized) <= max_chars:
        return normalized
    keep = max(0, max_chars - len(" ..."))
    return normalized[:keep] + " ..."


def _message_chars(messages: list[Message]) -> int:
    return sum(len(message.role) + len(message.content) for message in messages)


def _oldest_droppable_index(messages: list[Message]) -> int | None:
    latest_user_index = _latest_user_index(messages)
    for index, message in enumerate(messages):
        if message.role == "system":
            continue
        if index == latest_user_index:
            continue
        return index
    return None


def _latest_user_index(messages: list[Message]) -> int | None:
    for index in range(len(messages) - 1, -1, -1):
        if messages[index].role == "user":
            return index
    return None
