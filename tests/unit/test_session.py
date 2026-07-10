from colibri.config import AgentConfig
from colibri.media import MediaPart
from colibri.messages import Message, ModelResponse, ToolCall
from colibri.model.fake import FakeModelClient
from colibri.session import SYSTEM_PROMPT, AgentSession
from colibri.tools.base import ToolContext, ToolResult, ToolSpec
from colibri.tools.permissions import PermissionPolicy
from colibri.tools.registry import ToolRegistry


def test_submit_records_user_and_assistant_messages():
    session = AgentSession(config=AgentConfig.default(), model=FakeModelClient())

    response = session.submit("hello")

    assert response.text == "fake: hello"
    assert [message.role for message in session.messages] == ["user", "assistant"]
    assert session.messages[0].content == "hello"
    assert session.messages[1].content == "fake: hello"


def test_submit_restores_history_once_before_new_user_message():
    calls = 0

    def load_history():
        nonlocal calls
        calls += 1
        return [Message(role="user", content="previous"), Message(role="assistant", content="previous answer")]

    transcript = MemoryTranscript()
    session = AgentSession(
        config=AgentConfig.default(),
        model=FakeModelClient(),
        transcript=transcript,
        history_loader=load_history,
    )

    session.submit("current")
    session.submit("next")

    assert calls == 1
    assert [message.content for message in session.messages[:4]] == [
        "previous",
        "previous answer",
        "current",
        "fake: current",
    ]
    assert [payload["text"] for event_type, payload in transcript.events if event_type == "user_message"] == [
        "current",
        "next",
    ]


def test_reset_does_not_restore_old_transcript_again():
    calls = 0

    def load_history():
        nonlocal calls
        calls += 1
        return [Message(role="user", content="previous"), Message(role="assistant", content="previous answer")]

    session = AgentSession(config=AgentConfig.default(), model=FakeModelClient(), history_loader=load_history)
    session.submit("current")

    session.reset()
    session.submit("fresh")

    assert calls == 1
    assert [message.content for message in session.messages] == ["fresh", "fake: fresh"]


def test_session_reuses_lazy_runtime_dependencies_across_submits(monkeypatch, tmp_path):
    registry_calls = 0
    analyzer_calls = 0

    def build_registry(cls, config, cwd=None):
        nonlocal registry_calls
        registry_calls += 1
        return ToolRegistry(tools=[], cwd=tmp_path)

    def build_analyzer(config, model):
        nonlocal analyzer_calls
        analyzer_calls += 1
        return object()

    monkeypatch.setattr(ToolRegistry, "from_config", classmethod(build_registry))
    monkeypatch.setattr("colibri.session.ImageAnalyzer", build_analyzer)
    session = AgentSession(config=AgentConfig.default(), model=FakeModelClient())

    session.submit("first")
    session.submit("second")

    assert registry_calls == 1
    assert analyzer_calls == 1


def test_history_restore_error_is_logged_and_does_not_block_submit():
    def fail_restore():
        raise OSError("history unavailable")

    transcript = MemoryTranscript()
    session = AgentSession(
        config=AgentConfig.default(),
        model=FakeModelClient(),
        transcript=transcript,
        history_loader=fail_restore,
    )

    response = session.submit("current")

    assert response.text == "fake: current"
    assert (
        "history_restore_error",
        {"error_type": "OSError", "message": "history unavailable"},
    ) in transcript.events


def test_submit_appends_media_paths_to_user_message(tmp_path):
    session = AgentSession(config=AgentConfig.default(), model=FakeModelClient())
    image_path = tmp_path / "photo.png"

    response = session.submit(
        "这张图里有什么",
        media=[
            MediaPart(
                type="image",
                path=image_path,
                filename="photo.png",
                content_type="image/png",
            )
        ],
    )

    assert "Attachments saved locally:" in session.messages[0].content
    assert f"image: photo.png at {image_path}, content_type=image/png" in session.messages[0].content
    assert response.text == f"fake: 这张图里有什么\n\nAttachments saved locally:\n1. image: photo.png at {image_path}, content_type=image/png"


