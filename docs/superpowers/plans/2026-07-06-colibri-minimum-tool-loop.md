# Colibri Minimum Tool Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a bounded agent loop with read-only built-in tools so Colibri can execute model-requested file and shell tool calls.

**Architecture:** `AgentSession` remains the coordinator. `ModelClient` returns `ModelResponse.tool_calls`, `ToolRegistry` exposes OpenAI-compatible tool schemas, and built-in tools execute only safe read-oriented actions with bounded output.

**Tech Stack:** Python 3.11+, standard library only at runtime, `pytest` through `uv` for tests.

## Global Constraints

- Target device is M5Stack CardputerZero / Raspberry Pi Compute Module 0 class Linux with about 512MB RAM.
- Runtime dependencies must remain zero beyond the Python standard library.
- Do not implement file writes, network tools, memory, skills, MCP, GPIO, or transcript logging.
- Do not run arbitrary shell commands.
- Execute tool calls sequentially.
- All tool output must be capped to `config.tools.max_result_chars`.
- Tests must avoid real model APIs.

---

## Task 1: Message Shape and Model Serialization

**Files:**
- Modify: `src/colibri/messages.py`
- Modify: `src/colibri/model/openai_compatible.py`
- Test: `tests/unit/test_openai_compatible_model.py`

**Interfaces:**
- Produces: `Message(role: str, content: str, tool_call_id: str | None = None, tool_calls: list[ToolCall] = field(default_factory=list))`
- Produces: OpenAI-compatible serialization for `role="tool"` messages and assistant messages with `tool_calls`

Steps:

- [ ] Add tests for serializing tool result messages and assistant tool calls.
- [ ] Run those tests and verify they fail because `Message` lacks tool fields and serialization ignores them.
- [ ] Add optional fields to `Message`.
- [ ] Update `_api_messages()` to include `tool_call_id` and assistant `tool_calls`.
- [ ] Run `uv run python -m pytest tests/unit/test_openai_compatible_model.py -v`.
- [ ] Commit with `feat: support tool messages`.

## Task 2: Tool Base, Registry, and Built-In Tools

**Files:**
- Create: `src/colibri/tools/__init__.py`
- Create: `src/colibri/tools/base.py`
- Create: `src/colibri/tools/registry.py`
- Create: `src/colibri/tools/builtin/__init__.py`
- Create: `src/colibri/tools/builtin/files.py`
- Create: `src/colibri/tools/builtin/shell.py`
- Test: `tests/unit/test_tools.py`

**Interfaces:**
- Produces: `ToolSpec`
- Produces: `ToolResult`
- Produces: `ToolContext`
- Produces: `ToolRegistry.from_config(config: AgentConfig, cwd: Path | None = None) -> ToolRegistry`
- Produces: `ToolRegistry.specs() -> list[dict]`
- Produces: `ToolRegistry.run(call: ToolCall, context: ToolContext) -> ToolResult`

Steps:

- [ ] Write tests for registry specs, unknown tools, `files.list`, `files.read`, output truncation, allowed shell, denied shell, and shell timeout.
- [ ] Run tests and verify they fail because tools do not exist.
- [ ] Implement base dataclasses/protocol.
- [ ] Implement path safety for file tools using `Path.resolve()` and configured roots.
- [ ] Implement `files.list` and `files.read`.
- [ ] Implement `shell.run` with `shlex.split()`, allow/deny checks, timeout, and bounded combined output.
- [ ] Implement registry construction from `config.tools.enabled`.
- [ ] Run `uv run python -m pytest tests/unit/test_tools.py -v`.
- [ ] Commit with `feat: add read only tools`.

## Task 3: Bounded Agent Loop

**Files:**
- Modify: `src/colibri/session.py`
- Test: `tests/unit/test_session.py`

**Interfaces:**
- Consumes: `ToolRegistry`
- Produces: `AgentSession(..., tools: ToolRegistry | None = None)`
- Produces: bounded tool loop in `AgentSession.submit()`

Steps:

- [ ] Add tests with a scripted model that first returns a `ToolCall`, then asserts it receives a `role="tool"` message and returns final text.
- [ ] Add test for max tool round limit.
- [ ] Run session tests and verify new tests fail.
- [ ] Update `AgentSession` to build/use a `ToolRegistry`.
- [ ] Append assistant tool-call messages and tool result messages.
- [ ] Stop with deterministic text when `max_tool_rounds` is reached.
- [ ] Run `uv run python -m pytest tests/unit/test_session.py -v`.
- [ ] Commit with `feat: add bounded tool loop`.

## Task 4: CLI Regression and Docs

**Files:**
- Modify: `README.md`
- Test: all tests

**Interfaces:**
- Produces: documented examples for `files.list`, `files.read`, and `shell.run` availability through model tool calls.

Steps:

- [ ] Update README to state Milestone 2 includes minimal read-only tools.
- [ ] Run `uv run python -m pytest`.
- [ ] Run `uv run python -m colibri.cli ask "hello colibri"`.
- [ ] Commit with `docs: document minimum tool loop`.

## Final Verification

- [ ] Run `uv run python -m pytest`.
- [ ] Run `uv run python -m colibri.cli ask "hello colibri"`.
- [ ] Run `git status --short --branch`.
- [ ] Push to `origin/main` after all tests pass and the user wants the milestone published.
