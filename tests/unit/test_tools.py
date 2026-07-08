import json
from pathlib import Path

from colibri.config import AgentConfig
from colibri.messages import ToolCall
from colibri.tools.base import ToolContext
from colibri.tools.builtin import ShellRunTool
from colibri.tools.registry import ToolRegistry


def make_config(tmp_path: Path, **overrides) -> AgentConfig:
    data = {
        "files": {"roots": [str(tmp_path)]},
        "memory": {"root": str(tmp_path / "memory"), "max_search_results": 2},
        "tools": {"max_result_chars": 40, "max_shell_seconds": 1},
        "shell": {"deny": ["rm"]},
    }
    data.update(overrides)
    return AgentConfig.default().with_overrides(data)


def test_registry_exposes_enabled_builtin_tool_specs(tmp_path):
    config = make_config(tmp_path)
    registry = ToolRegistry.from_config(config, cwd=tmp_path)

    names = {spec["function"]["name"] for spec in registry.specs()}

    assert {
        "files.list",
        "files.read",
        "shell.run",
        "memory.list",
        "memory.read",
        "memory.search",
        "memory.write",
        "skill.run",
        "web.search",
    }.issubset(names)


def test_registry_gets_registered_tool_by_name(tmp_path):
    config = make_config(tmp_path)
    registry = ToolRegistry.from_config(config, cwd=tmp_path)

    tool = registry.get("files.read")

    assert tool is not None
    assert tool.spec.name == "files.read"


