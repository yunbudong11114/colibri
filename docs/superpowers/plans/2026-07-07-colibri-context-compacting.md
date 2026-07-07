# Colibri Context Compacting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep useful conversation continuity while bounding session history and model input size.

**Architecture:** Add deterministic compacting helpers that convert dropped messages into a bounded rolling summary. `AgentSession` injects summary and memory as temporary system context, then applies a character budget to model input before each model call.

**Tech Stack:** Python standard library, existing `AgentSession`, existing dataclass config, pytest.

## Global Constraints

- Update design documentation before code changes.
- Preserve pure headless server operation through CLI/stdin/stdout.
- Use only Python standard library APIs.
- Do not require network access for compacting.
- Do not keep full transcripts in memory.
- Do not add model-assisted summarization in this milestone.

---

## File Structure

- Create `src/colibri/context.py`: compact dropped messages, format summary context, and enforce input character budgets.
- Modify `src/colibri/session.py`: use compacting helpers, inject summary context, and log compact/budget events.
- Modify `README.md`: document Milestone 6 context compacting.
- Modify `docs/superpowers/specs/2026-07-01-colibri-design.md`: mark Milestone 6 complete after implementation.
- Add tests in `tests/unit/test_context.py`.
- Update tests in `tests/unit/test_session.py`.

## Tasks

### Task 1: Context Compacting Helpers

**Files:**

- Create: `src/colibri/context.py`
- Test: `tests/unit/test_context.py`

**Interfaces:**

- Produces: `summarize_messages(messages: list[Message], max_line_chars: int = 160) -> str`
- Produces: `append_summary(existing: str, addition: str, max_chars: int) -> str`
- Produces: `budget_model_messages(messages: list[Message], max_chars: int) -> tuple[list[Message], int]`

Steps:

- [ ] Add failing tests for user/assistant summary lines, tool result metadata summary, rolling summary bounds, and model message budgeting.
- [ ] Run `uv run python -m pytest tests/unit/test_context.py` and confirm it fails because `colibri.context` does not exist.
- [ ] Implement deterministic summary helpers.
- [ ] Implement character-based model input budgeting.
- [ ] Run `uv run python -m pytest tests/unit/test_context.py` and confirm it passes.

### Task 2: AgentSession Summary Integration

**Files:**

- Modify: `src/colibri/session.py`
- Test: `tests/unit/test_session.py`

**Interfaces:**

- Consumes: `summarize_messages`, `append_summary`, and `budget_model_messages`.
- Produces: summary temporary system message in model input.
- Produces: transcript event `context_compact`.

Steps:

- [ ] Add failing tests proving old messages update `session.summary`, summary is injected into model input, and summary is not stored in `session.messages`.
- [ ] Add a failing test proving `context_compact` transcript events are written.
- [ ] Run `uv run python -m pytest tests/unit/test_session.py` and confirm the new tests fail.
- [ ] Update `_trim_recent_messages()` to compact dropped messages into `self.summary`.
- [ ] Update `_model_messages()` to include summary context before memory context.
- [ ] Write `context_compact` transcript event when compaction happens.
- [ ] Run `uv run python -m pytest tests/unit/test_session.py` and confirm it passes.

### Task 3: Model Input Budget Integration

**Files:**

- Modify: `src/colibri/session.py`
- Test: `tests/unit/test_session.py`

**Interfaces:**

- Consumes: `budget_model_messages(messages, max_chars)`.
- Produces: transcript event `context_budget`.

Steps:

- [ ] Add a failing test proving model input is trimmed to fit `session.compact_trigger_chars` while keeping the latest user message.
- [ ] Add a failing test proving `context_budget` transcript event is written.
- [ ] Run `uv run python -m pytest tests/unit/test_session.py` and confirm failures.
- [ ] Apply model input budget before each model call.
- [ ] Run `uv run python -m pytest tests/unit/test_session.py` and confirm it passes.

### Task 4: Docs and Verification

**Files:**

- Modify: `README.md`
- Modify: `docs/superpowers/specs/2026-07-01-colibri-design.md`

Steps:

- [ ] Update README current milestone from Milestone 5 to Milestone 6.
- [ ] Document rolling summary compacting and model input budget.
- [ ] Mark Milestone 6 complete in the unified roadmap.
- [ ] Run `uv run python -m pytest`.
- [ ] Run `COLIBRI_HOME=/tmp/colibri-compact-smoke uv run python -m colibri.cli ask "hello compact"`.
- [ ] Commit and push the milestone.

## Self-Review

- Spec coverage: this plan covers deterministic compacting, summary injection, model input budget, transcript events, docs, and verification.
- Scope: model-assisted compacting, per-turn tool result budgets, shell permission fixes, skills, and MCP are intentionally excluded.
- Placeholder scan: no incomplete implementation placeholders are present.
