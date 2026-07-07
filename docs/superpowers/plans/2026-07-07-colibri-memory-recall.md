# Colibri Memory Recall Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Automatically inject relevant file-backed memory into model calls.

**Architecture:** Add a small `MemoryRecall` component that reads `MEMORY.md`, scores topics deterministically, reads selected topic files within budget, and returns a temporary context block. `AgentSession` calls it once per user turn and passes the extra message only to the model call.

**Tech Stack:** Python standard library, existing dataclass config, existing `AgentSession`, pytest.

## Global Constraints

- Update design documentation before code changes.
- Preserve pure headless server operation through CLI/stdin/stdout.
- Use only Python standard library APIs.
- Do not keep all memory files resident in process memory.
- Do not add embeddings, vector databases, model-based memory selection, skills, MCP, GPIO, or network tools in this milestone.

---

## File Structure

- Modify `src/colibri/config.py`: add memory recall config fields.
- Create `src/colibri/memory.py`: implement memory index parsing, scoring, and recall formatting.
- Modify `src/colibri/session.py`: inject temporary memory context and transcript event.
- Modify `configs/agent.example.toml`: document recall config fields.
- Modify `README.md`: document automatic memory recall.
- Add or update tests in `tests/unit/test_config.py`, `tests/unit/test_memory.py`, and `tests/unit/test_session.py`.

## Tasks

### Task 1: Memory Recall Config

**Files:**

- Modify: `src/colibri/config.py`
- Modify: `configs/agent.example.toml`
- Test: `tests/unit/test_config.py`

**Interfaces:**

- Produces: `MemoryConfig.enabled: bool`
- Produces: `MemoryConfig.max_recall_topics: int`
- Produces: `MemoryConfig.max_recall_chars: int`

Steps:

- [ ] Add a failing test that TOML overrides load `memory.enabled`, `memory.max_recall_topics`, and `memory.max_recall_chars`.
- [ ] Run `uv run python -m pytest tests/unit/test_config.py` and confirm the test fails.
- [ ] Add fields to `MemoryConfig`.
- [ ] Update `configs/agent.example.toml`.
- [ ] Run `uv run python -m pytest tests/unit/test_config.py` and confirm it passes.

### Task 2: Memory Recall Component

**Files:**

- Create: `src/colibri/memory.py`
- Test: `tests/unit/test_memory.py`

**Interfaces:**

- Produces: `MemoryRecallResult(text: str, topics: list[str], truncated: bool)`
- Produces: `MemoryRecall(config: AgentConfig).recall(user_text: str, messages: list[Message]) -> MemoryRecallResult`

Steps:

- [ ] Add failing tests for index parsing, keyword scoring, topic limit, char limit, disabled recall, and missing files.
- [ ] Run `uv run python -m pytest tests/unit/test_memory.py` and confirm tests fail because `colibri.memory` does not exist.
- [ ] Implement tokenization and index parsing.
- [ ] Implement deterministic scoring.
- [ ] Implement topic file reading and context block formatting.
- [ ] Implement character truncation with `...[truncated]`.
- [ ] Run `uv run python -m pytest tests/unit/test_memory.py` and confirm it passes.

### Task 3: AgentSession Injection

**Files:**

- Modify: `src/colibri/session.py`
- Test: `tests/unit/test_session.py`

**Interfaces:**

- Consumes: `MemoryRecall.recall(user_text, messages)`
- Produces: temporary `Message(role="system", content=memory_text)` in model input only.
- Produces: transcript event `memory_recall`.

Steps:

- [ ] Add a failing session test proving the model receives relevant memory text.
- [ ] Add a failing session test proving memory text is not persisted in `session.messages`.
- [ ] Add a failing session test proving transcript logs `memory_recall` with topics and `truncated`.
- [ ] Run `uv run python -m pytest tests/unit/test_session.py` and confirm failures.
- [ ] Wire `MemoryRecall` into `AgentSession.submit()`.
- [ ] Run `uv run python -m pytest tests/unit/test_session.py` and confirm it passes.

### Task 4: Docs and Verification

**Files:**

- Modify: `README.md`

Steps:

- [ ] Update README current milestone from Milestone 4 to Milestone 5.
- [ ] Document automatic memory recall and the relevant config fields.
- [ ] Run `uv run python -m pytest`.
- [ ] Run `COLIBRI_HOME=/tmp/colibri-recall-smoke uv run python -m colibri.cli ask "hello recall"`.
- [ ] Commit and push the milestone.

## Self-Review

- Spec coverage: this plan covers config, recall selection, injection, transcript metadata, docs, and verification.
- Scope: embeddings, vector search, model-based selection, memory rewriting, skills, MCP, GPIO, and network tools are intentionally excluded.
- Placeholder scan: no incomplete implementation placeholders are present.
