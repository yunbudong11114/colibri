from pathlib import Path

from colibri.config import AgentConfig
from colibri.messages import ToolCall
from colibri.tools.base import ToolContext
from colibri.tools.registry import ToolRegistry


def make_config(tmp_path: Path, **overrides) -> AgentConfig:
    data = {
        "files": {"roots": [str(tmp_path)]},
        "tools": {"max_result_chars": 40, "max_shell_seconds": 1},
        "shell": {"allow": ["python"], "deny": ["rm"]},
    }
    data.update(overrides)
    return AgentConfig.default().with_overrides(data)


def test_registry_exposes_enabled_builtin_tool_specs(tmp_path):
    config = make_config(tmp_path)
    registry = ToolRegistry.from_config(config, cwd=tmp_path)

    names = {spec["function"]["name"] for spec in registry.specs()}

    assert {"files.list", "files.read", "shell.run"}.issubset(names)


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


def test_shell_run_executes_allowlisted_command(tmp_path):
    config = make_config(tmp_path)
    registry = ToolRegistry.from_config(config, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

    result = registry.run(
        ToolCall(id="1", name="shell.run", arguments={"command": "python -c \"print('hi')\""}),
        context,
    )

    assert result.ok
    assert result.text.strip() == "hi"


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
