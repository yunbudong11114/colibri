# Colibri Dynamic Permissions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace static shell allowlist failures with Claude Code style interactive permission grants for tools and complete shell commands.

**Architecture:** Add a small project permission store, extend `PermissionPolicy` to reason about tool and shell subjects, and route all tool calls through one authorization path before execution. Shell keeps hard-deny safety checks, but normal non-allowlisted commands prompt the user instead of failing inside `ShellRunTool`.

**Tech Stack:** Python 3.12 standard library, `tomllib` for reads, hand-written TOML output for small permission files, pytest, existing Colibri CLI/stdin/stdout abstractions.

## Global Constraints

- Use stdin/stdout only; no GUI, browser, audio device, notification service, or TUI framework.
- Store session grants in memory only.
- Store project grants in `.colibri/permissions.toml`.
- Project-level shell grants are complete-command grants only.
- `files.roots` remains the hard filesystem boundary.
- `shell.deny` remains a hard-deny list for dangerous executables.
- Do not add wildcard, regex, prefix, or executable-wide project shell grants.
- Do not implement OS-level privilege escalation such as `sudo` or sandbox escape.
- Add `.colibri/permissions.toml` to `.gitignore`.

---

## File Structure

- Create `src/colibri/permissions_store.py`: owns `ProjectGrants` and `ProjectPermissionStore`.
- Modify `src/colibri/tools/permissions.py`: adds permission subjects, grant scopes, dynamic prompt choices, and project/session grant checks.
- Modify `src/colibri/tools/builtin/shell.py`: removes normal `shell.allow` hard blocking and exposes shell command subject metadata.
- Modify `src/colibri/tools/base.py`: adds an optional `permission_subject()` method shape for concrete tools.
- Modify `src/colibri/session.py`: asks permission policy with the current `ToolContext`, logs richer permission decisions, and returns denial results without executing tools.
- Modify `src/colibri/cli.py`: passes the registry cwd into `PermissionPolicy.from_config()` and fixes ready status naming if touched.
- Modify `.gitignore`: ignores `.colibri/permissions.toml`.
- Modify `configs/agent.example.toml` and `README.md`: document dynamic permissions and legacy `shell.allow`.
- Add/modify tests in `tests/unit/test_permissions.py`, `tests/unit/test_tools.py`, `tests/unit/test_session.py`, and `tests/unit/test_permissions_store.py`.

---

### Task 1: Project Permission Store

**Files:**
- Create: `src/colibri/permissions_store.py`
- Test: `tests/unit/test_permissions_store.py`

**Interfaces:**
- Produces: `ProjectGrants(shell_commands: set[str], tool_names: set[str])`
- Produces: `ProjectPermissionStore.for_cwd(cwd: Path) -> ProjectPermissionStore`
- Produces: `ProjectPermissionStore.load() -> ProjectGrants`
- Produces: `ProjectPermissionStore.save(grants: ProjectGrants) -> None`

- [ ] **Step 1: Write failing store tests**

Create `tests/unit/test_permissions_store.py`:

```python
from pathlib import Path

from colibri.permissions_store import ProjectGrants, ProjectPermissionStore


def test_project_permission_store_loads_missing_file_as_empty(tmp_path):
    store = ProjectPermissionStore.for_cwd(tmp_path)

    grants = store.load()

    assert grants.shell_commands == set()
    assert grants.tool_names == set()


def test_project_permission_store_saves_and_loads_deduplicated_toml(tmp_path):
    store = ProjectPermissionStore.for_cwd(tmp_path)

    store.save(
        ProjectGrants(
            shell_commands={"pwd", "git status", "pwd"},
            tool_names={"files.list", "files.read", "files.list"},
        )
    )
    grants = store.load()

    assert grants.shell_commands == {"pwd", "git status"}
    assert grants.tool_names == {"files.list", "files.read"}
    text = (tmp_path / ".colibri" / "permissions.toml").read_text(encoding="utf-8")
    assert '[shell]' in text
    assert 'commands = ["git status", "pwd"]' in text
    assert '[tools]' in text
    assert 'names = ["files.list", "files.read"]' in text
```

- [ ] **Step 2: Run store tests to verify failure**

Run: `uv run python -m pytest tests/unit/test_permissions_store.py -q`

Expected: FAIL with `ModuleNotFoundError: No module named 'colibri.permissions_store'`.

