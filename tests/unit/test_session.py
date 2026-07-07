from colibri.config import AgentConfig
from colibri.messages import ModelResponse, ToolCall
from colibri.model.fake import FakeModelClient
from colibri.session import AgentSession
from colibri.tools.base import ToolContext, ToolResult, ToolSpec
from colibri.tools.permissions import PermissionPolicy, PermissionRequest
from colibri.tools.registry import ToolRegistry


def test_submit_records_user_and_assistant_messages():
    session = AgentSession(config=AgentConfig.default(), model=FakeModelClient())

    response = session.submit("hello")

    assert response.text == "fake: hello"
    assert [message.role for message in session.messages] == ["user", "assistant"]
    assert session.messages[0].content == "hello"
    assert session.messages[1].content == "fake: hello"


def test_session_keeps_only_recent_messages():
    config = AgentConfig.default().with_overrides({"session": {"recent_message_limit": 4}})
    session = AgentSession(config=config, model=FakeModelClient())

    session.submit("one")
    session.submit("two")
    session.submit("three")

    assert [message.content for message in session.messages] == ["two", "fake: two", "three", "fake: three"]


def test_reset_clears_messages_and_summary():
    session = AgentSession(config=AgentConfig.default(), model=FakeModelClient())
    session.submit("hello")

    session.reset()

    assert session.messages == []
    assert session.summary == ""


def test_submit_executes_tool_call_and_returns_final_text(tmp_path):
    note = tmp_path / "note.txt"
    note.write_text("tool result text", encoding="utf-8")
    config = AgentConfig.default().with_overrides({"files": {"roots": [str(tmp_path)]}})
    model = ScriptedToolModel(path=str(note))
    session = AgentSession(
        config=config,
        model=model,
        tools=ToolRegistry.from_config(config, cwd=tmp_path),
    )

    response = session.submit("read note")

    assert response.text == "final answer"
    assert any(message.role == "tool" and message.content == "tool result text" for message in session.messages)
    assert model.second_call_had_tool_result


def test_submit_stops_at_max_tool_rounds(tmp_path):
    config = AgentConfig.default().with_overrides(
        {
            "session": {"max_tool_rounds": 1},
            "files": {"roots": [str(tmp_path)]},
        }
    )
    model = AlwaysToolModel()
    session = AgentSession(
        config=config,
        model=model,
        tools=ToolRegistry.from_config(config, cwd=tmp_path),
    )

    response = session.submit("loop")

    assert response.text == "Tool round limit reached"


def test_denied_tool_call_adds_result_without_running_tool(tmp_path):
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "deny"}})
    tool = CountingTool()
    session = AgentSession(
        config=config,
        model=SingleToolCallModel(tool_name="counting.tool"),
        tools=ToolRegistry([tool], cwd=tmp_path),
        permission_policy=PermissionPolicy.from_config(config),
    )

    response = session.submit("try tool")

    assert response.text == "final answer"
    assert tool.calls == 0
    assert any(
        message.role == "tool" and message.content == "permission_denied: Tool call denied"
        for message in session.messages
    )


def test_session_writes_transcript_events(tmp_path):
    note = tmp_path / "note.txt"
    note.write_text("tool result text", encoding="utf-8")
    config = AgentConfig.default().with_overrides({"files": {"roots": [str(tmp_path)]}})
    transcript = MemoryTranscript()
    session = AgentSession(
        config=config,
        model=ScriptedToolModel(path=str(note)),
        tools=ToolRegistry.from_config(config, cwd=tmp_path),
        transcript=transcript,
    )

    response = session.submit("read note")

    assert response.text == "final answer"
    event_types = [event_type for event_type, _payload in transcript.events]
    assert event_types == [
        "user_message",
        "assistant_message",
        "tool_call",
        "permission_decision",
        "tool_result",
        "assistant_message",
    ]


