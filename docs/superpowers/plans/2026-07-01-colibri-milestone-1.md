# Colibri Milestone 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first runnable Python skeleton for the CardputerZero lightweight agent: package layout, config loading, message/session types, fake model loop, and CLI REPL.

**Architecture:** Milestone 1 keeps the runtime synchronous and dependency-light. `AgentSession` coordinates messages and a `ModelClient` interface, while the CLI only handles user input/output and delegates all agent behavior to the session.

**Tech Stack:** Python 3.11+, standard library only at runtime, `pytest` for development tests, `src/` package layout.

## Global Constraints

- Target device is M5Stack CardputerZero / Raspberry Pi Compute Module 0 class Linux with about 512MB RAM.
- Runtime dependencies should be none beyond the Python standard library if practical.
- Use TOML via Python 3.11 `tomllib`.
- Keep v1 mostly synchronous.
- Do not implement MCP, skills, memory, real shell/file tools, voice, browser automation, subagents, or streaming in this milestone.
- All strings entering session state must be bounded.
- Use a standard `src/` layout and avoid runtime dependency on `uv`, Poetry, or Hatch on the device.

---

## File Structure

- Create `pyproject.toml`: project metadata and pytest configuration.
- Create `README.md`: concise local development and CLI usage notes.
- Create `configs/agent.example.toml`: example config matching the approved design defaults.
- Create `src/colibri/__init__.py`: package version export.
- Create `src/colibri/config.py`: config dataclasses, TOML loading, path expansion, defaults.
- Create `src/colibri/messages.py`: model-facing message and response dataclasses.
- Create `src/colibri/model/base.py`: `ModelClient` protocol.
- Create `src/colibri/model/fake.py`: deterministic model for tests and local smoke runs.
- Create `src/colibri/session.py`: `AgentSession`, bounded message handling, simple submit flow.
- Create `src/colibri/cli.py`: `argparse` CLI with `ask` and `repl`.
- Create `tests/unit/test_config.py`: config behavior.
- Create `tests/unit/test_session.py`: session behavior with fake model.
- Create `tests/unit/test_cli.py`: CLI smoke tests.

### Task 1: Project Skeleton and Config Loader

**Files:**
- Create: `pyproject.toml`
- Create: `README.md`
- Create: `configs/agent.example.toml`
- Create: `src/colibri/__init__.py`
- Create: `src/colibri/config.py`
- Test: `tests/unit/test_config.py`

**Interfaces:**
- Produces: `AgentConfig.load(path: str | Path | None = None) -> AgentConfig`
- Produces: `AgentConfig.default() -> AgentConfig`
- Produces: `ModelConfig`, `SessionConfig`, `ToolsConfig`, `ShellConfig`, `FilesConfig`, `SkillsConfig`, `McpConfig`
- Produces: `expand_user_path(value: str) -> Path`

- [ ] **Step 1: Write the failing config tests**

```python
from pathlib import Path

from colibri.config import AgentConfig, expand_user_path


def test_default_config_uses_small_device_limits():
    config = AgentConfig.default()

    assert config.model.provider == "fake"
    assert config.model.model == "fake-colibri-model"
    assert config.session.max_tool_rounds == 6
    assert config.session.recent_message_limit == 16
    assert config.session.compact_trigger_chars == 36000
    assert config.tools.max_result_chars == 12000
    assert config.shell.deny[:3] == ["rm", "shutdown", "reboot"]


def test_load_config_overrides_nested_values(tmp_path):
    config_path = tmp_path / "agent.toml"
    config_path.write_text(
        """
[model]
provider = "openai_compatible"
model = "gpt-4.1-mini"
timeout_seconds = 45

[session]
recent_message_limit = 8

[files]
roots = ["~/notes", "/tmp"]
""".strip(),
        encoding="utf-8",
    )

    config = AgentConfig.load(config_path)

    assert config.model.provider == "openai_compatible"
    assert config.model.model == "gpt-4.1-mini"
    assert config.model.timeout_seconds == 45
    assert config.session.recent_message_limit == 8
    assert config.files.roots[0].name == "notes"
    assert config.files.roots[1] == Path("/tmp")


def test_expand_user_path_expands_home():
    expanded = expand_user_path("~/.colibri")

    assert expanded.is_absolute()
    assert expanded.name == ".colibri"
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `python -m pytest tests/unit/test_config.py -v`

Expected: FAIL with `ModuleNotFoundError: No module named 'colibri'`.

- [ ] **Step 3: Implement project metadata and config code**

Create `pyproject.toml`:

```toml
[build-system]
requires = ["setuptools>=68"]
build-backend = "setuptools.build_meta"

