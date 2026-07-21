from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
import shlex
from typing import Any, Literal, Protocol

from colibri.config import AgentConfig
from colibri.permissions_store import UserGrants, UserPermissionStore
from colibri.tools.base import Tool, ToolContext
from colibri.tools.builtin.files import is_under_workspace_file_root, resolve_file_path
from colibri.tools.shell_policy import (
    denied_shell_executable,
    first_shell_executable,
    has_dangerous_shell_features,
    shell_command_segments,
    shell_executables,
)


PermissionDecision = Literal["allow", "deny", "confirm", "always"]


@dataclass(frozen=True)
class PermissionSubject:
    kind: Literal["tool", "shell", "file_path", "hardware_device"]
    tool_name: str
    shell_command: str | None = None
    shell_executable: str | None = None
    file_path: str | None = None
    file_root: str | None = None
    hardware_device: str | None = None
    read_only: bool = False


@dataclass(frozen=True)
class PermissionDecisionResult:
    allowed: bool
    decision: str
    scope: str
    reason: str = ""
    subject_kind: str = "tool"
    file_path: str | None = None
    file_root: str | None = None
    hardware_device: str | None = None


@dataclass(frozen=True)
class PermissionRequest:
    tool_name: str
    arguments: dict[str, Any]
    read_only: bool
    subject: PermissionSubject


class PermissionPrompter(Protocol):
    def confirm(self, request: PermissionRequest) -> str:
        ...


class ConsolePermissionPrompter:
    def confirm(self, request: PermissionRequest) -> str:
        for line in format_permission_prompt_lines(request):
            print(line)
        if request.subject.kind == "shell":
            return input(
                "[1] once [2] session-command [3] session-executable [4] user-command [5] user-executable [0] deny: "
            ).strip().lower()
        if request.subject.kind == "file_path":
            return input("[1] once [2] session-dir [4] user-dir [0] deny: ").strip().lower()
        if request.subject.kind == "hardware_device":
            return input("[1] once [2] session-device [4] user-device [0] deny: ").strip().lower()
        return input("[1] once [2] session [4] user [0] deny: ").strip().lower()