def test_system_prompt_has_sentence_spacing():
    assert "Colibri. You" in SYSTEM_PROMPT
    assert "low memory, battery, and tool limits." in SYSTEM_PROMPT
    assert "memory.search" not in SYSTEM_PROMPT
    assert "Create or edit files" not in SYSTEM_PROMPT
    assert "files.write" not in SYSTEM_PROMPT


def test_session_keeps_only_recent_messages():
    config = AgentConfig.default().with_overrides({"session": {"trigger_message_limit": 6, "recent_message_limit": 4}})
    session = AgentSession(config=config, model=FakeModelClient())

    session.submit("one")
    session.submit("two")
    session.submit("three")

    assert [message.content for message in session.messages] == ["two", "fake: two", "three", "fake: three"]


def test_session_compacts_message_buffer_into_summary():
    config = AgentConfig.default().with_overrides(
        {
            "session": {
                "trigger_message_limit": 6,
                "recent_message_limit": 4,
                "summary_max_chars": 400,
                "model_compact": False,
            }
        }
    )
    session = AgentSession(config=config, model=FakeModelClient())

    session.submit("one")
    session.submit("two")
    session.submit("three")

    assert [message.content for message in session.messages] == ["two", "fake: two", "three", "fake: three"]
    assert "user: one" in session.summary
    assert "assistant: fake: one" in session.summary
    assert "user: three" in session.summary
    assert "assistant: fake: three" in session.summary


def test_session_does_not_compact_before_trigger_message_limit():
    config = AgentConfig.default().with_overrides(
        {"session": {"trigger_message_limit": 10, "recent_message_limit": 4, "model_compact": False}}
    )
    session = AgentSession(config=config, model=FakeModelClient())

    session.submit("one")
    session.submit("two")
    session.submit("three")

    assert len(session.messages) == 6
    assert session.summary == ""


def test_session_retains_latest_user_message_even_outside_recent_window():
    config = AgentConfig.default().with_overrides(
        {"session": {"trigger_message_limit": 5, "recent_message_limit": 2, "model_compact": False}}
    )
    session = AgentSession(config=config, model=FakeModelClient())
    session.messages = [
        Message(role="user", content="active request"),
        Message(role="assistant", content="", tool_calls=[ToolCall(id="1", name="files.read", arguments={})]),
        Message(role="tool", content="result 1", tool_call_id="1"),
        Message(role="assistant", content="", tool_calls=[ToolCall(id="2", name="files.list", arguments={})]),
        Message(role="tool", content="result 2", tool_call_id="2"),
    ]

    session._compact_messages_if_needed()

    assert [message.content for message in session.messages] == ["active request", "", "result 2"]
    assert "user: active request" in session.summary


def test_session_recent_limit_keeps_complete_tool_call_group():
    config = AgentConfig.default().with_overrides(
        {"session": {"trigger_message_limit": 4, "recent_message_limit": 1, "model_compact": False}}
    )
    session = AgentSession(config=config, model=FakeModelClient())
    session.messages = [
        Message(role="user", content="active request"),
        Message(
            role="assistant",
            content="",
            tool_calls=[ToolCall(id="call_1", name="files.read", arguments={})],
        ),
        Message(role="tool", content="result", tool_call_id="call_1"),
        Message(role="assistant", content="done"),
    ]

    session._compact_messages_if_needed()

    assert [(message.role, message.tool_call_id) for message in session.messages] == [
        ("user", None),
        ("assistant", None),
    ]


