from __future__ import annotations

from dataclasses import replace
from datetime import datetime, timedelta

from colibri.messages import Message


_TIME_CONTEXT_SEPARATOR = "\n\n## Current Time\n"
_TIME_USAGE_INSTRUCTION = (
    "Use this only when needed. Do not mention or repeat the current time unless "
    "the user asks or it is relevant."
)


def current_time_context(now: datetime | None = None) -> str:
    current = datetime.now().astimezone() if now is None else now
    if current.tzinfo is None:
        current = current.astimezone()
    return _format_time_context(current.strftime("%Y-%m-%d %H:%M"), current.utcoffset())


def _format_time_context(date_time: str, offset: timedelta | None) -> str:
    return (
        f"## Current Time\n{date_time} ({_format_utc_offset(offset)})\n"
        f"{_TIME_USAGE_INSTRUCTION}"
    )


def append_time_context_to_latest_user(messages: list[Message], time_context: str) -> bool:
    for index in range(len(messages) - 1, -1, -1):
        message = messages[index]
        if message.role != "user":
            continue
        if not message.content:
            content = time_context
        elif time_context.startswith("## Current Time\n"):
            time_line = time_context.removeprefix("## Current Time\n")
            content = f"{message.content}{_TIME_CONTEXT_SEPARATOR}{time_line}"
        else:
            content = f"{message.content}\n\n{time_context}"
        messages[index] = replace(message, content=content)
        return True
    return False


def user_text_without_time_context(content: str) -> str:
    user_text, separator, time_line = content.rpartition(_TIME_CONTEXT_SEPARATOR)
    if separator and _is_formatted_time_line(time_line):
        return user_text
    return content


def _is_formatted_time_line(value: str) -> bool:
    time_line, separator, instruction = value.partition("\n")
    if not separator or instruction != _TIME_USAGE_INSTRUCTION:
        return False
    value = time_line
    if len(value) != 25:
        return False
    punctuation = {
        4: "-",
        7: "-",
        10: " ",
        13: ":",
        16: " ",
        17: "(",
        21: ":",
        24: ")",
    }
    for index, expected in punctuation.items():
        if value[index] != expected:
            return False
    if value[18] not in "+-":
        return False
    return all(
        character in "0123456789"
        for index, character in enumerate(value)
        if index not in punctuation and index != 18
    )


def _format_utc_offset(offset: timedelta | None) -> str:
    if offset is None:
        return "+00:00"
    total_minutes = int(offset.total_seconds() / 60)
    sign = "+" if total_minutes >= 0 else "-"
    absolute_minutes = abs(total_minutes)
    hours, minutes = divmod(absolute_minutes, 60)
    return f"{sign}{hours:02d}:{minutes:02d}"
