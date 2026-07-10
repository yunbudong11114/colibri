# Colibri Session Structure Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make AgentSession easier to maintain while reducing repeated runtime dependency allocation.

**Architecture:** Keep AgentSession as the single conversation coordinator. Cache session-lifetime collaborators lazily and extract focused private methods from submit without introducing new files or public abstractions.

**Tech Stack:** Python 3.11+, dataclasses, standard library, pytest.

## Global Constraints

- Update the design document before production code.
- Add no third-party dependencies.
- Preserve public AgentSession construction and submit behavior.
- Keep memory and skill loading dynamic on every submitted turn.
- Do not duplicate message buffers beyond existing bounded model and response copies.

---

### Task 1: Session-lifetime dependency reuse and restore diagnostics

**Files:**
- Modify: `src/colibri/session.py`
- Test: `tests/unit/test_session.py`

**Interfaces:**
- Consumes: existing `AgentSession.tools`, `permission_policy`, `history_loader`, `config`, and `model` fields.
- Produces: private `_runtime_dependencies() -> tuple[ToolRegistry, PermissionPolicy, ImageAnalyzer]` and a `history_restore_error` transcript event.

- [ ] Add tests that monkeypatch `ToolRegistry.from_config` and `ImageAnalyzer` and assert each is constructed once across two submits.
- [ ] Add a test whose history loader raises and assert submission succeeds while transcript contains `history_restore_error` with exception type and message.
- [ ] Run the focused tests and confirm they fail for missing caching and diagnostics.
- [ ] Implement lazy reuse of ToolRegistry, PermissionPolicy, and ImageAnalyzer.
- [ ] Record non-fatal history restore failures in transcript.
- [ ] Run the focused tests and require them to pass.

### Task 2: Submit orchestration extraction

**Files:**
- Modify: `src/colibri/session.py`
- Test: `tests/unit/test_session.py`

**Interfaces:**
- Consumes: the cached runtime dependencies from Task 1.
- Produces: private methods for preparing a user turn, loading dynamic context, completing one model step, executing one tool call, and returning the round-limit response.

- [ ] Refactor one behavior at a time without changing transcript event order or message mutation order.
- [ ] Keep MemoryContext loading and SkillIndex scanning inside the per-submit path.
- [ ] Run the complete session test module after each extraction.
- [ ] Run the full pytest suite and `git diff --check` after the final extraction.