@dataclass
class PermissionPolicy:
    default_permission: str
    user_store: UserPermissionStore
    prompter: PermissionPrompter | None = None
    session_tool_grants: set[str] = field(default_factory=set)
    session_shell_commands: set[str] = field(default_factory=set)
    session_shell_executables: set[str] = field(default_factory=set)
    session_file_roots: set[str] = field(default_factory=set)
    session_hardware_devices: set[str] = field(default_factory=set)

    @classmethod
    def from_config(
        cls,
        config: AgentConfig,
        prompter: PermissionPrompter | None = None,
        cwd: Path | None = None,
    ) -> "PermissionPolicy":
        return cls(
            default_permission=config.tools.default_permission,
            user_store=UserPermissionStore.for_user(),
            prompter=prompter,
        )

    def decide(self, tool: Tool, arguments: dict[str, Any], context: ToolContext) -> PermissionDecisionResult:
        subject = permission_subject_for(tool, arguments, context)
        hard_denied = False
        if subject.tool_name == "shell.run":
            if subject.shell_command is not None:
                hard_denied = denied_shell_executable(subject.shell_command, set(context.config.shell.deny)) is not None
            else:
                hard_denied = subject.shell_executable in context.config.shell.deny
        elif subject.kind == "hardware_device":
            device = next(
                (
                    candidate
                    for candidate in context.config.hardware.devices
                    if candidate.name == subject.hardware_device
                ),
                None,
            )
            capability = _hardware_write_capability(subject.tool_name)
            hard_denied = (
                device is None
                or not device.allow_write
                or capability is None
                or capability not in device.capabilities
            )
        if hard_denied:
            return _decision(False, "deny", "none", subject, "hard_deny")

        user_grants = self.user_store.load()
        grant_result = self._granted(subject, user_grants)
        if grant_result is not None:
            return grant_result

        default_result = self._default_decision(subject)
        if default_result is not None:
            return default_result

        request = PermissionRequest(
            tool_name=tool.spec.name,
            arguments=dict(arguments),
            read_only=tool.spec.read_only,
            subject=subject,
        )
        choice = parse_permission_choice(self._prompter().confirm(request))
        return self._apply_choice(choice, subject)

    def _granted(
        self,
        subject: PermissionSubject,
        user_grants: UserGrants,
    ) -> PermissionDecisionResult | None:
        if subject.kind == "shell":
            if subject.shell_command in self.session_shell_commands:
                return _decision(True, "allow", "session", subject)
            if _shell_command_matches_executables(
                subject.shell_command,
                self.session_shell_commands,
                self.session_shell_executables,
            ):
                return _decision(True, "allow", "session_executable", subject)
            if subject.shell_command in user_grants.shell_commands:
                return _decision(True, "allow", "user", subject)
            if _shell_command_matches_executables(
                subject.shell_command,
                user_grants.shell_commands,
                user_grants.shell_executables,
            ):
                return _decision(True, "allow", "user_executable", subject)
            return None
        if subject.kind == "file_path":
            if _path_under_any_root(subject.file_path, self.session_file_roots):
                return _decision(True, "allow", "session_file_root", subject)
            if _path_under_any_root(subject.file_path, user_grants.file_roots):
                return _decision(True, "allow", "user_file_root", subject)
            return None
        if subject.kind == "hardware_device":
            if subject.hardware_device in self.session_hardware_devices:
                return _decision(True, "allow", "session_device", subject)
            if subject.hardware_device in user_grants.hardware_devices:
                return _decision(True, "allow", "user_device", subject)
            return None
        if subject.tool_name in self.session_tool_grants:
            return _decision(True, "allow", "session", subject)
        if subject.tool_name in user_grants.tool_names:
            return _decision(True, "allow", "user", subject)
        return None

    def _default_decision(self, subject: PermissionSubject) -> PermissionDecisionResult | None:
        if self.default_permission == "allow":
            return _decision(True, "allow", "default", subject)
        if self.default_permission == "deny":
            return _decision(False, "deny", "default", subject)
        if self.default_permission == "confirm":
            return None
        if self.default_permission == "allow_read_confirm_write" and subject.kind != "shell" and subject.read_only:
            if subject.kind == "file_path":
                return None
            return _decision(True, "allow", "default_read_only", subject)
        return None

    def _apply_choice(
        self,
        choice: str,
        subject: PermissionSubject,
    ) -> PermissionDecisionResult:
        if choice == "1":
            return _decision(True, "allow", "once", subject)
        if choice == "2":
            if subject.kind == "shell" and subject.shell_command is not None:
                self.session_shell_commands.add(subject.shell_command)
            elif subject.kind == "file_path" and subject.file_path is not None:
                if subject.file_root is not None:
                    self.session_file_roots.add(subject.file_root)
            elif subject.kind == "hardware_device" and subject.hardware_device is not None:
                self.session_hardware_devices.add(subject.hardware_device)
            else:
                self.session_tool_grants.add(subject.tool_name)
            if subject.kind == "file_path":
                scope = "session_file_root"
            elif subject.kind == "hardware_device":
                scope = "session_device"
            else:
                scope = "session"
            return _decision(True, "allow", scope, subject)
        if choice == "3" and subject.kind == "shell" and subject.shell_executable is not None:
            self.session_shell_executables.update(_subject_shell_executables(subject))
            return _decision(True, "allow", "session_executable", subject)
        if choice == "5" and subject.kind == "shell" and subject.shell_executable is not None:
            self.user_store.merge(UserGrants(shell_executables=_subject_shell_executables(subject)))
            return _decision(True, "allow", "user_executable", subject)
        if choice == "4":
            if subject.kind == "shell" and subject.shell_command is not None:
                delta = UserGrants(shell_commands={subject.shell_command})
            elif subject.kind == "file_path" and subject.file_path is not None:
                delta = UserGrants(file_roots={subject.file_root} if subject.file_root else set())
            elif subject.kind == "hardware_device" and subject.hardware_device is not None:
                delta = UserGrants(hardware_devices={subject.hardware_device})
            else:
                delta = UserGrants(tool_names={subject.tool_name})
            self.user_store.merge(delta)
            if subject.kind == "file_path":
                scope = "user_file_root"
            elif subject.kind == "hardware_device":
                scope = "user_device"
            else:
                scope = "user"
            return _decision(True, "allow", scope, subject)
        return _decision(False, "deny", "once", subject, "user_denied")

    def _prompter(self) -> PermissionPrompter:
        if self.prompter is None:
            self.prompter = ConsolePermissionPrompter()
        return self.prompter


