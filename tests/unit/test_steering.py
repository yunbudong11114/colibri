from colibri.config import AgentConfig
from colibri.messages import ModelResponse, ToolCall
from colibri.session import AgentSession
from colibri.steering import SKIPPED_TOOL_RESULT, format_steering_ack
from colibri.tools.base import ToolContext, ToolResult, ToolSpec
from colibri.tools.permissions import PermissionPolicy
from colibri.tools.registry import ToolRegistry


def test_skip_result_constant():
    assert SKIPPED_TOOL_RESULT == "Skipped due to queued user message."


def test_ack_with_short_preview():
    assert format_steering_ack(2, "别用 rm") == "已改方向，跳过剩余 2 个工具\n改：别用 rm"


def test_ack_truncates_preview_at_20_chars():
    text = "一二三四五六七八九十一二三四五六七八九十多余"
    ack = format_steering_ack(1, text)
    assert ack.startswith("已改方向，跳过剩余 1 个工具\n改：")
    preview = ack.split("\n", 1)[1].removeprefix("改：")
    assert preview.endswith("…")
    assert len(preview.rstrip("…")) == 20


def test_ack_omits_preview_when_empty():
    assert format_steering_ack(0, "  ") == "已改方向，跳过剩余 0 个工具"


def test_steer_rejected_when_turn_inactive(tmp_path):
    session = AgentSession(
        config=AgentConfig.default().with_overrides({"tools": {"default_permission": "allow"}}),
        model=_TwoToolsThenTextModel(),
        tools=ToolRegistry([], cwd=tmp_path),
    )

    assert session.steer("change plan") is False
    assert session.is_turn_active() is False


def test_steer_rejected_while_permission_pending(tmp_path):
    session = AgentSession(
        config=AgentConfig.default().with_overrides({"tools": {"default_permission": "allow"}}),
        model=_TwoToolsThenTextModel(),
        tools=ToolRegistry([], cwd=tmp_path),
    )
    session._turn_active = True
    session._permission_pending = True

    assert session.steer("change plan") is False
    assert session.is_permission_pending() is True


def test_steer_skips_remaining_tools_and_injects_user_message(tmp_path):
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "allow"}})
    session_ref: list[AgentSession | None] = [None]
    tool_a = _SteeringTool(name="steer.a", session_ref=session_ref, steer_text="change plan")
    tool_b = _SteeringTool(name="steer.b", session_ref=session_ref)
    acks: list[str] = []
    session = AgentSession(
        config=config,
        model=_TwoToolsThenTextModel(),
        tools=ToolRegistry([tool_a, tool_b], cwd=tmp_path),
        permission_policy=PermissionPolicy.from_config(config, cwd=tmp_path),
        steer_notifier=acks.append,
    )
    session_ref[0] = session

    response = session.submit("do work")

    assert tool_a.runs == 1
    assert tool_b.runs == 0
    assert response.text == "steered-ok"
    assert any(
        message.role == "tool"
        and message.tool_call_id == "call_b"
        and SKIPPED_TOOL_RESULT in message.content
        for message in session.messages
    )
    assert any(
        message.role == "user" and message.content == "change plan" for message in session.messages
    )
    assert acks == [format_steering_ack(1, "change plan")]
    assert session.is_turn_active() is False


def test_steer_during_text_only_complete_is_applied(tmp_path):
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "allow"}})
    session_ref: list[AgentSession | None] = [None]
    acks: list[str] = []
    session = AgentSession(
        config=config,
        model=_SteerDuringTextOnlyModel(session_ref),
        tools=ToolRegistry([], cwd=tmp_path),
        permission_policy=PermissionPolicy.from_config(config, cwd=tmp_path),
        steer_notifier=acks.append,
    )
    session_ref[0] = session

    response = session.submit("do work")

    assert response.text == "steered-ok"
    assert any(
        message.role == "user" and message.content == "change plan" for message in session.messages
    )
    assert acks == [format_steering_ack(0, "change plan")]
    assert session.is_turn_active() is False


def test_steering_queue_empty_after_normal_submit(tmp_path):
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "allow"}})
    session = AgentSession(
        config=config,
        model=_PlainTextModel(),
        tools=ToolRegistry([], cwd=tmp_path),
        permission_policy=PermissionPolicy.from_config(config, cwd=tmp_path),
    )

    response = session.submit("hello")

    assert response.text == "done"
    assert session.is_turn_active() is False
    assert session._drain_one_steering() is None


class _SteerDuringTextOnlyModel:
    def __init__(self, session_ref: list):
        self.session_ref = session_ref
        self.calls = 0

    def complete(self, messages, tools, system, limits):
        self.calls += 1
        if self.calls == 1:
            session = self.session_ref[0]
            assert session is not None
            assert session.steer("change plan") is True
            return ModelResponse(text="almost done")
        return ModelResponse(text="steered-ok")


class _PlainTextModel:
    def complete(self, messages, tools, system, limits):
        return ModelResponse(text="done")


class _TwoToolsThenTextModel:
    def __init__(self):
        self.calls = 0

    def complete(self, messages, tools, system, limits):
        self.calls += 1
        if self.calls == 1:
            return ModelResponse(
                text="",
                tool_calls=[
                    ToolCall(id="call_a", name="steer.a", arguments={}),
                    ToolCall(id="call_b", name="steer.b", arguments={}),
                ],
            )
        return ModelResponse(text="steered-ok")


class _SteeringTool:
    def __init__(self, name: str, session_ref: list, steer_text: str | None = None):
        self.spec = ToolSpec(
            name=name,
            description="Steering test tool",
            input_schema={"type": "object", "properties": {}},
            read_only=True,
        )
        self.session_ref = session_ref
        self.steer_text = steer_text
        self.runs = 0

    def run(self, arguments, context: ToolContext) -> ToolResult:
        self.runs += 1
        if self.steer_text is not None:
            session = self.session_ref[0]
            assert session is not None
            assert session.steer(self.steer_text) is True
        return ToolResult(ok=True, text=f"{self.spec.name}-ok")