- [ ] **Step 3: Implement store**

Create `src/colibri/permissions_store.py`:

```python
from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
import tempfile
import tomllib


@dataclass(frozen=True)
class ProjectGrants:
    shell_commands: set[str] = field(default_factory=set)
    tool_names: set[str] = field(default_factory=set)


class ProjectPermissionStore:
    def __init__(self, path: Path):
        self.path = path

    @classmethod
    def for_cwd(cls, cwd: Path) -> "ProjectPermissionStore":
        return cls(cwd / ".colibri" / "permissions.toml")

    def load(self) -> ProjectGrants:
        if not self.path.exists():
            return ProjectGrants()
        data = tomllib.loads(self.path.read_text(encoding="utf-8"))
        shell = data.get("shell", {})
        tools = data.get("tools", {})
        return ProjectGrants(
            shell_commands={item for item in shell.get("commands", []) if isinstance(item, str)},
            tool_names={item for item in tools.get("names", []) if isinstance(item, str)},
        )

    def save(self, grants: ProjectGrants) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        text = self._format(grants)
        with tempfile.NamedTemporaryFile(
            "w",
            encoding="utf-8",
            dir=self.path.parent,
            prefix="permissions.",
            suffix=".tmp",
            delete=False,
        ) as handle:
            handle.write(text)
            temp_path = Path(handle.name)
        temp_path.replace(self.path)

    def _format(self, grants: ProjectGrants) -> str:
        lines = ["[shell]"]
        lines.append(f"commands = {_toml_string_list(sorted(grants.shell_commands))}")
        lines.append("")
        lines.append("[tools]")
        lines.append(f"names = {_toml_string_list(sorted(grants.tool_names))}")
        lines.append("")
        return "\n".join(lines)


def _toml_string_list(values: list[str]) -> str:
    escaped = [value.replace("\\", "\\\\").replace('"', '\\"') for value in values]
    return "[" + ", ".join(f'"{value}"' for value in escaped) + "]"
```

- [ ] **Step 4: Run store tests to verify pass**

Run: `uv run python -m pytest tests/unit/test_permissions_store.py -q`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/colibri/permissions_store.py tests/unit/test_permissions_store.py
git commit -m "feat: add project permission store"
```

---

### Task 2: Dynamic Permission Policy

**Files:**
- Modify: `src/colibri/tools/permissions.py`
- Test: `tests/unit/test_permissions.py`

**Interfaces:**
- Consumes: `ProjectGrants`, `ProjectPermissionStore`
- Produces: `PermissionSubject`
- Produces: `PermissionDecisionResult(allowed: bool, decision: str, scope: str, reason: str)`
- Produces: `PermissionPolicy.decide(tool: Tool, arguments: dict[str, Any], context: ToolContext) -> PermissionDecisionResult`

- [ ] **Step 1: Write failing permission tests**

Append to `tests/unit/test_permissions.py`:

```python
from pathlib import Path

from colibri.config import AgentConfig
from colibri.permissions_store import ProjectGrants, ProjectPermissionStore
from colibri.tools.base import ToolContext
from colibri.tools.builtin import ShellRunTool
from colibri.tools.permissions import PermissionPolicy


class FakePrompter:
    def __init__(self, *choices):
        self.choices = list(choices)
        self.requests = []

    def confirm(self, request):
        self.requests.append(request)
        return self.choices.pop(0)


def test_shell_command_prompts_when_no_grant(tmp_path):
    prompter = FakePrompter("y")
    config = AgentConfig.default()
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)

    result = policy.decide(ShellRunTool(), {"command": "pwd"}, ToolContext(config=config, cwd=tmp_path))

    assert result.allowed
    assert result.decision == "allow"
    assert result.scope == "once"
    assert prompter.requests[0].subject.shell_command == "pwd"


def test_shell_session_command_grant_allows_second_call_without_prompt(tmp_path):
    prompter = FakePrompter("s")
    config = AgentConfig.default()
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

    first = policy.decide(ShellRunTool(), {"command": "pwd"}, context)
    second = policy.decide(ShellRunTool(), {"command": "pwd"}, context)

    assert first.allowed
    assert second.allowed
    assert second.scope == "session"
    assert len(prompter.requests) == 1