def permission_subject_for(
    tool: Tool,
    arguments: dict[str, Any],
    context: ToolContext | None = None,
) -> PermissionSubject:
    if tool.spec.name in {"serial.write", "gpio.write", "i2c.write", "spi.transfer"}:
        device = arguments.get("device")
        if isinstance(device, str) and device:
            return PermissionSubject(
                kind="hardware_device",
                tool_name=tool.spec.name,
                hardware_device=device,
                read_only=False,
            )
    if tool.spec.name == "shell.run":
        command = arguments.get("command")
        command_text = command.strip() if isinstance(command, str) else ""
        executable = first_shell_executable(command_text)
        try:
            argv = shlex.split(command_text)
        except ValueError:
            argv = []
        write_path = _shell_write_path(command_text, argv, context)
        if write_path is not None:
            return PermissionSubject(
                kind="file_path",
                tool_name=tool.spec.name,
                shell_command=command_text,
                shell_executable=executable,
                file_path=str(write_path),
                file_root=str(_grant_root_for(write_path)),
                read_only=False,
            )
        return PermissionSubject(
            kind="shell",
            tool_name=tool.spec.name,
            shell_command=command_text,
            shell_executable=executable,
            read_only=False,
        )
    if tool.spec.name in {"files.list", "files.read", "files.write", "files.send", "image.understand"} and context is not None:
        raw_path = arguments.get("path")
        if isinstance(raw_path, str) and raw_path:
            resolved = resolve_file_path(raw_path, context.cwd)
            if resolved is not None and (
                tool.spec.name == "files.write" or not is_under_workspace_file_root(resolved, context)
            ):
                return PermissionSubject(
                    kind="file_path",
                    tool_name=tool.spec.name,
                    file_path=str(resolved),
                    file_root=str(_grant_root_for(resolved)),
                    read_only=tool.spec.read_only,
                )
    return PermissionSubject(kind="tool", tool_name=tool.spec.name, read_only=tool.spec.read_only)


def parse_permission_choice(reply: str) -> str:
    first = reply.strip().split(maxsplit=1)[0] if reply.strip() else "0"
    return first if first in {"0", "1", "2", "3", "4", "5"} else "0"


def _shell_write_path(command_text: str, argv: list[str], context: ToolContext | None) -> Path | None:
    if context is None or not command_text:
        return None
    target = _redirection_target(argv)
    if target is None:
        return None
    return resolve_file_path(target, context.cwd)


def _subject_shell_executables(subject: PermissionSubject) -> set[str]:
    executables = set(shell_executables(subject.shell_command or ""))
    if not executables and subject.shell_executable is not None:
        executables.add(subject.shell_executable)
    return executables


def _shell_command_matches_executables(
    command: str | None,
    commands: set[str],
    executables: set[str],
) -> bool:
    if command is None or not executables:
        return False
    if has_dangerous_shell_features(command):
        return False
    segments = shell_command_segments(command)
    if not segments:
        return False
    return all(
        segment in commands
        or any(_command_executable_matches(segment, executable) for executable in executables)
        for segment in segments
    )


def _command_executable_matches(command: str, executable: str) -> bool:
    command = command.strip()
    executable = executable.strip()
    if not executable:
        return False
    return command == executable or command.startswith(executable + " ")


