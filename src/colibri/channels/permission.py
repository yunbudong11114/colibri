from __future__ import annotations

from typing import Protocol

from colibri.tools.permissions import PermissionRequest, format_permission_prompt_lines, parse_permission_choice


class TextReplyChannel(Protocol):
    """Channels that can prompt a user and wait for a plain-text reply."""

    def prompt_for_text(self, recipient_id: str, prompt: str, timeout_seconds: int) -> str | None:
        ...


class ChannelTextPermissionPrompter:
    """Transport-agnostic permission UX: send prompt on channel, parse numeric reply.

    Weixin (and any future chat channel with text round-trip) should use this
    instead of embedding permission choice logic in the channel module.
    """

    def __init__(
        self,
        channel: TextReplyChannel,
        recipient_id: str,
        timeout_seconds: int = 300,
    ):
        self.channel = channel
        self.recipient_id = recipient_id
        self.timeout_seconds = timeout_seconds

    def confirm(self, request: PermissionRequest) -> str:
        reply = self.channel.prompt_for_text(
            self.recipient_id,
            format_channel_permission_prompt(request),
            self.timeout_seconds,
        )
        if reply is None:
            return "0"
        return parse_permission_choice(reply)


def format_channel_permission_prompt(request: PermissionRequest) -> str:
    lines = [f"Colibri wants to run {request.tool_name}."]
    for line in format_permission_prompt_lines(request):
        if request.subject.kind == "file_path" and line.startswith("file: "):
            lines.append("path: " + line.removeprefix("file: ").split(" ", 1)[-1])
        else:
            lines.append(line)
    lines.extend(["", "choose:"])
    if request.subject.kind == "shell":
        lines.extend(
            [
                "1. once",
                "2. session-command",
                "3. session-executable",
                "4. user-command",
                "5. user-executable",
                "0. deny",
            ]
        )
    elif request.subject.kind == "file_path":
        lines.extend(["1. once", "2. session-dir", "4. user-dir", "0. deny"])
    elif request.subject.kind == "hardware_device":
        lines.extend(["1. once", "2. session-device", "4. user-device", "0. deny"])
    else:
        lines.extend(["1. once", "2. session", "4. user", "0. deny"])
    return "\n".join(lines)