def test_session_recent_limit_keeps_tool_group_whole_when_group_exceeds_limit():
    config = AgentConfig.default().with_overrides(
        {"session": {"trigger_message_limit": 3, "recent_message_limit": 1, "model_compact": False}}
    )
    session = AgentSession(config=config, model=FakeModelClient())
    session.messages = [
        Message(role="user", content="active request"),
        Message(
            role="assistant",
            content="",
            tool_calls=[ToolCall(id="call_1", name="files.read", arguments={})],
        ),
        Message(role="tool", content="result", tool_call_id="call_1"),
    ]

    session._compact_messages_if_needed()

    assert [(message.role, message.tool_call_id) for message in session.messages] == [
        ("user", None),
        ("assistant", None),
        ("tool", "call_1"),
    ]


def test_session_uses_model_assisted_compact_without_tools():
    config = AgentConfig.default().with_overrides(
        {
            "model": {"provider": "openai_compatible"},
            "session": {
                "trigger_message_limit": 4,
                "recent_message_limit": 3,
                "summary_max_chars": 1000,
                "model_compact": True,
            },
        }
    )
    transcript = MemoryTranscript()
    model = CompactAwareModel()
    session = AgentSession(config=config, model=model, transcript=transcript)

    session.submit("one")
    session.submit("two")

    assert "Summary:" in session.summary
    assert "Primary Request and Intent" in session.summary
    assert "analysis scratchpad" not in session.summary
    assert model.compact_tools == []
    assert any(
        event_type == "context_compact" and payload["mode"] == "model"
        for event_type, payload in transcript.events
    )


def test_session_falls_back_when_model_assisted_compact_fails():
    config = AgentConfig.default().with_overrides(
        {
            "model": {"provider": "openai_compatible"},
            "session": {
                "trigger_message_limit": 4,
                "recent_message_limit": 3,
                "summary_max_chars": 1000,
                "model_compact": True,
            },
        }
    )
    transcript = MemoryTranscript()
    session = AgentSession(config=config, model=FailingCompactModel(), transcript=transcript)

    session.submit("one")
    session.submit("two")

    assert "user: one" in session.summary
    assert any(event_type == "context_compact_error" for event_type, _payload in transcript.events)
    assert any(
        event_type == "context_compact" and payload["mode"] == "fallback"
        for event_type, payload in transcript.events
    )


def test_session_summary_is_injected_without_persisting_it():
    config = AgentConfig.default().with_overrides(
        {
            "session": {
                "trigger_message_limit": 5,
                "recent_message_limit": 4,
                "summary_max_chars": 400,
                "model_compact": False,
            }
        }
    )
    model = SummaryAwareModel()
    session = AgentSession(config=config, model=model)

    session.submit("one")
    session.submit("two")
    response = session.submit("three")

    assert response.text == "summary used"
    assert model.first_messages[0].role == "system"
    assert "Compacted conversation summary:" in model.first_messages[0].content
    assert "user: one" in model.first_messages[0].content
    assert all("Compacted conversation summary:" not in message.content for message in session.messages)


def test_session_logs_context_compact_event():
    config = AgentConfig.default().with_overrides(
        {"session": {"trigger_message_limit": 6, "recent_message_limit": 4, "model_compact": False}}
    )
    transcript = MemoryTranscript()
    session = AgentSession(config=config, model=FakeModelClient(), transcript=transcript)

    session.submit("one")
    session.submit("two")
    session.submit("three")

    compact_events = [payload for event_type, payload in transcript.events if event_type == "context_compact"]
    assert sum(event["removed_messages"] for event in compact_events) == 2
    assert compact_events[-1]["compacted_messages"] == 6
    assert compact_events[-1]["kept_messages"] == 4
    assert compact_events[-1]["summary_chars"] == len(session.summary)


def test_session_budgets_model_input_and_logs_event():
    config = AgentConfig.default().with_overrides(
        {"session": {"model_input_char_limit": 80, "recent_message_limit": 20}}
    )
    transcript = MemoryTranscript()
    model = BudgetAwareModel()
    session = AgentSession(config=config, model=model, transcript=transcript)

    session.submit("first " + "x" * 30)
    session.submit("second " + "y" * 30)

    assert any(message.content.startswith("second") for message in model.first_messages)
    assert not any(message.content.startswith("first") for message in model.first_messages)
    budget_events = [payload for event_type, payload in transcript.events if event_type == "context_budget"]
    assert budget_events
    assert budget_events[-1]["dropped_model_messages"] > 0