[project]
name = "colibri"
version = "0.1.0"
description = "Lightweight Python agent runtime for CardputerZero"
readme = "README.md"
requires-python = ">=3.11"
dependencies = []

[project.scripts]
colibri = "colibri.cli:main"

[tool.setuptools.packages.find]
where = ["src"]

[tool.pytest.ini_options]
testpaths = ["tests"]
pythonpath = ["src"]
```

Create `README.md`:

```markdown
# Colibri

Lightweight Python agent runtime for CardputerZero-class Linux devices.

Milestone 1 includes a package skeleton, config loader, fake model session, and CLI smoke path.

```bash
python -m pytest
python -m colibri.cli ask "hello"
python -m colibri.cli repl
```
```

Create `configs/agent.example.toml`:

```toml
[model]
provider = "openai_compatible"
base_url = "https://api.openai.com/v1"
model = "gpt-4.1-mini"
api_key_env = "OPENAI_API_KEY"
timeout_seconds = 60
max_output_tokens = 1024

[session]
max_tool_rounds = 6
recent_message_limit = 16
compact_trigger_chars = 36000
summary_max_chars = 6000
idle_exit_seconds = 300
transcript = true

[tools]
enabled = ["shell", "files", "http", "memory", "skills", "mcp"]
default_permission = "allow_read_confirm_write"
max_result_chars = 12000
max_shell_seconds = 30

[shell]
allow = ["ls", "cat", "sed", "rg", "python", "git status"]
deny = ["rm", "shutdown", "reboot", "mkfs", "dd", "sudo"]

[files]
roots = ["~/.colibri", "/tmp"]
confirm_write = true

[skills]
dirs = ["~/.colibri/skills"]
max_loaded = 20

[mcp]
enabled = false
startup = "lazy"
max_active_servers = 1
```

Create `src/colibri/__init__.py`:

```python
__version__ = "0.1.0"
```

Create `src/colibri/config.py`:

```python
from __future__ import annotations

from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any
import tomllib


def expand_user_path(value: str) -> Path:
    return Path(value).expanduser().resolve()


@dataclass(frozen=True)
class ModelConfig:
    provider: str = "fake"
    base_url: str = "https://api.openai.com/v1"
    model: str = "fake-colibri-model"
    api_key_env: str = "OPENAI_API_KEY"
    timeout_seconds: int = 60
    max_output_tokens: int = 1024


@dataclass(frozen=True)
class SessionConfig:
    max_tool_rounds: int = 6
    recent_message_limit: int = 16
    compact_trigger_chars: int = 36000
    summary_max_chars: int = 6000
    idle_exit_seconds: int = 300
    transcript: bool = True


@dataclass(frozen=True)
class ToolsConfig:
    enabled: list[str] = field(default_factory=lambda: ["shell", "files", "http", "memory", "skills", "mcp"])
    default_permission: str = "allow_read_confirm_write"
    max_result_chars: int = 12000
    max_shell_seconds: int = 30


@dataclass(frozen=True)
class ShellConfig:
    allow: list[str] = field(default_factory=lambda: ["ls", "cat", "sed", "rg", "python", "git status"])
    deny: list[str] = field(default_factory=lambda: ["rm", "shutdown", "reboot", "mkfs", "dd", "sudo"])