def test_session_writes_round_limit_event(tmp_path):
    config = AgentConfig.default().with_overrides(
        {"session": {"max_tool_rounds": 1}, "files": {"roots": [str(tmp_path)]}}
    )
    transcript = MemoryTranscript()
    session = AgentSession(
        config=config,
        model=AlwaysToolModel(),
        tools=ToolRegistry.from_config(config, cwd=tmp_path),
        transcript=transcript,
    )

    response = session.submit("loop")

    assert response.text == "Tool round limit reached"
    assert transcript.events[-1][0] == "round_limit"


def test_close_closes_transcript():
    transcript = MemoryTranscript()
    session = AgentSession(config=AgentConfig.default(), model=FakeModelClient(), transcript=transcript)

    session.close()

    assert transcript.closed


def test_memory_write_uses_permission_confirmation(tmp_path):
    config = AgentConfig.default().with_overrides({"memory": {"root": str(tmp_path / "memory")}})
    prompter = FakePrompter(reply="yes")
    policy = PermissionPolicy.from_config(config, prompter=prompter)
    session = AgentSession(
        config=config,
        model=ScriptedMemoryWriteModel(),
        tools=ToolRegistry.from_config(config, cwd=tmp_path),
        permission_policy=policy,
    )

    response = session.submit("remember device")

    assert response.text == "final answer"
    assert prompter.requests == [PermissionRequest("memory.write", {"topic": "devices", "text": "Router upstairs"}, False)]
    assert (tmp_path / "memory" / "topics" / "devices.md").read_text(encoding="utf-8") == "- Router upstairs\n"


class ScriptedToolModel:
    def __init__(self, path: str):
        self.path = path
        self.calls = 0
        self.second_call_had_tool_result = False

    def complete(self, messages, tools, system, limits):
        self.calls += 1
        if self.calls == 1:
            assert any(tool["function"]["name"] == "files.read" for tool in tools)
            return ModelResponse(
                text="",
                tool_calls=[ToolCall(id="call_1", name="files.read", arguments={"path": self.path})],
            )

        self.second_call_had_tool_result = any(
            message.role == "tool" and message.tool_call_id == "call_1" for message in messages
        )
        return ModelResponse(text="final answer")


class AlwaysToolModel:
    def complete(self, messages, tools, system, limits):
        return ModelResponse(
            text="",
            tool_calls=[ToolCall(id="call_1", name="files.list", arguments={"path": "."})],
        )


class SingleToolCallModel:
    def __init__(self, tool_name: str):
        self.tool_name = tool_name
        self.calls = 0

    def complete(self, messages, tools, system, limits):
        self.calls += 1
        if self.calls == 1:
            return ModelResponse(
                text="",
                tool_calls=[ToolCall(id="call_1", name=self.tool_name, arguments={})],
            )
        return ModelResponse(text="final answer")


class ScriptedMemoryWriteModel:
    def __init__(self):
        self.calls = 0

    def complete(self, messages, tools, system, limits):
        self.calls += 1
        if self.calls == 1:
            assert any(tool["function"]["name"] == "memory.write" for tool in tools)
            return ModelResponse(
                text="",
                tool_calls=[
                    ToolCall(
                        id="call_1",
                        name="memory.write",
                        arguments={"topic": "devices", "text": "Router upstairs"},
                    )
                ],
            )
        return ModelResponse(text="final answer")


class CountingTool:
    spec = ToolSpec(
        name="counting.tool",
        description="Count calls",
        input_schema={"type": "object", "properties": {}},
        read_only=False,
    )

    def __init__(self):
        self.calls = 0

    def run(self, arguments, context: ToolContext) -> ToolResult:
        self.calls += 1
        return ToolResult(ok=True, text="ran")


class MemoryTranscript:
    def __init__(self):
        self.events = []
        self.closed = False

    def write(self, event_type, payload):
        self.events.append((event_type, payload))

    def close(self):
        self.closed = True


class FakePrompter:
    def __init__(self, reply):
        self.reply = reply
        self.requests = []

    def confirm(self, request):
        self.requests.append(request)
        return self.reply
