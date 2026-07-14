from __future__ import annotations

import os
import shlex


def denied_shell_executable(command: str, deny: set[str]) -> str | None:
    for executable in shell_executables(command):
        name = os.path.basename(executable)
        if executable in deny or name in deny:
            return executable
    return None


def first_shell_executable(command: str) -> str | None:
    return next(iter(shell_executables(command)), None)


def shell_executables(command: str) -> list[str]:
    executables: list[str] = []
    for segment in shell_command_segments(command):
        try:
            argv = shlex.split(segment)
        except ValueError:
            continue
        executable = _first_executable_token(argv)
        if executable is not None:
            executables.append(executable)
    return executables


def has_dangerous_shell_features(command: str) -> bool:
    if _has_unquoted_nested_shell_syntax(command):
        return True
    for segment in shell_command_segments(command):
        try:
            argv = shlex.split(segment)
        except ValueError:
            continue
        executable = _first_executable_token(argv)
        if executable in {"eval", "source", "."}:
            return True
    return False


def shell_command_segments(command: str) -> list[str]:
    segments: list[str] = []
    buffer: list[str] = []
    quote: str | None = None
    escaped = False
    index = 0
    while index < len(command):
        char = command[index]
        if escaped:
            buffer.append(char)
            escaped = False
            index += 1
            continue
        if char == "\\" and quote != "'":
            buffer.append(char)
            escaped = True
            index += 1
            continue
        if quote is not None:
            buffer.append(char)
            if char == quote:
                quote = None
            index += 1
            continue
        if char in {"'", '"'}:
            quote = char
            buffer.append(char)
            index += 1
            continue
        if char == "\n" or char == ";":
            _push_segment(segments, buffer)
            index += 1
            continue
        if char == "|" and (index + 1 >= len(command) or command[index + 1] != "|"):
            _push_segment(segments, buffer)
            index += 1
            continue
        if char in {"&", "|"} and index + 1 < len(command) and command[index + 1] == char:
            _push_segment(segments, buffer)
            index += 2
            continue
        if char == "&" and not _is_ampersand_redirection(command, index):
            _push_segment(segments, buffer)
            index += 1
            continue
        buffer.append(char)
        index += 1
    _push_segment(segments, buffer)
    return segments


def _has_unquoted_nested_shell_syntax(command: str) -> bool:
    quote: str | None = None
    escaped = False
    index = 0
    while index < len(command):
        char = command[index]
        if escaped:
            escaped = False
            index += 1
            continue
        if char == "\\" and quote != "'":
            escaped = True
            index += 1
            continue
        if quote is not None:
            if char == quote:
                quote = None
            index += 1
            continue
        if char in {"'", '"'}:
            quote = char
            index += 1
            continue
        if command.startswith("$(", index) or char in {"`", "("}:
            return True
        if command.startswith("<<", index):
            return True
        index += 1
    return False


def _is_ampersand_redirection(command: str, index: int) -> bool:
    previous_char = command[index - 1] if index > 0 else ""
    next_char = command[index + 1] if index + 1 < len(command) else ""
    return previous_char == ">" or next_char == ">"


def _push_segment(segments: list[str], buffer: list[str]) -> None:
    segment = "".join(buffer).strip()
    if segment:
        segments.append(segment)
    buffer.clear()


def _first_executable_token(argv: list[str]) -> str | None:
    for token in argv:
        if _is_env_assignment(token):
            continue
        executable = _strip_inline_redirection(token)
        if executable:
            return executable
    return None


def _is_env_assignment(token: str) -> bool:
    if "=" not in token:
        return False
    name, _, _value = token.partition("=")
    return bool(name) and (name[0].isalpha() or name[0] == "_") and all(ch.isalnum() or ch == "_" for ch in name)


def _strip_inline_redirection(token: str) -> str:
    for marker in ("<", ">"):
        if marker in token:
            token = token.split(marker, 1)[0]
    return token