@dataclass(frozen=True)
class FilesConfig:
    roots: list[Path] = field(default_factory=lambda: [expand_user_path("~/.colibri"), Path("/tmp")])
    confirm_write: bool = True


@dataclass(frozen=True)
class SkillsConfig:
    dirs: list[Path] = field(default_factory=lambda: [expand_user_path("~/.colibri/skills")])
    max_loaded: int = 20


@dataclass(frozen=True)
class McpConfig:
    enabled: bool = False
    startup: str = "lazy"
    max_active_servers: int = 1


@dataclass(frozen=True)
class AgentConfig:
    model: ModelConfig = field(default_factory=ModelConfig)
    session: SessionConfig = field(default_factory=SessionConfig)
    tools: ToolsConfig = field(default_factory=ToolsConfig)
    shell: ShellConfig = field(default_factory=ShellConfig)
    files: FilesConfig = field(default_factory=FilesConfig)
    skills: SkillsConfig = field(default_factory=SkillsConfig)
    mcp: McpConfig = field(default_factory=McpConfig)

    @classmethod
    def default(cls) -> "AgentConfig":
        return cls()

    @classmethod
    def load(cls, path: str | Path | None = None) -> "AgentConfig":
        if path is None:
            return cls.default()
        data = tomllib.loads(Path(path).read_text(encoding="utf-8"))
        return cls.default().with_overrides(data)

    def with_overrides(self, data: dict[str, Any]) -> "AgentConfig":
        return replace(
            self,
            model=_replace_dataclass(self.model, data.get("model", {})),
            session=_replace_dataclass(self.session, data.get("session", {})),
            tools=_replace_dataclass(self.tools, data.get("tools", {})),
            shell=_replace_dataclass(self.shell, data.get("shell", {})),
            files=_replace_dataclass(self.files, _path_list_overrides(data.get("files", {}), "roots")),
            skills=_replace_dataclass(self.skills, _path_list_overrides(data.get("skills", {}), "dirs")),
            mcp=_replace_dataclass(self.mcp, data.get("mcp", {})),
        )


def _replace_dataclass(instance: Any, overrides: dict[str, Any]) -> Any:
    if not overrides:
        return instance
    return replace(instance, **overrides)


def _path_list_overrides(overrides: dict[str, Any], key: str) -> dict[str, Any]:
    copied = dict(overrides)
    if key in copied:
        copied[key] = [expand_user_path(value) for value in copied[key]]
    return copied
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `python -m pytest tests/unit/test_config.py -v`

Expected: 3 passed.

### Task 2: Messages, Fake Model, and Agent Session

**Files:**
- Create: `src/colibri/messages.py`
- Create: `src/colibri/model/__init__.py`
- Create: `src/colibri/model/base.py`
- Create: `src/colibri/model/fake.py`
- Create: `src/colibri/session.py`
- Test: `tests/unit/test_session.py`

**Interfaces:**
- Consumes: `AgentConfig`, `SessionConfig`
- Produces: `Message(role: str, content: str)`
- Produces: `ModelResponse(text: str, tool_calls: list[ToolCall])`
- Produces: `AgentResponse(text: str, messages: list[Message])`
- Produces: `ModelClient.complete(messages: list[Message], tools: list[dict], system: str, limits: ModelLimits) -> ModelResponse`
- Produces: `AgentSession.submit(user_text: str) -> AgentResponse`
- Produces: `AgentSession.reset() -> None`

- [ ] **Step 1: Write the failing session tests**

```python
from colibri.config import AgentConfig
from colibri.model.fake import FakeModelClient
from colibri.session import AgentSession


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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `python -m pytest tests/unit/test_session.py -v`

Expected: FAIL with `ModuleNotFoundError` or missing `AgentSession`.

- [ ] **Step 3: Implement message, model, and session code**

Create `src/colibri/messages.py`:

```python
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass(frozen=True)
class Message:
    role: str
    content: str


@dataclass(frozen=True)
class ToolCall:
    id: str
    name: str
    arguments: dict[str, Any]