def test_reset_clears_messages_and_summary():
    session = AgentSession(config=AgentConfig.default(), model=FakeModelClient())
    session.submit("hello")

    session.reset()

    assert session.messages == []
    assert session.summary == ""


def test_session_sends_media_result_through_media_sender(tmp_path):
    path = tmp_path / "report.txt"
    path.write_text("hello", encoding="utf-8")
    sent: list[MediaPart] = []
    config = AgentConfig.default().with_overrides(
        {
            "files": {"roots": [str(tmp_path)]},
            "tools": {"default_permission": "allow", "max_result_chars": 1000},
            "session": {"max_tool_rounds": 3},
        }
    )
    session = AgentSession(
        config=config,
        model=ScriptedToolThenFinalModel("files.send", {"path": "report.txt", "caption": "请看"}),
        tools=ToolRegistry.from_config(config, cwd=tmp_path),
        media_sender=sent.append,
    )

    response = session.submit("send report")

    assert response.text == "final: Sent file to channel: report.txt"
    assert sent == [
        MediaPart(
            type="file",
            path=path.resolve(),
            filename="report.txt",
            content_type="text/plain",
            caption="请看",
        )
    ]


def test_session_turns_media_sender_failure_into_tool_error(tmp_path):
    path = tmp_path / "report.txt"
    path.write_text("hello", encoding="utf-8")
    config = AgentConfig.default().with_overrides(
        {
            "files": {"roots": [str(tmp_path)]},
            "tools": {"default_permission": "allow", "max_result_chars": 1000},
            "session": {"max_tool_rounds": 3},
        }
    )

    def fail_send(part: MediaPart) -> None:
        raise RuntimeError("send failed")

    session = AgentSession(
        config=config,
        model=ScriptedToolThenFinalModel("files.send", {"path": "report.txt"}),
        tools=ToolRegistry.from_config(config, cwd=tmp_path),
        media_sender=fail_send,
    )

    response = session.submit("send report")

    assert response.text == "final: media_send_error: send failed"


def test_submit_executes_tool_call_and_returns_final_text(tmp_path):
    note = tmp_path / "note.txt"
    note.write_text("tool result text", encoding="utf-8")
    config = AgentConfig.default().with_overrides(
        {"files": {"roots": [str(tmp_path)]}, "memory": {"enabled": False, "root": str(tmp_path / "memory")}}
    )
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

    assert "Tool round limit reached after 1 round" in response.text
    assert "Recent tool results:" in response.text
    assert "files.list" in response.text


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
        message.role == "tool" and message.content == "permission_denied: User denied counting.tool"
        for message in session.messages
    )


def test_session_returns_user_denial_to_model(tmp_path):
    config = AgentConfig.default()
    prompter = FakePrompter(reply="n")
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)
    session = AgentSession(
        config=config,
        model=ScriptedToolThenFinalModel("shell.run", {"command": "pwd"}),
        tools=ToolRegistry.from_config(config, cwd=tmp_path),
        permission_policy=policy,
    )

    response = session.submit("where am i")

    assert "denied" in response.text.lower()
    assert any(
        message.role == "tool" and "User denied shell.run: pwd" in message.content
        for message in session.messages
    )


def test_session_allows_out_of_root_file_path_after_dynamic_permission(tmp_path):
    allowed_root = tmp_path / "allowed"
    outside = tmp_path / "outside"
    allowed_root.mkdir()
    outside.mkdir()
    (outside / "note.txt").write_text("hello", encoding="utf-8")
    config = AgentConfig.default().with_overrides({"files": {"roots": [str(allowed_root)]}})
    prompter = FakePrompter(reply="y")
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=allowed_root)
    transcript = MemoryTranscript()
    session = AgentSession(
        config=config,
        model=ScriptedToolThenFinalModel("files.list", {"path": str(outside)}),
        tools=ToolRegistry.from_config(config, cwd=allowed_root),
        permission_policy=policy,
        transcript=transcript,
    )

    response = session.submit("list outside")

    assert "note.txt" in response.text
    assert prompter.requests[0].subject.kind == "file_path"
    event = [payload for name, payload in transcript.events if name == "permission_decision"][0]
    assert event["subject_kind"] == "file_path"
    assert event["file_path"] == str(outside.resolve())


