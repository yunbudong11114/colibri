from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
import shlex
from typing import Any, Literal, Protocol

from colibri.config import AgentConfig
from colibri.permissions_store import ProjectGrants, ProjectPermissionStore
from colibri.tools.base import Tool, ToolContext


PermissionDecision = Literal["allow", "deny", "confirm", "always"]


@dataclass(frozen=True)
class PermissionSubject:
    kind: Literal["tool", "shell"]
    tool_name: str
    shell_command: str | None = None
    shell_executable: str | None = None
    read_only: bool = False


@dataclass(frozen=True)
class PermissionDecisionResult:
    allowed: bool
    decision: str
    scope: str
    reason: str = ""


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
        if request.subject.kind == "shell":
            print(f"shell: {request.subject.shell_command}")
            return input("[y] once [s] session [e] executable-session [p] project [n] deny: ").strip().lower()
        print(f"tool: {request.tool_name} {request.arguments}")
        return input("[y] once [s] session [p] project [n] deny: ").strip().lower()


@dataclass
class PermissionPolicy:
    default_permission: str
    project_store: ProjectPermissionStore
    prompter: PermissionPrompter | None = None
    session_tool_grants: set[str] = field(default_factory=set)
    session_shell_commands: set[str] = field(default_factory=set)
    session_shell_executables: set[str] = field(default_factory=set)

    @classmethod
    def from_config(
        cls,
        config: AgentConfig,
        prompter: PermissionPrompter | None = None,
        cwd: Path | None = None,
    ) -> "PermissionPolicy":
        return cls(
            default_permission=config.tools.default_permission,
            project_store=ProjectPermissionStore.for_cwd(cwd or Path.cwd()),
            prompter=prompter,
        )

    def decide(self, tool: Tool, arguments: dict[str, Any], context: ToolContext) -> PermissionDecisionResult:
        subject = permission_subject_for(tool, arguments)
        if subject.kind == "shell" and subject.shell_executable in context.config.shell.deny:
            return PermissionDecisionResult(False, "deny", "none", "hard_deny")

        project_grants = self.project_store.load()
        grant_result = self._granted(subject, project_grants)
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
        choice = self._prompter().confirm(request).strip().lower()
        return self._apply_choice(choice, subject, project_grants)

    def _granted(
        self,
        subject: PermissionSubject,
        project_grants: ProjectGrants,
    ) -> PermissionDecisionResult | None:
        if subject.kind == "shell":
            if subject.shell_command in self.session_shell_commands:
                return PermissionDecisionResult(True, "allow", "session")
            if subject.shell_executable in self.session_shell_executables:
                return PermissionDecisionResult(True, "allow", "session_executable")
            if subject.shell_command in project_grants.shell_commands:
                return PermissionDecisionResult(True, "allow", "project")
            return None
        if subject.tool_name in self.session_tool_grants:
            return PermissionDecisionResult(True, "allow", "session")
        if subject.tool_name in project_grants.tool_names:
            return PermissionDecisionResult(True, "allow", "project")
        return None

    def _default_decision(self, subject: PermissionSubject) -> PermissionDecisionResult | None:
        if self.default_permission == "allow":
            return PermissionDecisionResult(True, "allow", "default")
        if self.default_permission == "deny":
            return PermissionDecisionResult(False, "deny", "default")
        if self.default_permission == "confirm":
            return None
        if self.default_permission == "allow_read_confirm_write" and subject.kind != "shell" and subject.read_only:
            return PermissionDecisionResult(True, "allow", "default_read_only")
        return None

    def _apply_choice(
        self,
        choice: str,
        subject: PermissionSubject,
        project_grants: ProjectGrants,
    ) -> PermissionDecisionResult:
        if choice in {"y", "yes"}:
            return PermissionDecisionResult(True, "allow", "once")
        if choice in {"s", "session", "a", "always"}:
            if subject.kind == "shell" and subject.shell_command is not None:
                self.session_shell_commands.add(subject.shell_command)
            else:
                self.session_tool_grants.add(subject.tool_name)
            return PermissionDecisionResult(True, "allow", "session")
        if choice in {"e", "executable"} and subject.kind == "shell" and subject.shell_executable is not None:
            self.session_shell_executables.add(subject.shell_executable)
            return PermissionDecisionResult(True, "allow", "session_executable")
        if choice in {"p", "project"}:
            if subject.kind == "shell" and subject.shell_command is not None:
                next_grants = ProjectGrants(
                    shell_commands=set(project_grants.shell_commands) | {subject.shell_command},
                    tool_names=set(project_grants.tool_names),
                )
            else:
                next_grants = ProjectGrants(
                    shell_commands=set(project_grants.shell_commands),
                    tool_names=set(project_grants.tool_names) | {subject.tool_name},
                )
            self.project_store.save(next_grants)
            return PermissionDecisionResult(True, "allow", "project")
        return PermissionDecisionResult(False, "deny", "once", "user_denied")

    def _prompter(self) -> PermissionPrompter:
        if self.prompter is None:
            self.prompter = ConsolePermissionPrompter()
        return self.prompter


def permission_subject_for(tool: Tool, arguments: dict[str, Any]) -> PermissionSubject:
    if tool.spec.name == "shell.run":
        command = arguments.get("command")
        command_text = command.strip() if isinstance(command, str) else ""
        executable = None
        try:
            argv = shlex.split(command_text)
            executable = argv[0] if argv else None
        except ValueError:
            executable = None
        return PermissionSubject(
            kind="shell",
            tool_name=tool.spec.name,
            shell_command=command_text,
            shell_executable=executable,
            read_only=False,
        )
    return PermissionSubject(kind="tool", tool_name=tool.spec.name, read_only=tool.spec.read_only)
