from dataclasses import dataclass
from pathlib import Path
from typing import Any

from colibri.config import AgentConfig
from colibri.permissions_store import ProjectGrants, ProjectPermissionStore
from colibri.tools.base import ToolContext, ToolResult, ToolSpec
from colibri.tools.builtin import ShellRunTool
from colibri.tools.permissions import PermissionPolicy, PermissionRequest


@dataclass
class FakePrompter:
    replies: list[str]
    requests: list[PermissionRequest]

    def confirm(self, request: PermissionRequest) -> str:
        self.requests.append(request)
        return self.replies.pop(0)


class FakeTool:
    def __init__(self, name: str = "fake.tool", read_only: bool = True):
        self.spec = ToolSpec(
            name=name,
            description="Fake tool",
            input_schema={"type": "object", "properties": {}},
            read_only=read_only,
        )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        return ToolResult(ok=True, text="ran")


def tool_context(config: AgentConfig, tmp_path) -> ToolContext:
    return ToolContext(config=config, cwd=tmp_path or Path.cwd())


def test_read_only_tool_is_allowed_under_default_policy():
    config = AgentConfig.default()
    policy = PermissionPolicy.from_config(config)

    result = policy.decide(FakeTool(read_only=True), {"path": "note.txt"}, tool_context(config, None))

    assert result.allowed
    assert result.decision == "allow"
    assert result.scope == "default_read_only"


def test_confirm_policy_calls_prompter(tmp_path):
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "confirm"}})
    prompter = FakePrompter(replies=["yes"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter)

    result = policy.decide(FakeTool(), {"path": "note.txt"}, tool_context(config, tmp_path))

    assert result.allowed
    assert result.decision == "allow"
    assert result.scope == "once"
    assert prompter.requests[0].tool_name == "fake.tool"
    assert prompter.requests[0].arguments == {"path": "note.txt"}
    assert prompter.requests[0].read_only is True
    assert prompter.requests[0].subject.kind == "tool"


def test_always_choice_allows_tool_for_current_session(tmp_path):
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "confirm"}})
    prompter = FakePrompter(replies=["always"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)
    context = tool_context(config, tmp_path)
    tool = FakeTool()

    first = policy.decide(tool, {}, context)
    second = policy.decide(tool, {}, context)

    assert first.allowed
    assert first.scope == "session"
    assert second.allowed
    assert second.scope == "session"
    assert len(prompter.requests) == 1


def test_deny_policy_blocks_tool_without_prompting(tmp_path):
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "deny"}})
    prompter = FakePrompter(replies=["yes"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter)

    result = policy.decide(FakeTool(), {}, tool_context(config, tmp_path))

    assert not result.allowed
    assert result.decision == "deny"
    assert result.scope == "default"
    assert prompter.requests == []


def test_allow_read_confirm_write_confirms_non_read_only_tool(tmp_path):
    config = AgentConfig.default()
    prompter = FakePrompter(replies=["no"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter)

    result = policy.decide(FakeTool(read_only=False), {"command": "write"}, tool_context(config, tmp_path))

    assert not result.allowed
    assert result.decision == "deny"
    assert result.reason == "user_denied"
    assert prompter.requests[0].tool_name == "fake.tool"
    assert prompter.requests[0].arguments == {"command": "write"}
    assert prompter.requests[0].read_only is False


def test_shell_command_prompts_when_no_grant(tmp_path):
    config = AgentConfig.default()
    prompter = FakePrompter(replies=["y"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)

    result = policy.decide(ShellRunTool(), {"command": "pwd"}, tool_context(config, tmp_path))

    assert result.allowed
    assert result.decision == "allow"
    assert result.scope == "once"
    assert prompter.requests[0].subject.shell_command == "pwd"


def test_shell_session_command_grant_allows_second_call_without_prompt(tmp_path):
    config = AgentConfig.default()
    prompter = FakePrompter(replies=["s"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)
    context = tool_context(config, tmp_path)

    first = policy.decide(ShellRunTool(), {"command": "pwd"}, context)
    second = policy.decide(ShellRunTool(), {"command": "pwd"}, context)

    assert first.allowed
    assert second.allowed
    assert second.scope == "session"
    assert len(prompter.requests) == 1


def test_shell_session_executable_grant_allows_same_executable(tmp_path):
    config = AgentConfig.default()
    prompter = FakePrompter(replies=["e"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)
    context = tool_context(config, tmp_path)

    first = policy.decide(ShellRunTool(), {"command": "git status"}, context)
    second = policy.decide(ShellRunTool(), {"command": "git log"}, context)

    assert first.allowed
    assert second.allowed
    assert second.scope == "session_executable"
    assert len(prompter.requests) == 1


def test_shell_project_command_grant_is_exact(tmp_path):
    config = AgentConfig.default()
    store = ProjectPermissionStore.for_cwd(tmp_path)
    store.save(ProjectGrants(shell_commands={"git status"}))
    prompter = FakePrompter(replies=["n"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)
    context = tool_context(config, tmp_path)

    allowed = policy.decide(ShellRunTool(), {"command": "git status"}, context)
    denied = policy.decide(ShellRunTool(), {"command": "git push"}, context)

    assert allowed.allowed
    assert allowed.scope == "project"
    assert not denied.allowed
    assert prompter.requests[0].subject.shell_command == "git push"


def test_shell_hard_deny_blocks_without_prompt(tmp_path):
    config = AgentConfig.default()
    prompter = FakePrompter(replies=["y"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)

    result = policy.decide(ShellRunTool(), {"command": "sudo whoami"}, tool_context(config, tmp_path))

    assert not result.allowed
    assert result.reason == "hard_deny"
    assert prompter.requests == []