def test_shell_session_executable_grant_allows_same_executable(tmp_path):
    prompter = FakePrompter("e")
    config = AgentConfig.default()
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

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
    prompter = FakePrompter("n")
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)
    context = ToolContext(config=config, cwd=tmp_path)

    allowed = policy.decide(ShellRunTool(), {"command": "git status"}, context)
    denied = policy.decide(ShellRunTool(), {"command": "git push"}, context)

    assert allowed.allowed
    assert allowed.scope == "project"
    assert not denied.allowed
    assert prompter.requests[0].subject.shell_command == "git push"


def test_shell_hard_deny_blocks_without_prompt(tmp_path):
    prompter = FakePrompter("y")
    config = AgentConfig.default()
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)

    result = policy.decide(ShellRunTool(), {"command": "sudo whoami"}, ToolContext(config=config, cwd=tmp_path))

    assert not result.allowed
    assert result.reason == "hard_deny"
    assert prompter.requests == []
```

- [ ] **Step 2: Run permission tests to verify failure**

Run: `uv run python -m pytest tests/unit/test_permissions.py -q`

Expected: FAIL because `PermissionPolicy.from_config()` does not accept `cwd`, and `decide()` does not accept `ToolContext`.

- [ ] **Step 3: Implement dynamic policy**

Modify `src/colibri/tools/permissions.py` with these public shapes:

```python
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

    def _granted(self, subject: PermissionSubject, project_grants: ProjectGrants) -> PermissionDecisionResult | None:
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
```

- [ ] **Step 4: Preserve old tests with compatibility edits**

Update existing `tests/unit/test_permissions.py` call sites so every `policy.decide(...)` passes a `ToolContext`. For example:

```python
context = ToolContext(config=config, cwd=tmp_path)
allowed = policy.decide(tool, {}, context).allowed
```

Where old tests checked tuple decisions, update them to inspect `PermissionDecisionResult.allowed`, `.decision`, and `.scope`.

- [ ] **Step 5: Run permission tests**

Run: `uv run python -m pytest tests/unit/test_permissions.py -q`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/colibri/tools/permissions.py tests/unit/test_permissions.py
git commit -m "feat: add dynamic permission policy"
```

---

### Task 3: Shell Tool Stops Hard-Allowlisting Normal Commands

**Files:**
- Modify: `src/colibri/tools/builtin/shell.py`
- Test: `tests/unit/test_tools.py`

**Interfaces:**
- Consumes: `PermissionPolicy` owns allow/prompt decisions before execution.
- Produces: `ShellRunTool.run()` no longer rejects commands only because they are absent from `config.shell.allow`.

- [ ] **Step 1: Add failing shell test**

Append to `tests/unit/test_tools.py`:

```python
def test_shell_run_does_not_require_allowlist_after_permission_phase(tmp_path):
    config = AgentConfig.default().with_overrides(
        {"shell": {"allow": [], "deny": ["rm", "sudo"]}, "tools": {"max_shell_seconds": 5}}
    )
    context = ToolContext(config=config, cwd=tmp_path)

    result = ShellRunTool().run({"command": "pwd"}, context)

    assert result.ok
    assert str(tmp_path) in result.text
```

- [ ] **Step 2: Run shell test to verify failure**

Run: `uv run python -m pytest tests/unit/test_tools.py::test_shell_run_does_not_require_allowlist_after_permission_phase -q`

Expected: FAIL with `Command is not allowlisted`.

- [ ] **Step 3: Remove normal allowlist rejection**

In `src/colibri/tools/builtin/shell.py`, remove this block:

```python
if executable not in context.config.shell.allow and command not in context.config.shell.allow:
    return ToolResult(ok=False, text="Command is not allowlisted", error_type="permission_denied")
```

Keep the hard deny block:

```python
if executable in context.config.shell.deny:
    return ToolResult(ok=False, text="Command is denied", error_type="permission_denied")
```

- [ ] **Step 4: Run focused shell tests**

Run: `uv run python -m pytest tests/unit/test_tools.py -q`

Expected: PASS after updating any tests that expected allowlist denial.

- [ ] **Step 5: Commit**

```bash
git add src/colibri/tools/builtin/shell.py tests/unit/test_tools.py
git commit -m "feat: let permissions authorize shell commands"
```

---

### Task 4: Session Integration and Denial Feedback

**Files:**
- Modify: `src/colibri/session.py`
- Modify: `src/colibri/cli.py`
- Test: `tests/unit/test_session.py`
- Test: `tests/unit/test_cli.py`