def test_registry_rejects_unknown_tool(tmp_path):
    config = make_config(tmp_path)
    registry = ToolRegistry.from_config(config, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

    result = registry.run(ToolCall(id="1", name="missing.tool", arguments={}), context)

    assert not result.ok
    assert result.error_type == "unknown_tool"


def test_files_list_lists_allowed_directory(tmp_path):
    (tmp_path / "alpha.txt").write_text("a", encoding="utf-8")
    (tmp_path / "nested").mkdir()
    config = make_config(tmp_path)
    registry = ToolRegistry.from_config(config, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

    result = registry.run(ToolCall(id="1", name="files.list", arguments={"path": str(tmp_path)}), context)

    assert result.ok
    assert result.text.splitlines() == ["alpha.txt", "nested/"]


def test_files_list_rejects_disallowed_directory(tmp_path):
    config = make_config(tmp_path)
    registry = ToolRegistry.from_config(config, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

    result = registry.run(ToolCall(id="1", name="files.list", arguments={"path": "/"}), context)

    assert not result.ok
    assert result.error_type == "permission_denied"


def test_files_read_reads_allowed_file_and_truncates(tmp_path):
    path = tmp_path / "note.txt"
    path.write_text("x" * 100, encoding="utf-8")
    config = make_config(tmp_path)
    registry = ToolRegistry.from_config(config, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

    result = registry.run(ToolCall(id="1", name="files.read", arguments={"path": str(path)}), context)

    assert result.ok
    assert result.truncated
    assert len(result.text) <= config.tools.max_result_chars


def test_shell_run_executes_command_after_permission_phase(tmp_path):
    config = make_config(tmp_path)
    registry = ToolRegistry.from_config(config, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

    result = registry.run(
        ToolCall(id="1", name="shell.run", arguments={"command": "python -c \"print('hi')\""}),
        context,
    )

    assert result.ok
    assert result.text.strip() == "hi"


def test_shell_run_does_not_require_allowlist_after_permission_phase(tmp_path):
    config = AgentConfig.default().with_overrides(
        {"shell": {"deny": ["rm", "sudo"]}, "tools": {"max_shell_seconds": 5}}
    )
    context = ToolContext(config=config, cwd=tmp_path)

    result = ShellRunTool().run({"command": "pwd"}, context)

    assert result.ok
    assert str(tmp_path) in result.text


def test_shell_run_rejects_denied_command(tmp_path):
    config = make_config(tmp_path)
    registry = ToolRegistry.from_config(config, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

    result = registry.run(ToolCall(id="1", name="shell.run", arguments={"command": "rm file"}), context)

    assert not result.ok
    assert result.error_type == "permission_denied"


def test_shell_run_times_out_slow_command(tmp_path):
    config = make_config(tmp_path, tools={"max_result_chars": 100, "max_shell_seconds": 0.01})
    registry = ToolRegistry.from_config(config, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

    result = registry.run(
        ToolCall(id="1", name="shell.run", arguments={"command": "python -c \"import time; time.sleep(1)\""}),
        context,
    )

    assert not result.ok
    assert result.error_type == "timeout"


def test_web_search_builds_baidu_request_and_formats_results(monkeypatch, tmp_path):
    captured = {}

    class FakeResponse:
        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, traceback):
            return False

        def read(self):
            return json.dumps(
                {
                    "references": [
                        {
                            "title": "杭州天气",
                            "url": "https://example.test/weather",
                            "snippet": "drop this bulky field",
                            "summary": "晴",
                        }
                    ]
                }
            ).encode("utf-8")

    def fake_urlopen(request, timeout):
        captured["url"] = request.full_url
        captured["headers"] = {key.lower(): value for key, value in request.header_items()}
        captured["body"] = json.loads(request.data.decode("utf-8"))
        captured["timeout"] = timeout
        return FakeResponse()

    monkeypatch.delenv("DUMATE_SESSION_ID", raising=False)
    monkeypatch.delenv("DUMATE_SCHEDULER_URL", raising=False)
    monkeypatch.setattr("colibri.tools.builtin.web.urllib.request.urlopen", fake_urlopen)
    config = make_config(
        tmp_path,
        web_search={
            "api_key": "search-key",
            "max_results": 7,
            "timeout_seconds": 3,
        },
        tools={"max_result_chars": 1000, "max_shell_seconds": 1},
    )
    registry = ToolRegistry.from_config(config, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

    result = registry.run(
        ToolCall(id="1", name="web.search", arguments={"query": "杭州天气", "count": 2, "freshness": "pd"}),
        context,
    )

    assert result.ok
    assert captured["url"] == "https://qianfan.baidubce.com/v2/ai_search/web_search"
    assert captured["headers"]["authorization"] == "Bearer search-key"
    assert captured["headers"]["x-appbuilder-from"] == "openclaw"
    assert captured["body"]["messages"] == [{"content": "杭州天气", "role": "user"}]
    assert captured["body"]["resource_type_filter"] == [{"type": "web", "top_k": 2}]
    assert "range" in captured["body"]["search_filter"]
    assert captured["timeout"] == 3
    assert "杭州天气" in result.text
    assert "snippet" not in result.text


def test_web_search_requires_configured_baidu_api_key(monkeypatch, tmp_path):
    monkeypatch.delenv("DUMATE_SESSION_ID", raising=False)
    monkeypatch.delenv("DUMATE_SCHEDULER_URL", raising=False)
    config = make_config(tmp_path, web_search={"api_key": ""})
    registry = ToolRegistry.from_config(config, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

    result = registry.run(ToolCall(id="1", name="web.search", arguments={"query": "hello"}), context)

    assert not result.ok
    assert result.error_type == "invalid_config"


def test_memory_list_returns_sorted_topic_names(tmp_path):
    memory_topics = tmp_path / "memory" / "topics"
    memory_topics.mkdir(parents=True)
    (memory_topics / "devices.md").write_text("devices", encoding="utf-8")
    (memory_topics / "preferences.md").write_text("preferences", encoding="utf-8")
    config = make_config(tmp_path)
    registry = ToolRegistry.from_config(config, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

    result = registry.run(ToolCall(id="1", name="memory.list", arguments={}), context)

    assert result.ok
    assert result.text.splitlines() == ["devices", "preferences"]


def test_memory_read_reads_topic_and_rejects_invalid_name(tmp_path):
    memory_topics = tmp_path / "memory" / "topics"
    memory_topics.mkdir(parents=True)
    (memory_topics / "devices.md").write_text("router notes", encoding="utf-8")
    config = make_config(tmp_path)
    registry = ToolRegistry.from_config(config, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

    read_result = registry.run(ToolCall(id="1", name="memory.read", arguments={"topic": "devices"}), context)
    invalid_result = registry.run(ToolCall(id="2", name="memory.read", arguments={"topic": "../secrets"}), context)

    assert read_result.ok
    assert read_result.text == "router notes"
    assert not invalid_result.ok
    assert invalid_result.error_type == "invalid_arguments"


def test_memory_search_finds_index_and_topic_lines_with_limit(tmp_path):
    memory_root = tmp_path / "memory"
    memory_topics = memory_root / "topics"
    memory_topics.mkdir(parents=True)
    (memory_root / "MEMORY.md").write_text("- devices: wifi and router notes\n", encoding="utf-8")
    (memory_topics / "devices.md").write_text("wifi password lives elsewhere\nrouter is upstairs\n", encoding="utf-8")
    (memory_topics / "preferences.md").write_text("wifi tone should be concise\n", encoding="utf-8")
    config = make_config(
        tmp_path,
        memory={"root": str(memory_root), "max_search_results": 2},
        tools={"max_result_chars": 200, "max_shell_seconds": 1},
    )
    registry = ToolRegistry.from_config(config, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

    result = registry.run(ToolCall(id="1", name="memory.search", arguments={"query": "wifi"}), context)

    assert result.ok
    assert result.text.splitlines() == [
        "index: - devices: wifi and router notes",
        "devices: wifi password lives elsewhere",
    ]


def test_memory_write_appends_bullet_and_creates_directories(tmp_path):
    config = make_config(tmp_path)
    registry = ToolRegistry.from_config(config, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

    result = registry.run(
        ToolCall(
            id="1",
            name="memory.write",
            arguments={"topic": "devices", "text": " Router is upstairs "},
        ),
        context,
    )

    assert result.ok
    assert result.text == "Appended memory topic: devices"
    assert (tmp_path / "memory" / "topics" / "devices.md").read_text(encoding="utf-8") == "- Router is upstairs\n"


def test_memory_write_is_not_read_only(tmp_path):
    config = make_config(tmp_path)
    registry = ToolRegistry.from_config(config, cwd=tmp_path)

    tool = registry.get("memory.write")

    assert tool is not None
    assert not tool.spec.read_only


def test_skill_run_is_not_read_only(tmp_path):
    config = make_config(tmp_path)
    registry = ToolRegistry.from_config(config, cwd=tmp_path)

    tool = registry.get("skill.run")

    assert tool is not None
    assert not tool.spec.read_only