def test_session_file_directory_grant_passes_root_to_file_tool(tmp_path):
    allowed_root = tmp_path / "allowed"
    outside = tmp_path / "outside"
    allowed_root.mkdir()
    outside.mkdir()
    (outside / "note.txt").write_text("hello", encoding="utf-8")
    config = AgentConfig.default().with_overrides({"files": {"roots": [str(allowed_root)]}})
    prompter = FakePrompter(reply="s")
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=allowed_root)
    transcript = MemoryTranscript()
    session = AgentSession(
        config=config,
        model=ScriptedToolThenFinalModel("files.read", {"path": str(outside / "note.txt")}),
        tools=ToolRegistry.from_config(config, cwd=allowed_root),
        permission_policy=policy,
        transcript=transcript,
    )

    response = session.submit("read outside")

    assert "hello" in response.text
    event = [payload for name, payload in transcript.events if name == "permission_decision"][0]
    assert event["scope"] == "session_file_root"
    assert event["file_root"] == str(outside.resolve())


def test_session_writes_transcript_events(tmp_path):
    note = tmp_path / "note.txt"
    note.write_text("tool result text", encoding="utf-8")
    config = AgentConfig.default().with_overrides(
        {"files": {"roots": [str(tmp_path)]}, "memory": {"enabled": False, "root": str(tmp_path / "memory")}}
    )
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


def test_session_logs_dynamic_permission_payload(tmp_path):
    config = AgentConfig.default()
    transcript = MemoryTranscript()
    prompter = FakePrompter(reply="y")
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)
    session = AgentSession(
        config=config,
        model=ScriptedToolThenFinalModel("shell.run", {"command": "pwd"}),
        tools=ToolRegistry.from_config(config, cwd=tmp_path),
        permission_policy=policy,
        transcript=transcript,
    )

    session.submit("where am i")

    event = [payload for name, payload in transcript.events if name == "permission_decision"][0]
    assert event["tool_name"] == "shell.run"
    assert event["subject_kind"] == "shell"
    assert event["scope"] == "once"
    assert event["allowed"] is True
    assert event["shell_command"] == "pwd"


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

    assert "Tool round limit reached after 1 round" in response.text
    assert "files.list" in response.text
    assert transcript.events[-1][0] == "round_limit"
    assert transcript.events[-1][1]["text"] == response.text


def test_close_closes_transcript():
    transcript = MemoryTranscript()
    session = AgentSession(config=AgentConfig.default(), model=FakeModelClient(), transcript=transcript)

    session.close()

    assert transcript.closed


def test_memory_write_uses_permission_confirmation(tmp_path):
    config = AgentConfig.default().with_overrides({"memory": {"root": str(tmp_path / "memory")}})
    prompter = FakePrompter(reply="yes")
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)
    session = AgentSession(
        config=config,
        model=ScriptedMemoryWriteModel(),
        tools=ToolRegistry.from_config(config, cwd=tmp_path),
        permission_policy=policy,
    )

    response = session.submit("remember device")

    assert response.text == "final answer"
    assert len(prompter.requests) == 1
    assert prompter.requests[0].tool_name == "memory.write"
    assert prompter.requests[0].arguments == {
        "file": "topics/devices.md",
        "content": "Router upstairs",
        "mode": "append",
    }
    assert prompter.requests[0].read_only is False
    assert (tmp_path / "memory" / "topics" / "devices.md").read_text(encoding="utf-8") == "Router upstairs\n"


