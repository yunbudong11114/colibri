from dataclasses import dataclass
from pathlib import Path
from typing import Any

from colibri.config import AgentConfig
from colibri.permissions_store import ProjectGrants, ProjectPermissionStore
from colibri.tools.base import ToolContext, ToolResult, ToolSpec
from colibri.tools.builtin import FilesListTool, FilesWriteTool, ImageUnderstandTool, ShellRunTool
from colibri.tools.permissions import (
    PermissionPolicy,
    PermissionRequest,
    PermissionSubject,
    format_permission_prompt_lines,
)


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
    prompter = FakePrompter(replies=["n", "n", "n", "n"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)
    context = tool_context(config, tmp_path)

    allowed = policy.decide(ShellRunTool(), {"command": "git status"}, context)
    denied = policy.decide(ShellRunTool(), {"command": "git push"}, context)

    assert allowed.allowed
    assert allowed.scope == "project"
    assert not denied.allowed
    assert prompter.requests[0].subject.shell_command == "git push"


def test_shell_project_prefix_grant_allows_token_boundary_matches(tmp_path):
    config = AgentConfig.default()
    store = ProjectPermissionStore.for_cwd(tmp_path)
    store.save(ProjectGrants(shell_prefixes={"git status", "cargo test"}))
    prompter = FakePrompter(replies=["n", "n", "n", "n"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)
    context = tool_context(config, tmp_path)

    exact = policy.decide(ShellRunTool(), {"command": "git status"}, context)
    longer = policy.decide(ShellRunTool(), {"command": "git status --short"}, context)
    other = policy.decide(ShellRunTool(), {"command": "cargo test --manifest-path colibri-rust/Cargo.toml"}, context)
    compound = policy.decide(ShellRunTool(), {"command": "git status --short && cargo test"}, context)
    background = policy.decide(ShellRunTool(), {"command": "git status --short & cargo test"}, context)
    denied = policy.decide(ShellRunTool(), {"command": "git statusx"}, context)
    denied_compound = policy.decide(ShellRunTool(), {"command": "git status --short && python -V"}, context)
    denied_background = policy.decide(ShellRunTool(), {"command": "git status --short & python -V"}, context)
    denied_substitution = policy.decide(ShellRunTool(), {"command": "git status $(echo ok)"}, context)

    assert exact.allowed
    assert exact.scope == "project_prefix"
    assert longer.allowed
    assert longer.scope == "project_prefix"
    assert other.allowed
    assert other.scope == "project_prefix"
    assert compound.allowed
    assert compound.scope == "project_prefix"
    assert background.allowed
    assert background.scope == "project_prefix"
    assert not denied.allowed
    assert prompter.requests[0].subject.shell_command == "git statusx"
    assert not denied_compound.allowed
    assert prompter.requests[1].subject.shell_command == "git status --short && python -V"
    assert not denied_background.allowed
    assert prompter.requests[2].subject.shell_command == "git status --short & python -V"
    assert not denied_substitution.allowed
    assert prompter.requests[3].subject.shell_command == "git status $(echo ok)"


def test_shell_hard_deny_blocks_without_prompt(tmp_path):
    config = AgentConfig.default()
    prompter = FakePrompter(replies=["y"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)

    result = policy.decide(ShellRunTool(), {"command": "sudo whoami"}, tool_context(config, tmp_path))

    assert not result.allowed
    assert result.reason == "hard_deny"
    assert prompter.requests == []


def test_shell_hard_deny_wins_over_redirection_file_path_prompt(tmp_path):
    config = AgentConfig.default()
    prompter = FakePrompter(replies=["y"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)

    result = policy.decide(
        ShellRunTool(),
        {"command": "sudo tee /tmp/colibri/out.txt"},
        tool_context(config, tmp_path),
    )

    assert not result.allowed
    assert result.reason == "hard_deny"
    assert prompter.requests == []


def test_out_of_root_file_path_prompts_instead_of_default_allow(tmp_path):
    allowed_root = tmp_path / "allowed"
    outside = tmp_path / "outside"
    allowed_root.mkdir()
    outside.mkdir()
    config = AgentConfig.default().with_overrides({"files": {"roots": [str(allowed_root)]}})
    prompter = FakePrompter(replies=["y"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=allowed_root)

    result = policy.decide(FilesListTool(), {"path": str(outside)}, tool_context(config, allowed_root))

    assert result.allowed
    assert result.scope == "once"
    assert result.subject_kind == "file_path"
    assert result.file_path == str(outside.resolve())
    assert prompter.requests[0].subject.kind == "file_path"
    assert prompter.requests[0].subject.file_path == str(outside.resolve())


def test_out_of_root_image_path_prompts_instead_of_default_allow(tmp_path):
    allowed_root = tmp_path / "allowed"
    outside = tmp_path / "outside"
    allowed_root.mkdir()
    outside.mkdir()
    config = AgentConfig.default().with_overrides({"files": {"roots": [str(allowed_root)]}})
    prompter = FakePrompter(replies=["y"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=allowed_root)

    result = policy.decide(
        ImageUnderstandTool(),
        {"path": str(outside / "photo.png")},
        tool_context(config, allowed_root),
    )

    assert result.allowed
    assert result.scope == "once"
    assert result.subject_kind == "file_path"
    assert prompter.requests[0].tool_name == "image.understand"


def test_out_of_root_files_write_prompts_as_file_path(tmp_path):
    allowed_root = tmp_path / "allowed"
    outside = tmp_path / "outside"
    allowed_root.mkdir()
    outside.mkdir()
    target = outside / "artifact.html"
    config = AgentConfig.default().with_overrides({"files": {"roots": [str(allowed_root)]}})
    prompter = FakePrompter(replies=["y"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=allowed_root)

    result = policy.decide(
        FilesWriteTool(),
        {"path": str(target), "content": "<html></html>"},
        tool_context(config, allowed_root),
    )

    assert result.allowed
    assert result.subject_kind == "file_path"
    assert result.file_path == str(target.resolve())
    assert prompter.requests[0].subject.kind == "file_path"


def test_in_root_files_write_prompts_with_absolute_path_and_content_summary(tmp_path):
    config = AgentConfig.default()
    prompter = FakePrompter(replies=["y"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)
    context = tool_context(config, tmp_path)
    content = 'print("Hello, World!")\n'

    result = policy.decide(FilesWriteTool(), {"path": "hello_world.py", "content": content}, context)

    assert result.allowed
    request = prompter.requests[0]
    assert request.subject.kind == "file_path"
    assert request.subject.file_path == str((tmp_path / "hello_world.py").resolve())
    lines = format_permission_prompt_lines(request)
    assert lines[0] == f"file: files.write {(tmp_path / 'hello_world.py').resolve()}"
    assert any("content:" in line and "chars" in line and "bytes" in line for line in lines)
    assert content not in "\n".join(lines)


def test_memory_write_prompt_summarizes_content_without_absolute_path(tmp_path):
    request = PermissionRequest(
        tool_name="memory.write",
        arguments={"file": "USER.md", "mode": "replace", "content": "喜欢中文回答" * 20},
        read_only=False,
        subject=PermissionSubject(kind="tool", tool_name="memory.write", read_only=False),
    )

    text = "\n".join(format_permission_prompt_lines(request))

    assert "tool: memory.write" in text
    assert "file: USER.md" in text
    assert "mode: replace" in text
    assert "content:" in text
    assert "chars" in text
    assert "bytes" in text
    assert "喜欢中文回答" * 20 not in text
    assert "/USER.md" not in text


def test_shell_redirection_to_out_of_root_path_prompts_as_file_path(tmp_path):
    allowed_root = tmp_path / "allowed"
    outside = tmp_path / "outside"
    allowed_root.mkdir()
    outside.mkdir()
    target = outside / "baidu.html"
    config = AgentConfig.default().with_overrides({"files": {"roots": [str(allowed_root)]}})
    prompter = FakePrompter(replies=["y"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=allowed_root)

    result = policy.decide(
        ShellRunTool(),
        {"command": f"cat << 'EOF' > {target}\n<html></html>\nEOF"},
        tool_context(config, allowed_root),
    )

    assert result.allowed
    assert result.subject_kind == "file_path"
    assert result.file_path == str(target.resolve())
    assert prompter.requests[0].subject.kind == "file_path"
    assert prompter.requests[0].subject.shell_command is not None


def test_files_under_startup_cwd_are_allowed_without_prompt(tmp_path):
    config = AgentConfig.default().with_overrides({"files": {"roots": []}})
    prompter = FakePrompter(replies=["n"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)
    context = tool_context(config, tmp_path)

    result = policy.decide(FilesListTool(), {"path": "."}, context)

    assert result.allowed
    assert result.scope == "default_read_only"
    assert prompter.requests == []


def test_file_path_session_grant_allows_same_resolved_path_without_prompt(tmp_path):
    allowed_root = tmp_path / "allowed"
    outside = tmp_path / "outside"
    allowed_root.mkdir()
    outside.mkdir()
    config = AgentConfig.default().with_overrides({"files": {"roots": [str(allowed_root)]}})
    prompter = FakePrompter(replies=["s"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=allowed_root)
    context = tool_context(config, allowed_root)

    first = policy.decide(FilesListTool(), {"path": str(outside)}, context)
    second = policy.decide(FilesListTool(), {"path": str(outside)}, context)

    assert first.allowed
    assert second.allowed
    assert second.scope == "session_file_root"
    assert len(prompter.requests) == 1


def test_file_path_session_grant_allows_children_under_same_directory(tmp_path):
    allowed_root = tmp_path / "allowed"
    outside = tmp_path / "outside"
    allowed_root.mkdir()
    outside.mkdir()
    (outside / "one.txt").write_text("one", encoding="utf-8")
    (outside / "two.txt").write_text("two", encoding="utf-8")
    config = AgentConfig.default().with_overrides({"files": {"roots": [str(allowed_root)]}})
    prompter = FakePrompter(replies=["s"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=allowed_root)
    context = ToolContext(config=config, cwd=allowed_root)

    first = policy.decide(FilesListTool(), {"path": str(outside)}, context)
    second = policy.decide(FilesListTool(), {"path": str(outside / "two.txt")}, context)

    assert first.allowed
    assert first.scope == "session_file_root"
    assert second.allowed
    assert second.scope == "session_file_root"
    assert len(prompter.requests) == 1


def test_file_path_project_root_grant_allows_children_without_prompt(tmp_path):
    allowed_root = tmp_path / "allowed"
    outside = tmp_path / "outside"
    allowed_root.mkdir()
    outside.mkdir()
    (outside / "note.txt").write_text("hello", encoding="utf-8")
    config = AgentConfig.default().with_overrides({"files": {"roots": [str(allowed_root)]}})
    store = ProjectPermissionStore.for_cwd(allowed_root)
    store.save(ProjectGrants(file_roots={str(outside.resolve())}))
    prompter = FakePrompter(replies=["n"], requests=[])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=allowed_root)
    context = ToolContext(config=config, cwd=allowed_root)

    result = policy.decide(FilesListTool(), {"path": str(outside / "note.txt")}, context)

    assert result.allowed
    assert result.scope == "project_file_root"
    assert prompter.requests == []
