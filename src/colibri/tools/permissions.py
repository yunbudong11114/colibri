from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Literal, Protocol

from colibri.config import AgentConfig
from colibri.tools.base import Tool


PermissionDecision = Literal["allow", "deny", "confirm", "always"]


@dataclass(frozen=True)
class PermissionRequest:
    tool_name: str
    arguments: dict[str, Any]
    read_only: bool


class PermissionPrompter(Protocol):
    def confirm(self, request: PermissionRequest) -> str:
        ...


class ConsolePermissionPrompter:
    def confirm(self, request: PermissionRequest) -> str:
        print(f"Tool request: {request.tool_name}")
        print(f"Arguments: {request.arguments}")
        return input("Allow this tool call? [y]es/[n]o/[a]lways: ").strip().lower()


@dataclass
class PermissionPolicy:
    default_permission: str
    prompter: PermissionPrompter | None = None
    always_allow: set[str] = field(default_factory=set)

    @classmethod
    def from_config(
        cls,
        config: AgentConfig,
        prompter: PermissionPrompter | None = None,
    ) -> "PermissionPolicy":
        return cls(default_permission=config.tools.default_permission, prompter=prompter)

    def decide(self, tool: Tool, arguments: dict[str, Any]) -> tuple[bool, PermissionDecision]:
        tool_name = tool.spec.name
        if tool_name in self.always_allow:
            return True, "allow"

        decision = self.check(tool)
        if decision == "allow":
            return True, "allow"
        if decision == "deny":
            return False, "deny"

        request = PermissionRequest(
            tool_name=tool_name,
            arguments=dict(arguments),
            read_only=tool.spec.read_only,
        )
        choice = self._prompter().confirm(request).strip().lower()
        if choice in {"a", "always"}:
            self.always_allow.add(tool_name)
            return True, "always"
        if choice in {"y", "yes"}:
            return True, "allow"
        return False, "deny"

    def check(self, tool: Tool) -> Literal["allow", "deny", "confirm"]:
        if self.default_permission == "allow":
            return "allow"
        if self.default_permission == "deny":
            return "deny"
        if self.default_permission == "confirm":
            return "confirm"
        if self.default_permission == "allow_read_confirm_write":
            return "allow" if tool.spec.read_only else "confirm"
        return "confirm"

    def _prompter(self) -> PermissionPrompter:
        if self.prompter is None:
            self.prompter = ConsolePermissionPrompter()
        return self.prompter