**Interfaces:**
- Consumes: `PermissionPolicy.decide(tool, arguments, context) -> PermissionDecisionResult`
- Produces: denied tool calls append `ToolResult(ok=False, text="User denied ...", error_type="permission_denied")`
- Produces: richer `permission_decision` transcript payload.

- [ ] **Step 1: Add failing session tests**

Append to `tests/unit/test_session.py`:

```python
def test_session_returns_user_denial_to_model(tmp_path):
    config = AgentConfig.default().with_overrides({"files": {"roots": [str(tmp_path)]}})
    prompter = FakePermissionPrompter(["n"])
    policy = PermissionPolicy.from_config(config, prompter=prompter, cwd=tmp_path)
    session = AgentSession(
        config=config,
        model=ScriptedToolThenFinalModel("shell.run", {"command": "pwd"}),
        tools=ToolRegistry.from_config(config, cwd=tmp_path),
        permission_policy=policy,
    )

    response = session.submit("where am i")

    assert "denied" in response.text.lower()
    assert any(message.role == "tool" and "User denied shell.run: pwd" in message.content for message in session.messages)


def test_session_logs_dynamic_permission_payload(tmp_path):
    config = AgentConfig.default()
    transcript = FakeTranscript()
    prompter = FakePermissionPrompter(["y"])
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
```

If helper classes are missing, add:

```python
class FakePermissionPrompter:
    def __init__(self, choices):
        self.choices = list(choices)

    def confirm(self, request):
        return self.choices.pop(0)


class ScriptedToolThenFinalModel:
    def __init__(self, tool_name, arguments):
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
```

- [ ] **Step 2: Run session tests to verify failure**

Run: `uv run python -m pytest tests/unit/test_session.py::test_session_returns_user_denial_to_model tests/unit/test_session.py::test_session_logs_dynamic_permission_payload -q`

Expected: FAIL because `AgentSession` still expects `(allowed, decision)` tuple or logs old payload.

- [ ] **Step 3: Update `AgentSession.submit()` permission handling**

Replace old permission section in `src/colibri/session.py` with this shape:

```python
decision = policy.decide(tool, call.arguments, context)
self._write_transcript(
    "permission_decision",
    {
        "tool_name": call.name,
        "subject_kind": _permission_subject_kind(call),
        "decision": decision.decision,
        "scope": decision.scope,
        "allowed": decision.allowed,
        "reason": decision.reason,
        "shell_command": call.arguments.get("command") if call.name == "shell.run" else None,
    },
)
if decision.allowed:
    result = tool.run(call.arguments, context)
else:
    result = ToolResult(
        ok=False,
        text=_denied_tool_text(call),
        error_type="permission_denied",
    )
```

Use local helper functions:

```python
def _permission_subject_kind(call: ToolCall) -> str:
    return "shell" if call.name == "shell.run" else "tool"


def _denied_tool_text(call: ToolCall) -> str:
    if call.name == "shell.run":
        command = call.arguments.get("command")
        if isinstance(command, str) and command.strip():
            return f"User denied shell.run: {command.strip()}"
    return f"User denied {call.name}"
```

The transcript payload can call `_permission_subject_kind(call)` instead of a new imported helper.

- [ ] **Step 4: Pass cwd into default policy**

In `src/colibri/session.py`, change default policy creation to:

```python
if self.permission_policy is None:
    self.permission_policy = PermissionPolicy.from_config(self.config, cwd=registry.cwd)
```

No CLI change is required if the session owns default policy creation.

- [ ] **Step 5: Run focused session tests**

Run: `uv run python -m pytest tests/unit/test_session.py -q`

Expected: PASS after updating old permission assertions.

- [ ] **Step 6: Commit**

```bash
git add src/colibri/session.py tests/unit/test_session.py
git commit -m "feat: route tool calls through dynamic grants"
```

---

### Task 5: Gitignore, Config, Diagnostics, and Docs

**Files:**
- Modify: `.gitignore`
- Modify: `configs/agent.example.toml`
- Modify: `README.md`
- Modify: `src/colibri/diagnostics.py`
- Test: `tests/unit/test_diagnostics.py`

**Interfaces:**
- Produces: `.colibri/permissions.toml` is ignored.
- Produces: docs clarify `shell.allow` is legacy/pregrant-like and `shell.deny` is hard deny.
- Produces: diagnostics show project permission file presence without printing secrets.