@dataclass(frozen=True)
class ModelLimits:
    timeout_seconds: int
    max_output_tokens: int


@dataclass(frozen=True)
class ModelResponse:
    text: str
    tool_calls: list[ToolCall] = field(default_factory=list)


@dataclass(frozen=True)
class AgentResponse:
    text: str
    messages: list[Message]
```

Create `src/colibri/model/__init__.py`:

```python
from colibri.model.base import ModelClient
from colibri.model.fake import FakeModelClient

__all__ = ["FakeModelClient", "ModelClient"]
```

Create `src/colibri/model/base.py`:

```python
from __future__ import annotations

from typing import Protocol

from colibri.messages import Message, ModelLimits, ModelResponse


class ModelClient(Protocol):
    def complete(
        self,
        messages: list[Message],
        tools: list[dict],
        system: str,
        limits: ModelLimits,
    ) -> ModelResponse:
        ...
```

Create `src/colibri/model/fake.py`:

```python
from __future__ import annotations

from colibri.messages import Message, ModelLimits, ModelResponse


class FakeModelClient:
    def complete(
        self,
        messages: list[Message],
        tools: list[dict],
        system: str,
        limits: ModelLimits,
    ) -> ModelResponse:
        last_user = next((message.content for message in reversed(messages) if message.role == "user"), "")
        return ModelResponse(text=f"fake: {last_user}")
```

Create `src/colibri/session.py`:

```python
from __future__ import annotations

from dataclasses import dataclass, field
from time import monotonic

from colibri.config import AgentConfig
from colibri.messages import AgentResponse, Message, ModelLimits
from colibri.model.base import ModelClient


SYSTEM_PROMPT = (
    "You are a lightweight personal agent running on a CardputerZero-class Linux device. "
    "Prefer short, practical responses and respect low memory, battery, and tool limits."
)


@dataclass
class AgentSession:
    config: AgentConfig
    model: ModelClient
    messages: list[Message] = field(default_factory=list)
    summary: str = ""
    started_at: float = field(default_factory=monotonic)
    last_activity_at: float = field(default_factory=monotonic)

    def submit(self, user_text: str) -> AgentResponse:
        bounded_text = self._bound_text(user_text, self.config.session.compact_trigger_chars)
        self.messages.append(Message(role="user", content=bounded_text))
        self._trim_recent_messages()

        model_response = self.model.complete(
            messages=list(self.messages),
            tools=[],
            system=SYSTEM_PROMPT,
            limits=ModelLimits(
                timeout_seconds=self.config.model.timeout_seconds,
                max_output_tokens=self.config.model.max_output_tokens,
            ),
        )
        assistant_text = self._bound_text(model_response.text, self.config.tools.max_result_chars)
        self.messages.append(Message(role="assistant", content=assistant_text))
        self._trim_recent_messages()
        self.last_activity_at = monotonic()

        return AgentResponse(text=assistant_text, messages=list(self.messages))

    def reset(self) -> None:
        self.messages.clear()
        self.summary = ""
        self.last_activity_at = monotonic()

    def close(self) -> None:
        return None

    def _trim_recent_messages(self) -> None:
        limit = self.config.session.recent_message_limit
        if len(self.messages) > limit:
            self.messages = self.messages[-limit:]

    @staticmethod
    def _bound_text(text: str, max_chars: int) -> str:
        if len(text) <= max_chars:
            return text
        keep = max(0, max_chars - len("\n...[truncated]"))
        return text[:keep] + "\n...[truncated]"
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `python -m pytest tests/unit/test_session.py -v`

Expected: 3 passed.

### Task 3: CLI Ask and REPL

**Files:**
- Create: `src/colibri/cli.py`
- Test: `tests/unit/test_cli.py`

**Interfaces:**
- Consumes: `AgentConfig.load`
- Consumes: `FakeModelClient`
- Consumes: `AgentSession.submit`
- Produces: `main(argv: list[str] | None = None) -> int`