def test_skill_run_uses_permission_confirmation(tmp_path):
    skill_dir = tmp_path / "skills" / "release"
    scripts_dir = skill_dir / "scripts"
    scripts_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text("# Release Notes\n", encoding="utf-8")
    (scripts_dir / "render.py").write_text("print('rendered')\n", encoding="utf-8")
    (skill_dir / "skill.toml").write_text(
        """
[[commands]]
name = "render"
command = "python"
args = ["scripts/render.py"]
read_only = false
""".strip(),
        encoding="utf-8",
    )
    config = AgentConfig.default().with_overrides({"skills": {"dirs": [str(tmp_path / "skills")]}})
    prompter = FakePrompter(reply="yes")
    policy = PermissionPolicy.from_config(config, prompter=prompter)
    session = AgentSession(
        config=config,
        model=ScriptedSkillRunModel(),
        tools=ToolRegistry.from_config(config, cwd=tmp_path),
        permission_policy=policy,
    )

    response = session.submit("run release render")

    assert response.text == "final answer"
    assert len(prompter.requests) == 1
    assert prompter.requests[0].tool_name == "skill.run"
    assert prompter.requests[0].arguments == {"skill": "release", "command": "render"}
    assert prompter.requests[0].read_only is False


def test_session_injects_always_on_memory_without_persisting_it(tmp_path):
    memory_root = tmp_path / "memory"
    memory_topics = memory_root / "topics"
    memory_topics.mkdir(parents=True)
    (memory_root / "MEMORY.md").write_text("- Colibri runs on CardputerZero.\n", encoding="utf-8")
    (memory_root / "USER.md").write_text("- User prefers concise Chinese answers.\n", encoding="utf-8")
    (memory_root / "INDEX.md").write_text("- [devices](topics/devices.md): Router notes.\n", encoding="utf-8")
    (memory_topics / "devices.md").write_text("- Router is upstairs.\n", encoding="utf-8")
    config = AgentConfig.default().with_overrides({"memory": {"root": str(memory_root)}})
    model = MemoryAwareModel()
    session = AgentSession(config=config, model=model)

    response = session.submit("where is the router?")

    assert response.text == "memory used"
    assert model.first_messages[0].role == "system"
    assert "Always-on memory:" in model.first_messages[0].content
    assert "[MEMORY.md]" in model.first_messages[0].content
    assert "[USER.md]" in model.first_messages[0].content
    assert "Router is upstairs" not in model.first_messages[0].content
    assert [message.role for message in session.messages] == ["user", "assistant"]
    assert all("Always-on memory:" not in message.content for message in session.messages)


def test_session_logs_memory_context_event(tmp_path):
    memory_root = tmp_path / "memory"
    memory_topics = memory_root / "topics"
    memory_topics.mkdir(parents=True)
    (memory_root / "MEMORY.md").write_text("- Colibri runs on CardputerZero.\n", encoding="utf-8")
    (memory_root / "USER.md").write_text("- User prefers concise Chinese answers.\n", encoding="utf-8")
    config = AgentConfig.default().with_overrides({"memory": {"root": str(memory_root)}})
    transcript = MemoryTranscript()
    session = AgentSession(config=config, model=MemoryAwareModel(), transcript=transcript)

    session.submit("where is the router?")

    assert transcript.events[1] == ("memory_context", {"files": ["MEMORY.md", "USER.md"], "truncated": False})


def test_session_injects_relevant_skill_without_persisting_it(tmp_path):
    skill_dir = tmp_path / "skills" / "release"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text("# Release Notes\n\nUse this when writing release notes.\n", encoding="utf-8")
    config = AgentConfig.default().with_overrides({"skills": {"dirs": [str(tmp_path / "skills")]}})
    transcript = MemoryTranscript()
    model = SkillAwareModel()
    session = AgentSession(config=config, model=model, transcript=transcript)

    response = session.submit("please write release notes")

    assert response.text == "skill used"
    assert any(message.role == "system" and "Relevant skills:" in message.content for message in model.first_messages)
    assert all("Relevant skills:" not in message.content for message in session.messages)
    assert ("skill_recall", {"skills": ["release"], "truncated": False}) in transcript.events


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