def format_permission_prompt_lines(request: PermissionRequest) -> list[str]:
    if request.subject.kind == "shell":
        return [f"shell: {request.subject.shell_command or ''}"]

    if request.subject.kind == "file_path":
        lines = [f"file: {request.tool_name} {request.subject.file_path or ''}"]
        if request.subject.shell_command:
            lines.append(f"command: {request.subject.shell_command}")
        if request.tool_name == "files.write":
            lines.append(_content_summary(request.arguments.get("content")))
        return lines

    if request.subject.kind == "hardware_device":
        return [
            f"hardware: {request.tool_name}",
            f"device: {request.subject.hardware_device or ''}",
            f"arguments: {_summarized_arguments(request.arguments)}",
        ]

    if request.tool_name == "memory.write":
        lines = [f"tool: {request.tool_name}"]
        target = request.arguments.get("file") or request.arguments.get("topic")
        if isinstance(target, str) and target:
            lines.append(f"file: {target}")
        mode = request.arguments.get("mode")
        if isinstance(mode, str) and mode:
            lines.append(f"mode: {mode}")
        lines.append(_content_summary(request.arguments.get("content")))
        return lines

    return [f"tool: {request.tool_name} {_summarized_arguments(request.arguments)}"]


def _summarized_arguments(arguments: dict[str, Any]) -> dict[str, Any]:
    summarized: dict[str, Any] = {}
    for key, value in arguments.items():
        if key == "content":
            summarized[key] = _content_summary(value).removeprefix("content: ")
        else:
            summarized[key] = value
    return summarized


def _content_summary(value: Any) -> str:
    if not isinstance(value, str):
        return "content: missing"
    char_count = len(value)
    byte_count = len(value.encode("utf-8"))
    preview = value.replace("\n", "\\n")
    if len(preview) > 40:
        preview = preview[:37] + "..."
    return f"content: {char_count} chars, {byte_count} bytes, preview={preview!r}"


def _redirection_target(argv: list[str]) -> str | None:
    redirect_ops = {">", ">>", "1>", "1>>", "2>", "2>>", "&>", "&>>"}
    for index, token in enumerate(argv):
        if token in redirect_ops and index + 1 < len(argv):
            target = argv[index + 1]
            if not _is_non_file_redirection_target(target):
                return target
        for op in sorted(redirect_ops, key=len, reverse=True):
            if token.startswith(op) and len(token) > len(op):
                target = token[len(op) :]
                if not _is_non_file_redirection_target(target):
                    return target
    if argv and argv[0] == "tee":
        for token in argv[1:]:
            if token.startswith("-"):
                continue
            if not _is_non_file_redirection_target(token):
                return token
    return None


def _is_non_file_redirection_target(target: str) -> bool:
    if target == "/dev/null":
        return True
    if not target.startswith("&"):
        return False
    descriptor = target[1:]
    return descriptor == "-" or descriptor.isdigit()


def _decision(
    allowed: bool,
    decision: str,
    scope: str,
    subject: PermissionSubject,
    reason: str = "",
) -> PermissionDecisionResult:
    return PermissionDecisionResult(
        allowed=allowed,
        decision=decision,
        scope=scope,
        reason=reason,
        subject_kind=subject.kind,
        file_path=subject.file_path,
        file_root=subject.file_root,
        hardware_device=subject.hardware_device,
    )


def _grant_root_for(path: Path) -> Path:
    return path if path.exists() and path.is_dir() else path.parent


def _path_under_any_root(path: str | None, roots: set[str]) -> bool:
    if path is None:
        return False
    resolved = Path(path).expanduser()
    for root in roots:
        try:
            resolved.relative_to(Path(root).expanduser().resolve())
            return True
        except (OSError, ValueError):
            continue
    return False


def _hardware_write_capability(tool_name: str) -> str | None:
    return {
        "serial.write": "serial",
        "gpio.write": "gpio",
        "i2c.write": "i2c",
        "spi.transfer": "spi",
    }.get(tool_name)
