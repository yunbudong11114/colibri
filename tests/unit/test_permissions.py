from dataclasses import dataclass
from typing import Any

from colibri.config import AgentConfig
from colibri.tools.base import ToolContext, ToolResult, ToolSpec
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


def test_read_only_tool_is_allowed_under_default_policy():
    config = AgentConfig.default()
    policy = PermissionPolicy.from_config(config)

    allowed, decision = policy.decide(FakeTool(read_only=True), {"path": "note.txt"})

    assert allowed
    assert decision == "allow"


def test_confirm_policy_calls_prompter():
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "confirm"}})
    prompter = FakePrompter(replies=["yes"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter)

    allowed, decision = policy.decide(FakeTool(), {"path": "note.txt"})

    assert allowed
    assert decision == "allow"
    assert prompter.requests == [PermissionRequest("fake.tool", {"path": "note.txt"}, True)]


def test_always_choice_allows_tool_for_current_session():
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "confirm"}})
    prompter = FakePrompter(replies=["always"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter)
    tool = FakeTool()

    first_allowed, first_decision = policy.decide(tool, {})
    second_allowed, second_decision = policy.decide(tool, {})

    assert first_allowed
    assert first_decision == "always"
    assert second_allowed
    assert second_decision == "allow"
    assert len(prompter.requests) == 1


def test_deny_policy_blocks_tool_without_prompting():
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "deny"}})
    prompter = FakePrompter(replies=["yes"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter)

    allowed, decision = policy.decide(FakeTool(), {})

    assert not allowed
    assert decision == "deny"
    assert prompter.requests == []


def test_allow_read_confirm_write_confirms_non_read_only_tool():
    config = AgentConfig.default()
    prompter = FakePrompter(replies=["no"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter)

    allowed, decision = policy.decide(FakeTool(read_only=False), {"command": "write"})

    assert not allowed
    assert decision == "deny"
    assert prompter.requests == [PermissionRequest("fake.tool", {"command": "write"}, False)]