- [ ] **Step 1: Add failing ignore/docs diagnostics tests**

Append to `tests/unit/test_diagnostics.py`:

```python
def test_diagnostics_reports_project_permissions_file(tmp_path):
    (tmp_path / ".colibri").mkdir()
    (tmp_path / ".colibri" / "permissions.toml").write_text(
        '[shell]\ncommands = ["pwd"]\n\n[tools]\nnames = []\n',
        encoding="utf-8",
    )
    config = AgentConfig.default()

    lines = build_diagnostics(config, None, cwd=tmp_path)

    assert "project_permissions=present" in "\n".join(lines)
```

Update `build_diagnostics()` signature expectation in this test to require `cwd: Path | None = None`.

- [ ] **Step 2: Run diagnostics test to verify failure**

Run: `uv run python -m pytest tests/unit/test_diagnostics.py::test_diagnostics_reports_project_permissions_file -q`

Expected: FAIL because `build_diagnostics()` has no `cwd` parameter and does not report project permissions.

- [ ] **Step 3: Update `.gitignore`**

Append:

```gitignore
.colibri/permissions.toml
```

- [ ] **Step 4: Update diagnostics**

Modify `src/colibri/diagnostics.py`:

```python
from pathlib import Path
from colibri.permissions_store import ProjectPermissionStore


def build_diagnostics(config: AgentConfig, config_path: Path | None = None, cwd: Path | None = None) -> list[str]:
    project_store = ProjectPermissionStore.for_cwd(cwd or Path.cwd())
    project_permissions = "present" if project_store.path.exists() else "missing"
    ...
    lines.append(f"project_permissions={project_permissions}")
```

Preserve all existing diagnostics lines.

- [ ] **Step 5: Update config comments/docs**

In `configs/agent.example.toml`, keep `shell.allow` only if the implementation still reads it for backward compatibility, and add comments:

```toml
# shell.allow is legacy. Dynamic permissions prompt for ungranted commands.
# shell.deny remains a hard deny list.
[shell]
allow = ["ls", "cat", "sed", "rg", "python", "git status"]
deny = ["rm", "shutdown", "reboot", "mkfs", "dd", "sudo"]
```

In `README.md`, add a short section:

```markdown
### Dynamic Permissions

Colibri prompts before running ungranted shell commands and non-read-only tools. Choices are once, session, project, or deny. Project shell grants are exact command matches stored in `.colibri/permissions.toml`, which should not be committed. `shell.deny` remains a hard deny list.
```

- [ ] **Step 6: Run diagnostics and config tests**

Run: `uv run python -m pytest tests/unit/test_diagnostics.py tests/unit/test_config.py -q`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add .gitignore configs/agent.example.toml README.md src/colibri/diagnostics.py tests/unit/test_diagnostics.py
git commit -m "docs: document dynamic permissions"
```

---

### Task 6: Full Verification and Push

**Files:**
- No new source files unless prior tasks reveal missed imports.

**Interfaces:**
- Consumes: all prior tasks.
- Produces: green full test suite and pushed branch.

- [ ] **Step 1: Run full test suite**

Run: `uv run python -m pytest`

Expected: all tests pass.

- [ ] **Step 2: Run manual fake-model smoke**

Run:

```bash
printf 'hello\n/quit\n' | uv run python -m colibri.cli repl
```

Expected: exits `0`, prints a fake response, and does not prompt for permissions.

- [ ] **Step 3: Inspect git diff**

Run: `git status --short`

Expected: clean after commits.

- [ ] **Step 4: Push**

Run: `git push`

Expected: current branch pushes to remote.

---

## Self-Review

- Spec coverage: dynamic prompt flow, once/session/project/deny, complete-command project shell grants, hard-deny shell rules, file root boundary, project TOML store, stdin/stdout prompt, transcript payloads, `.gitignore`, and tests are covered.
- Scope check: this plan intentionally excludes dynamic file-root expansion, network risk classification, MCP subjects, permission CLI commands, and wildcard/prefix grants.
- Red-flag scan: no forbidden planning patterns remain.
- Type consistency: `ProjectGrants`, `ProjectPermissionStore`, `PermissionSubject`, and `PermissionDecisionResult` are introduced before later tasks consume them.