class ScriptedToolThenFinalModel:
    def __init__(self, tool_name: str, arguments: dict):
        self.tool_name = tool_name
        self.arguments = arguments
        self.calls = 0

    def complete(self, messages, tools, system, limits):
        self.calls += 1
        if self.calls == 1:
            return ModelResponse(
                text="",
                tool_calls=[ToolCall(id="call_1", name=self.tool_name, arguments=self.arguments)],
            )
        last_tool = [message.content for message in messages if message.role == "tool"][-1]
        return ModelResponse(text=f"final: {last_tool}")


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
                        arguments={"file": "topics/devices.md", "content": "Router upstairs", "mode": "append"},
                    )
                ],
            )
        return ModelResponse(text="final answer")


class ScriptedSkillRunModel:
    def __init__(self):
        self.calls = 0

    def complete(self, messages, tools, system, limits):
        self.calls += 1
        if self.calls == 1:
            assert any(tool["function"]["name"] == "skill.run" for tool in tools)
            return ModelResponse(
                text="",
                tool_calls=[
                    ToolCall(
                        id="call_1",
                        name="skill.run",
                        arguments={"skill": "release", "command": "render"},
                    )
                ],
            )
        return ModelResponse(text="final answer")


class MemoryAwareModel:
    def __init__(self):
        self.first_messages = []

    def complete(self, messages, tools, system, limits):
        self.first_messages = list(messages)
        assert any(message.role == "system" and "Colibri runs on CardputerZero" in message.content for message in messages)
        assert not any(message.role == "system" and "Router is upstairs" in message.content for message in messages)
        return ModelResponse(text="memory used")


class SkillAwareModel:
    def __init__(self):
        self.first_messages = []

    def complete(self, messages, tools, system, limits):
        self.first_messages = list(messages)
        assert any(message.role == "system" and "[release]" in message.content for message in messages)
        return ModelResponse(text="skill used")


class SummaryAwareModel:
    def __init__(self):
        self.calls = 0
        self.first_messages = []

    def complete(self, messages, tools, system, limits):
        self.calls += 1
        if self.calls == 3:
            self.first_messages = list(messages)
            assert any(
                message.role == "system" and "Compacted conversation summary:" in message.content
                for message in messages
            )
            return ModelResponse(text="summary used")
        return ModelResponse(text=f"summary setup {self.calls}")


class BudgetAwareModel:
    def __init__(self):
        self.calls = 0
        self.first_messages = []

    def complete(self, messages, tools, system, limits):
        self.calls += 1
        self.first_messages = list(messages)
        return ModelResponse(text=f"budget {self.calls}")


class CompactAwareModel:
    def __init__(self):
        self.compact_tools = None

    def complete(self, messages, tools, system, limits):
        if system == "You are a helpful AI assistant tasked with summarizing conversations.":
            self.compact_tools = list(tools)
            assert any(message.role == "user" and "Respond with TEXT ONLY" in message.content for message in messages)
            return ModelResponse(
                text=(
                    "<analysis>\nanalysis scratchpad\n</analysis>\n\n"
                    "<summary>\n"
                    "1. Primary Request and Intent:\n"
                    "   User started with one.\n"
                    "9. Optional Next Step:\n"
                    "   Continue with the latest request.\n"
                    "</summary>"
                )
            )
        last_user = next((message.content for message in reversed(messages) if message.role == "user"), "")
        return ModelResponse(text=f"normal: {last_user}")


class FailingCompactModel:
    def complete(self, messages, tools, system, limits):
        if system == "You are a helpful AI assistant tasked with summarizing conversations.":
            raise RuntimeError("compact failed")
        last_user = next((message.content for message in reversed(messages) if message.role == "user"), "")
        return ModelResponse(text=f"normal: {last_user}")


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