- [ ] **Step 1: Write the failing CLI tests**

```python
from colibri.cli import main


def test_ask_prints_fake_response(capsys):
    exit_code = main(["ask", "status"])

    captured = capsys.readouterr()

    assert exit_code == 0
    assert captured.out.strip() == "fake: status"


def test_repl_exits_on_quit(monkeypatch, capsys):
    inputs = iter(["hello", "/quit"])
    monkeypatch.setattr("builtins.input", lambda _: next(inputs))

    exit_code = main(["repl"])

    captured = capsys.readouterr()

    assert exit_code == 0
    assert "fake: hello" in captured.out
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `python -m pytest tests/unit/test_cli.py -v`

Expected: FAIL with missing `colibri.cli`.

- [ ] **Step 3: Implement CLI**

Create `src/colibri/cli.py`:

```python
from __future__ import annotations

import argparse
from pathlib import Path
from typing import Sequence

from colibri.config import AgentConfig
from colibri.model.fake import FakeModelClient
from colibri.session import AgentSession


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="colibri")
    parser.add_argument("--config", type=Path, default=None)
    subparsers = parser.add_subparsers(dest="command", required=True)

    ask = subparsers.add_parser("ask")
    ask.add_argument("text")

    subparsers.add_parser("repl")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    config = AgentConfig.load(args.config)
    session = AgentSession(config=config, model=FakeModelClient())

    if args.command == "ask":
        print(session.submit(args.text).text)
        return 0

    if args.command == "repl":
        return _run_repl(session)

    return 2


def _run_repl(session: AgentSession) -> int:
    while True:
        try:
            user_text = input("colibri> ")
        except EOFError:
            print()
            return 0

        if user_text.strip() in {"/quit", "/exit"}:
            return 0
        if not user_text.strip():
            continue

        print(session.submit(user_text).text)


if __name__ == "__main__":
    raise SystemExit(main())
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `python -m pytest tests/unit/test_cli.py -v`

Expected: 2 passed.

### Task 4: Milestone Verification

**Files:**
- Modify: `README.md`

**Interfaces:**
- Consumes: all interfaces from Tasks 1-3.
- Produces: documented smoke commands.

- [ ] **Step 1: Run all tests**

Run: `python -m pytest -v`

Expected: 8 passed.

- [ ] **Step 2: Run CLI smoke command**

Run: `python -m colibri.cli ask "hello cardputer"`

Expected stdout:

```text
fake: hello cardputer
```

- [ ] **Step 3: Update README with milestone status**

Replace `README.md` with:

```markdown
# Colibri

Lightweight Python agent runtime for CardputerZero-class Linux devices.

## Current Milestone

Milestone 1 provides:

- Python package skeleton.
- TOML config loader with CardputerZero-friendly defaults.
- Message and model interfaces.
- Deterministic fake model for tests and smoke runs.
- `AgentSession.submit()` for a bounded single model turn.
- CLI `ask` and `repl` commands.

## Development

```bash
python -m pytest
python -m colibri.cli ask "hello"
python -m colibri.cli repl
```

The runtime is standard-library only. `pytest` is only needed for development tests.
```

- [ ] **Step 4: Run final verification**

Run: `python -m pytest -v`

Expected: 8 passed.

Run: `python -m colibri.cli ask "hello cardputer"`

Expected: `fake: hello cardputer`

## Self-Review

- Spec coverage: This plan covers Milestone 1 from the approved design: `pyproject.toml`, config loader, console CLI, message types, fake model, and basic `AgentSession`.
- Intentional gaps: OpenAI-compatible HTTP calls, tool registry, permissions, transcript JSONL, skills, memory, compacting, and MCP are deferred to later milestones listed in the approved design.
- Placeholder scan: No `TBD`, `TODO`, or undefined follow-up code remains.
- Type consistency: `AgentConfig`, `Message`, `ModelResponse`, `ModelClient`, and `AgentSession.submit()` signatures are consistent across tasks.
