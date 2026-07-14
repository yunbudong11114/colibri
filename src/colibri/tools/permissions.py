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
)


PermissionDecision = Literal["allow", "deny", "confirm", "always"]


@dataclass(frozen=True)
class PermissionSubject:
    kind: Literal["tool", "shell", "file_path"]
    tool_name: str
    shell_command: str | None = None
    shell_executable: str | None = None
    file_path: str | None = None
    file_root: str | None = None
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
        choice = _permission_choice(self._prompter().confirm(request))
        return self._apply_choice(choice, subject, user_grants)

    def _granted(
        self,
        subject: PermissionSubject,
        user_grants: UserGrants,
    ) -> PermissionDecisionResult | None:
        if subject.kind == "shell":
            if subject.shell_command in self.session_shell_commands:
                return _decision(True, "allow", "session", subject)
            if subject.shell_executable in self.session_shell_executables:
                return _decision(True, "allow", "session_executable", subject)
            if subject.shell_command in user_grants.shell_commands:
                return _decision(True, "allow", "user", subject)
            if _shell_command_matches_user_executables(subject.shell_command, user_grants):
                return _decision(True, "allow", "user_executable", subject)
            return None
        if subject.kind == "file_path":
            if _path_under_any_root(subject.file_path, self.session_file_roots):
                return _decision(True, "allow", "session_file_root", subject)
            if _path_under_any_root(subject.file_path, user_grants.file_roots):
                return _decision(True, "allow", "user_file_root", subject)
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
        user_grants: UserGrants,
    ) -> PermissionDecisionResult:
        if choice == "1":
            return _decision(True, "allow", "once", subject)
        if choice == "2":
            if subject.kind == "shell" and subject.shell_command is not None:
                self.session_shell_commands.add(subject.shell_command)
            elif subject.kind == "file_path" and subject.file_path is not None:
                if subject.file_root is not None:
                    self.session_file_roots.add(subject.file_root)
            else:
                self.session_tool_grants.add(subject.tool_name)
            scope = "session_file_root" if subject.kind == "file_path" else "session"
            return _decision(True, "allow", scope, subject)
        if choice == "3" and subject.kind == "shell" and subject.shell_executable is not None:
            self.session_shell_executables.add(subject.shell_executable)
            return _decision(True, "allow", "session_executable", subject)
        if choice == "5" and subject.kind == "shell" and subject.shell_executable is not None:
            next_grants = UserGrants(
                shell_commands=set(user_grants.shell_commands),
                shell_executables=set(user_grants.shell_executables) | {subject.shell_executable},
                tool_names=set(user_grants.tool_names),
                file_roots=set(user_grants.file_roots),
            )
            self.user_store.save(next_grants)
            return _decision(True, "allow", "user_executable", subject)
        if choice == "4":
            if subject.kind == "shell" and subject.shell_command is not None:
                next_grants = UserGrants(
                    shell_commands=set(user_grants.shell_commands) | {subject.shell_command},
                    shell_executables=set(user_grants.shell_executables),
                    tool_names=set(user_grants.tool_names),
                    file_roots=set(user_grants.file_roots),
                )
            elif subject.kind == "file_path" and subject.file_path is not None:
                next_grants = UserGrants(
                    shell_commands=set(user_grants.shell_commands),
                    shell_executables=set(user_grants.shell_executables),
                    tool_names=set(user_grants.tool_names),
                    file_roots=(
                        set(user_grants.file_roots) | ({subject.file_root} if subject.file_root else set())
                    ),
                )
            else:
                next_grants = UserGrants(
                    shell_commands=set(user_grants.shell_commands),
                    shell_executables=set(user_grants.shell_executables),
                    tool_names=set(user_grants.tool_names) | {subject.tool_name},
                    file_roots=set(user_grants.file_roots),
                )
            self.user_store.save(next_grants)
            scope = "user_file_root" if subject.kind == "file_path" else "user"
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


def _permission_choice(reply: str) -> str:
    first = reply.strip().split(maxsplit=1)[0] if reply.strip() else "0"
    return first if first in {"0", "1", "2", "3", "4", "5"} else "0"


def _shell_write_path(command_text: str, argv: list[str], context: ToolContext | None) -> Path | None:
    if context is None or not command_text:
        return None
    target = _redirection_target(argv)
    if target is None:
        return None
    return resolve_file_path(target, context.cwd)


def _shell_command_matches_user_executables(command: str | None, grants: UserGrants) -> bool:
    if command is None or not grants.shell_executables:
        return False
    if has_dangerous_shell_features(command):
        return False
    segments = shell_command_segments(command)
    if not segments:
        return False
    return all(
        segment in grants.shell_commands
        or any(_command_executable_matches(segment, executable) for executable in grants.shell_executables)
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
            return argv[index + 1]
        for op in sorted(redirect_ops, key=len, reverse=True):
            if token.startswith(op) and len(token) > len(op):
                return token[len(op) :]
    if argv and argv[0] == "tee":
        for token in argv[1:]:
            if token.startswith("-"):
                continue
            return token
    return None


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
