# Colibri File Memory Tools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add file-backed memory tools so Colibri can list, read, search, and append persistent Markdown memory topics.

**Architecture:** The memory feature stays inside the existing tool system. `MemoryConfig` defines the disk root and search limit, `memory.py` implements focused built-in tools, and `ToolRegistry.from_config()` exposes them when `"memory"` is enabled.

**Tech Stack:** Python standard library, existing dataclass config, existing synchronous tool registry, pytest.

## Global Constraints

- Update design documentation before code changes.
- Preserve pure headless server operation through CLI/stdin/stdout.
- Use only Python standard library APIs.
- Keep memory files on disk, not in process memory.
- Do not implement automatic memory prompt injection in this milestone.
- Do not implement skills, MCP, GPIO, or network tools in this milestone.

---

## File Structure

- Modify `src/colibri/config.py`: add `MemoryConfig`, add it to `AgentConfig`, and load `[memory]` TOML overrides.
- Create `src/colibri/tools/builtin/memory.py`: implement `memory.list`, `memory.read`, `memory.search`, and `memory.write`.
- Modify `src/colibri/tools/builtin/__init__.py`: export memory tool classes.
- Modify `src/colibri/tools/registry.py`: register memory tools when `"memory"` is enabled.
- Modify `README.md`: describe Milestone 4 memory tools.
- Add or update tests in `tests/unit/test_config.py`, `tests/unit/test_tools.py`, and `tests/unit/test_session.py`.

## Tasks

### Task 1: Memory Config

**Files:**

- Modify: `src/colibri/config.py`
- Test: `tests/unit/test_config.py`

**Interfaces:**

- Produces: `MemoryConfig(root: Path, max_search_results: int)`
- Produces: `AgentConfig.memory`

Steps:

- [ ] Add a failing test that `[memory] root` and `max_search_results` overrides load into `AgentConfig.memory`.
- [ ] Run `uv run python -m pytest tests/unit/test_config.py` and confirm the new test fails because `AgentConfig` has no memory config.
- [ ] Add `MemoryConfig` to `src/colibri/config.py`.
- [ ] Update `AgentConfig.with_overrides()` to load `[memory]`, converting `root` through `expand_user_path`.
- [ ] Run `uv run python -m pytest tests/unit/test_config.py` and confirm it passes.

### Task 2: Memory Tool Implementation

**Files:**

- Create: `src/colibri/tools/builtin/memory.py`
- Modify: `src/colibri/tools/builtin/__init__.py`
- Test: `tests/unit/test_tools.py`

**Interfaces:**

- Produces: `MemoryListTool`
- Produces: `MemoryReadTool`
- Produces: `MemorySearchTool`
- Produces: `MemoryWriteTool`

Steps:

- [ ] Add failing tests for sorted topic listing, topic reading, invalid topic rejection, search result limits, append creation, and `MemoryWriteTool.spec.read_only is False`.
- [ ] Run `uv run python -m pytest tests/unit/test_tools.py` and confirm the new tests fail because memory tools do not exist.
- [ ] Implement `_topic_name()`, `_topic_path()`, and `_memory_root()` helpers in `memory.py`.
- [ ] Implement `MemoryListTool.run()` to return sorted topic names from `topics/*.md`.
- [ ] Implement `MemoryReadTool.run()` to read an existing topic or return `not_found`.
- [ ] Implement `MemorySearchTool.run()` to search `MEMORY.md` and topic files with case-insensitive substring matching.
- [ ] Implement `MemoryWriteTool.run()` to append one Markdown bullet and create missing directories.
- [ ] Export the classes from `src/colibri/tools/builtin/__init__.py`.
- [ ] Run `uv run python -m pytest tests/unit/test_tools.py` and confirm it passes.

### Task 3: Registry and Session Integration

**Files:**

- Modify: `src/colibri/tools/registry.py`
- Test: `tests/unit/test_tools.py`
- Test: `tests/unit/test_session.py`

**Interfaces:**

- Consumes: memory tool classes from Task 2.
- Produces: memory tool specs through `ToolRegistry.from_config()`.

Steps:

- [ ] Add a failing registry test that default `ToolRegistry.from_config()` exposes `memory.list`, `memory.read`, `memory.search`, and `memory.write`.
- [ ] Add a failing session test showing default permission policy asks for confirmation before `memory.write`.
- [ ] Run `uv run python -m pytest tests/unit/test_tools.py tests/unit/test_session.py` and confirm the new tests fail.
- [ ] Update `ToolRegistry.from_config()` to add memory tools when `"memory"` is enabled.
- [ ] Run `uv run python -m pytest tests/unit/test_tools.py tests/unit/test_session.py` and confirm both pass.

### Task 4: Docs and Verification

**Files:**

- Modify: `README.md`

Steps:

- [ ] Update README current milestone from Milestone 3 to Milestone 4.
- [ ] Document memory storage at `~/.colibri/memory`.
- [ ] Document `memory.list`, `memory.read`, `memory.search`, and `memory.write`.
- [ ] Run `uv run python -m pytest`.
- [ ] Run `uv run python -m colibri.cli ask "hello memory"`.
- [ ] Commit and push the milestone.

## Self-Review

- Spec coverage: the plan covers config, memory tools, registry integration, permission behavior for writes, README, and verification.
- Scope: automatic memory injection, skills, MCP, GPIO, and network tools are intentionally excluded.
- Placeholder scan: no incomplete implementation placeholders are present.
