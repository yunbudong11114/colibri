# Colibri OpenAI-Compatible Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a zero-runtime-dependency OpenAI-compatible chat completion model adapter so Colibri can use either the current fake model or a real configured API model.

**Architecture:** Keep `AgentSession` unchanged as the conversation coordinator. Add a model factory that turns `ModelConfig.provider` into a `ModelClient`, then implement `OpenAICompatibleModelClient` behind the existing `ModelClient.complete()` interface using standard-library HTTP.

**Tech Stack:** Python 3.11+, standard library only at runtime (`urllib.request`, `json`, `os`, `dataclasses`), `pytest` for tests.

## Global Constraints

- Target device is M5Stack CardputerZero / Raspberry Pi Compute Module 0 class Linux with about 512MB RAM.
- Runtime dependencies must remain zero beyond the Python standard library.
- Use OpenAI-compatible `/chat/completions` in this milestone.
- Do not implement shell, file, HTTP, memory, skills, GPIO, or MCP tools in this milestone.
- Do not implement the full bounded tool loop in this milestone.
- Do not implement streaming responses in this milestone.
- Do not store API keys in config files or transcripts.
- Tests must avoid real network calls.
- Before code changes, the design document must already exist and be reviewed: `docs/superpowers/specs/2026-07-06-colibri-openai-compatible-model-design.md`.

---

## File Structure

- Modify `src/colibri/config.py`: add `ConfigError`.
- Create `src/colibri/model/errors.py`: add `ModelError`.
- Create `src/colibri/model/factory.py`: choose model client from `ModelConfig`.
- Create `src/colibri/model/openai_compatible.py`: implement standard-library HTTP adapter.
- Modify `src/colibri/model/__init__.py`: export new model symbols.
- Modify `src/colibri/cli.py`: build model through factory and handle expected errors.
- Create `configs/openai.example.toml`: real API example without secrets.
- Modify `README.md`: document fake default and OpenAI-compatible config usage.
- Create `tests/unit/test_model_factory.py`: provider selection tests.
- Create `tests/unit/test_openai_compatible_model.py`: adapter request/response/error tests.
- Modify `tests/unit/test_cli.py`: expected error handling and fake path regression.

---

### Task 1: Model Errors and Factory

**Files:**
- Modify: `src/colibri/config.py`
- Create: `src/colibri/model/errors.py`
- Create: `src/colibri/model/factory.py`
- Modify: `src/colibri/model/__init__.py`
- Test: `tests/unit/test_model_factory.py`

**Interfaces:**
- Consumes: `colibri.config.ModelConfig`
- Produces: `colibri.config.ConfigError`
- Produces: `colibri.model.errors.ModelError`
- Produces: `build_model_client(config: ModelConfig) -> ModelClient`
- Produces: `OpenAICompatibleModelClient.from_config(config: ModelConfig) -> OpenAICompatibleModelClient` as a stub dependency that Task 2 fills in

- [ ] **Step 1: Write failing factory tests**

Create `tests/unit/test_model_factory.py`:

```python
import pytest

from colibri.config import ConfigError, ModelConfig
from colibri.model.factory import build_model_client
from colibri.model.fake import FakeModelClient
from colibri.model.openai_compatible import OpenAICompatibleModelClient


def test_factory_returns_fake_model_for_default_provider():
    client = build_model_client(ModelConfig())

    assert isinstance(client, FakeModelClient)


def test_factory_returns_openai_compatible_model(monkeypatch):
    monkeypatch.setenv("COLIBRI_TEST_API_KEY", "test-key")
    config = ModelConfig(
        provider="openai_compatible",
        base_url="https://api.example.test/v1",
        model="test-model",
        api_key_env="COLIBRI_TEST_API_KEY",
    )

    client = build_model_client(config)

    assert isinstance(client, OpenAICompatibleModelClient)
    assert client.base_url == "https://api.example.test/v1"
    assert client.model == "test-model"


def test_factory_rejects_unknown_provider():
    config = ModelConfig(provider="mystery")

    with pytest.raises(ConfigError, match="Unsupported model provider: mystery"):
        build_model_client(config)
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
python -m pytest tests/unit/test_model_factory.py -v
```

Expected: FAIL with `ModuleNotFoundError` for `colibri.model.factory` or `colibri.model.openai_compatible`.

- [ ] **Step 3: Add configuration and model error classes**

Modify `src/colibri/config.py` near the imports:

```python
class ConfigError(RuntimeError):
    pass
```

Create `src/colibri/model/errors.py`:

```python
class ModelError(RuntimeError):
    pass
```

- [ ] **Step 4: Add a minimal OpenAI-compatible client stub**

Create `src/colibri/model/openai_compatible.py`:

```python
from __future__ import annotations

from dataclasses import dataclass
import os

from colibri.config import ConfigError, ModelConfig
from colibri.messages import Message, ModelLimits, ModelResponse


@dataclass(frozen=True)
class OpenAICompatibleModelClient:
    base_url: str
    model: str
    api_key: str

    @classmethod
    def from_config(cls, config: ModelConfig) -> "OpenAICompatibleModelClient":
        api_key = os.environ.get(config.api_key_env, "")
        if not api_key:
            raise ConfigError(f"Missing API key environment variable: {config.api_key_env}")
        return cls(base_url=config.base_url.rstrip("/"), model=config.model, api_key=api_key)

    def complete(
        self,
        messages: list[Message],
        tools: list[dict],
        system: str,
        limits: ModelLimits,
    ) -> ModelResponse:
        raise NotImplementedError("OpenAI-compatible completion is implemented in Task 2")
```

- [ ] **Step 5: Add the model factory**

Create `src/colibri/model/factory.py`:

```python
from __future__ import annotations

from colibri.config import ConfigError, ModelConfig
from colibri.model.base import ModelClient
from colibri.model.fake import FakeModelClient
from colibri.model.openai_compatible import OpenAICompatibleModelClient


def build_model_client(config: ModelConfig) -> ModelClient:
    if config.provider == "fake":
        return FakeModelClient()
    if config.provider == "openai_compatible":
        return OpenAICompatibleModelClient.from_config(config)
    raise ConfigError(f"Unsupported model provider: {config.provider}")
```

- [ ] **Step 6: Export model symbols**

Modify `src/colibri/model/__init__.py`:

```python
from colibri.model.base import ModelClient
from colibri.model.errors import ModelError
from colibri.model.factory import build_model_client
from colibri.model.fake import FakeModelClient
from colibri.model.openai_compatible import OpenAICompatibleModelClient

__all__ = [
    "FakeModelClient",
    "ModelClient",
    "ModelError",
    "OpenAICompatibleModelClient",
    "build_model_client",
]
```

- [ ] **Step 7: Run factory tests**

Run:

```bash
python -m pytest tests/unit/test_model_factory.py -v
```

Expected: PASS with 3 tests.

- [ ] **Step 8: Run existing model-adjacent tests**

Run:

```bash
python -m pytest tests/unit/test_config.py tests/unit/test_session.py -v
```

Expected: PASS.

- [ ] **Step 9: Commit Task 1**

Run:

```bash
git add src/colibri/config.py src/colibri/model/__init__.py src/colibri/model/errors.py src/colibri/model/factory.py src/colibri/model/openai_compatible.py tests/unit/test_model_factory.py
git commit -m "feat: add model client factory"
```

---

### Task 2: OpenAI-Compatible Chat Completion Adapter

**Files:**
- Modify: `src/colibri/model/openai_compatible.py`
- Test: `tests/unit/test_openai_compatible_model.py`

**Interfaces:**
- Consumes: `OpenAICompatibleModelClient.from_config(config: ModelConfig) -> OpenAICompatibleModelClient`
- Consumes: `Message`, `ModelLimits`, `ModelResponse`, `ToolCall`
- Produces: `OpenAICompatibleModelClient.complete(messages, tools, system, limits) -> ModelResponse`
- Produces: `_chat_completions_url() -> str`
- Produces: `_request_json(url: str, payload: dict, timeout_seconds: int) -> dict`

- [ ] **Step 1: Write failing adapter tests**

Create `tests/unit/test_openai_compatible_model.py`:

```python
import json
import urllib.error

import pytest

from colibri.config import ConfigError, ModelConfig
from colibri.messages import Message, ModelLimits
from colibri.model.errors import ModelError
from colibri.model.openai_compatible import OpenAICompatibleModelClient


def make_client() -> OpenAICompatibleModelClient:
    return OpenAICompatibleModelClient(
        base_url="https://api.example.test/v1",
        model="test-model",
        api_key="test-key",
    )


def test_from_config_requires_api_key(monkeypatch):
    monkeypatch.delenv("COLIBRI_MISSING_KEY", raising=False)
    config = ModelConfig(provider="openai_compatible", api_key_env="COLIBRI_MISSING_KEY")

    with pytest.raises(ConfigError, match="COLIBRI_MISSING_KEY"):
        OpenAICompatibleModelClient.from_config(config)


def test_complete_builds_chat_completion_request(monkeypatch):
    client = make_client()
    captured = {}

    def fake_request(self, url, payload, timeout_seconds):
        captured["url"] = url
        captured["payload"] = payload
        captured["timeout_seconds"] = timeout_seconds
        return {"choices": [{"message": {"content": "hi there"}}]}

    monkeypatch.setattr(OpenAICompatibleModelClient, "_request_json", fake_request)

    response = client.complete(
        messages=[Message(role="user", content="hello")],
        tools=[],
        system="system prompt",
        limits=ModelLimits(timeout_seconds=12, max_output_tokens=34),
    )

    assert response.text == "hi there"
    assert captured["url"] == "https://api.example.test/v1/chat/completions"
    assert captured["timeout_seconds"] == 12
    assert captured["payload"] == {
        "model": "test-model",
        "messages": [
            {"role": "system", "content": "system prompt"},
            {"role": "user", "content": "hello"},
        ],
        "max_completion_tokens": 34,
    }


def test_complete_passes_tools_when_present(monkeypatch):
    client = make_client()
    captured = {}
    tools = [{"type": "function", "function": {"name": "lookup", "parameters": {"type": "object"}}}]

    def fake_request(self, url, payload, timeout_seconds):
        captured["payload"] = payload
        return {"choices": [{"message": {"content": "done"}}]}

    monkeypatch.setattr(OpenAICompatibleModelClient, "_request_json", fake_request)

    client.complete(
        messages=[Message(role="user", content="use a tool")],
        tools=tools,
        system="",
        limits=ModelLimits(timeout_seconds=5, max_output_tokens=20),
    )

    assert captured["payload"]["tools"] == tools
    assert captured["payload"]["messages"] == [{"role": "user", "content": "use a tool"}]


def test_complete_parses_tool_calls(monkeypatch):
    client = make_client()

    def fake_request(self, url, payload, timeout_seconds):
        return {
            "choices": [
                {
                    "message": {
                        "content": None,
                        "tool_calls": [
                            {
                                "id": "call_1",
                                "type": "function",
                                "function": {
                                    "name": "lookup",
                                    "arguments": "{\"city\": \"Shanghai\"}",
                                },
                            }
                        ],
                    }
                }
            ]
        }

    monkeypatch.setattr(OpenAICompatibleModelClient, "_request_json", fake_request)

    response = client.complete(
        messages=[Message(role="user", content="weather")],
        tools=[],
        system="",
        limits=ModelLimits(timeout_seconds=5, max_output_tokens=20),
    )

    assert response.text == ""
    assert len(response.tool_calls) == 1
    assert response.tool_calls[0].id == "call_1"
    assert response.tool_calls[0].name == "lookup"
    assert response.tool_calls[0].arguments == {"city": "Shanghai"}


def test_complete_rejects_empty_choices(monkeypatch):
    client = make_client()
    monkeypatch.setattr(
        OpenAICompatibleModelClient,
        "_request_json",
        lambda self, url, payload, timeout_seconds: {"choices": []},
    )

    with pytest.raises(ModelError, match="missing choices"):
        client.complete(
            messages=[Message(role="user", content="hello")],
            tools=[],
            system="",
            limits=ModelLimits(timeout_seconds=5, max_output_tokens=20),
        )


def test_request_json_turns_http_error_into_model_error():
    client = make_client()
    body = json.dumps({"error": {"message": "bad auth"}}).encode("utf-8")
    error = urllib.error.HTTPError(
        url="https://api.example.test/v1/chat/completions",
        code=401,
        msg="Unauthorized",
        hdrs={},
        fp=FakeErrorBody(body),
    )

    with pytest.raises(ModelError, match="HTTP 401"):
        client._perform_request(error)


class FakeErrorBody:
    def __init__(self, body: bytes):
        self.body = body

    def read(self) -> bytes:
        return self.body
```

- [ ] **Step 2: Run adapter tests to verify they fail**

Run:

```bash
python -m pytest tests/unit/test_openai_compatible_model.py -v
```

Expected: FAIL because `complete()` raises `NotImplementedError` and `_perform_request()` does not exist.

- [ ] **Step 3: Replace the adapter stub with implementation**

Modify `src/colibri/model/openai_compatible.py`:

```python
from __future__ import annotations

from dataclasses import dataclass
import json
import os
import urllib.error
import urllib.request

from colibri.config import ConfigError, ModelConfig
from colibri.messages import Message, ModelLimits, ModelResponse, ToolCall
from colibri.model.errors import ModelError


@dataclass(frozen=True)
class OpenAICompatibleModelClient:
    base_url: str
    model: str
    api_key: str

    @classmethod
    def from_config(cls, config: ModelConfig) -> "OpenAICompatibleModelClient":
        api_key = os.environ.get(config.api_key_env, "")
        if not api_key:
            raise ConfigError(f"Missing API key environment variable: {config.api_key_env}")
        return cls(base_url=config.base_url.rstrip("/"), model=config.model, api_key=api_key)

    def complete(
        self,
        messages: list[Message],
        tools: list[dict],
        system: str,
        limits: ModelLimits,
    ) -> ModelResponse:
        payload: dict = {
            "model": self.model,
            "messages": self._api_messages(messages=messages, system=system),
            "max_completion_tokens": limits.max_output_tokens,
        }
        if tools:
            payload["tools"] = tools

        data = self._request_json(self._chat_completions_url(), payload, limits.timeout_seconds)
        return self._parse_response(data)

    def _chat_completions_url(self) -> str:
        return f"{self.base_url}/chat/completions"

    def _api_messages(self, messages: list[Message], system: str) -> list[dict[str, str]]:
        api_messages = []
        if system:
            api_messages.append({"role": "system", "content": system})
        api_messages.extend({"role": message.role, "content": message.content} for message in messages)
        return api_messages

    def _request_json(self, url: str, payload: dict, timeout_seconds: int) -> dict:
        body = json.dumps(payload).encode("utf-8")
        request = urllib.request.Request(
            url=url,
            data=body,
            method="POST",
            headers={
                "Authorization": f"Bearer {self.api_key}",
                "Content-Type": "application/json",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=timeout_seconds) as response:
                return json.loads(response.read().decode("utf-8"))
        except urllib.error.HTTPError as error:
            return self._perform_request(error)
        except urllib.error.URLError as error:
            raise ModelError(f"Model request failed: {error.reason}") from error
        except TimeoutError as error:
            raise ModelError("Model request timed out") from error
        except json.JSONDecodeError as error:
            raise ModelError("Model response was not valid JSON") from error

    def _perform_request(self, error: urllib.error.HTTPError) -> dict:
        body = error.read().decode("utf-8", errors="replace")
        compact = body[:500]
        raise ModelError(f"Model request failed with HTTP {error.code}: {compact}") from error

    def _parse_response(self, data: dict) -> ModelResponse:
        choices = data.get("choices")
        if not choices:
            raise ModelError("Model response missing choices")

        message = choices[0].get("message", {})
        text = message.get("content") or ""
        tool_calls = [self._parse_tool_call(item) for item in message.get("tool_calls", [])]
        return ModelResponse(text=text, tool_calls=tool_calls)

    def _parse_tool_call(self, item: dict) -> ToolCall:
        function = item.get("function", {})
        raw_arguments = function.get("arguments") or "{}"
        try:
            arguments = json.loads(raw_arguments)
        except json.JSONDecodeError:
            arguments = {"raw": raw_arguments}
        return ToolCall(
            id=item.get("id", ""),
            name=function.get("name", ""),
            arguments=arguments,
        )
```

- [ ] **Step 4: Run adapter tests**

Run:

```bash
python -m pytest tests/unit/test_openai_compatible_model.py -v
```

Expected: PASS.

- [ ] **Step 5: Run factory tests again**

Run:

```bash
python -m pytest tests/unit/test_model_factory.py -v
```

Expected: PASS.

- [ ] **Step 6: Commit Task 2**

Run:

```bash
git add src/colibri/model/openai_compatible.py tests/unit/test_openai_compatible_model.py
git commit -m "feat: add openai compatible model adapter"
```

---

### Task 3: CLI Integration, Example Config, and Docs

**Files:**
- Modify: `src/colibri/cli.py`
- Create: `configs/openai.example.toml`
- Modify: `README.md`
- Modify: `tests/unit/test_cli.py`

**Interfaces:**
- Consumes: `build_model_client(config.model) -> ModelClient`
- Consumes: `ConfigError`
- Consumes: `ModelError`
- Produces: CLI return code `1` for expected config/model failures

- [ ] **Step 1: Write failing CLI tests**

Modify `tests/unit/test_cli.py`:

```python
from colibri.cli import main
from colibri.model.errors import ModelError


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


def test_main_returns_one_for_expected_model_errors(monkeypatch, capsys):
    class BrokenModel:
        def complete(self, messages, tools, system, limits):
            raise ModelError("boom")

    monkeypatch.setattr("colibri.cli.build_model_client", lambda config: BrokenModel())

    exit_code = main(["ask", "hello"])

    captured = capsys.readouterr()

    assert exit_code == 1
    assert captured.err.strip() == "Model error: boom"
```

- [ ] **Step 2: Run CLI tests to verify they fail**

Run:

```bash
python -m pytest tests/unit/test_cli.py -v
```

Expected: FAIL because `colibri.cli.build_model_client` does not exist or `ModelError` is not caught.

- [ ] **Step 3: Update CLI to use model factory and catch expected errors**

Modify `src/colibri/cli.py`:

```python
from __future__ import annotations

import argparse
from pathlib import Path
import sys
from typing import Sequence

from colibri.config import AgentConfig, ConfigError
from colibri.model.errors import ModelError
from colibri.model.factory import build_model_client
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
    try:
        args = build_parser().parse_args(argv)
        config = AgentConfig.load(args.config)
        session = AgentSession(config=config, model=build_model_client(config.model))

        if args.command == "ask":
            print(session.submit(args.text).text)
            return 0

        if args.command == "repl":
            return _run_repl(session)

        return 2
    except ConfigError as error:
        print(f"Config error: {error}", file=sys.stderr)
        return 1
    except ModelError as error:
        print(f"Model error: {error}", file=sys.stderr)
        return 1


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

        try:
            print(session.submit(user_text).text)
        except ModelError as error:
            print(f"Model error: {error}", file=sys.stderr)
            return 1


if __name__ == "__main__":
    raise SystemExit(main())
```

- [ ] **Step 4: Add OpenAI-compatible example config**

Create `configs/openai.example.toml`:

```toml
[model]
provider = "openai_compatible"
base_url = "https://api.openai.com/v1"
model = "gpt-5.5"
api_key_env = "OPENAI_API_KEY"
timeout_seconds = 60
max_output_tokens = 1024

[session]
recent_message_limit = 16
compact_trigger_chars = 36000
```

- [ ] **Step 5: Update README usage**

Modify `README.md` to include:

```markdown
## Model Providers

Colibri defaults to the deterministic fake model:

```bash
PYTHONPATH=src python -m colibri.cli ask "hello"
```

To use an OpenAI-compatible chat completions API, copy `configs/openai.example.toml`, set `OPENAI_API_KEY` in the environment, and pass the config:

```bash
PYTHONPATH=src python -m colibri.cli --config configs/openai.example.toml ask "say hi in five words"
```

The runtime does not read API keys from config files. It reads the environment variable named by `model.api_key_env`.
```

- [ ] **Step 6: Run CLI tests**

Run:

```bash
python -m pytest tests/unit/test_cli.py -v
```

Expected: PASS.

- [ ] **Step 7: Run all tests**

Run:

```bash
python -m pytest
```

Expected: PASS with all unit tests.

- [ ] **Step 8: Run fake CLI smoke**

Run:

```bash
PYTHONPATH=src python -m colibri.cli ask "hello colibri"
```

Expected:

```text
fake: hello colibri
```

- [ ] **Step 9: Run missing-key CLI smoke**

Run:

```bash
PYTHONPATH=src python -m colibri.cli --config configs/openai.example.toml ask "hello"
```

Expected: exit code `1` and stderr:

```text
Config error: Missing API key environment variable: OPENAI_API_KEY
```

- [ ] **Step 10: Commit Task 3**

Run:

```bash
git add README.md configs/openai.example.toml src/colibri/cli.py tests/unit/test_cli.py
git commit -m "feat: wire cli to model factory"
```

---

## Final Verification

- [ ] **Step 1: Run full tests**

Run:

```bash
python -m pytest
```

Expected: PASS.

- [ ] **Step 2: Run fake CLI smoke**

Run:

```bash
PYTHONPATH=src python -m colibri.cli ask "hello colibri"
```

Expected:

```text
fake: hello colibri
```

- [ ] **Step 3: Check git state**

Run:

```bash
git status --short --branch
```

Expected: only pre-existing untracked `.superpowers/` remains, unless the user asks to include or remove it.

- [ ] **Step 4: Optional real API smoke**

Run only if the user explicitly provides a valid API key in the environment:

```bash
PYTHONPATH=src python -m colibri.cli --config configs/openai.example.toml ask "say hi in five words"
```

Expected: a concise assistant response from the configured provider.
