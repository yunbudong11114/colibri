# Colibri Runtime Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve valid tool-call history, make Weixin shutdown bounded, bound temporary media storage, and remove inactive runtime configuration.

**Architecture:** Keep the existing session and Weixin module boundaries. Add reusable logical-message grouping to `context.py`, make the Weixin queue publisher stop-aware, and implement opportunistic media cleanup with module-level constants and throttling.

**Tech Stack:** Python 3.11 standard library, pycryptodome, pytest.

## Global Constraints

- Update design documentation before production code.
- Use no new third-party dependencies.
- Preserve current image understanding, permission, and gateway session behavior.
- Keep the runtime suitable for a 128 MB small Linux server.

---

### Task 1: Atomic message history

**Files:**
- Modify: `src/colibri/context.py`
- Modify: `src/colibri/session.py`
- Test: `tests/unit/test_context.py`
- Test: `tests/unit/test_session.py`

- [ ] Add failing tests where recent retention and character budgeting cut between an assistant tool call and its tool result.
- [ ] Verify the focused tests fail by exposing an orphan tool result.
- [ ] Add logical-message grouping helpers and use them in both history operations.
- [ ] Verify the focused context and session tests pass.

### Task 2: Bounded Weixin worker shutdown

**Files:**
- Modify: `src/colibri/channels/weixin.py`
- Test: `tests/unit/test_channels.py`

- [ ] Add a failing regression test that fills the work queue and raises from the worker handler.
- [ ] Verify the channel thread fails to terminate under the old implementation.
- [ ] Add a shared stop event, interruptible queue publication, non-blocking finalization, and bounded worker join.
- [ ] Verify the regression and existing aggregation and permission tests pass.

### Task 3: Temporary media cleanup

**Files:**
- Modify: `src/colibri/channels/weixin.py`
- Test: `tests/unit/test_channels.py`

- [ ] Add failing tests for age cleanup, total-size cleanup, and ignored filesystem errors.
- [ ] Verify the cleanup helper is absent.
- [ ] Implement throttled direct-child cleanup and call it at channel startup and before inbound writes.
- [ ] Verify all focused Weixin tests pass.

### Task 4: Remove inactive runtime surface

**Files:**
- Modify: `src/colibri/config.py`
- Modify: `configs/agent.example.toml`
- Modify: `src/colibri/channels/weixin.py`
- Modify: `tests/unit/test_config.py`
- Modify: relevant README statements that claim Python MCP support

- [ ] Update config tests to assert MCP is not enabled or exposed and remove `max_recall_topics` expectations.
- [ ] Verify config tests fail against the old defaults.
- [ ] Remove inactive config fields, example values, and the unused text helper while preserving unknown-section loading.
- [ ] Verify config and channel tests pass.

### Task 5: Full verification

**Files:**
- Review all modified files listed above.

- [ ] Run `uv run python -m pytest -q` and require all tests to pass.
- [ ] Run `git diff --check` and require no whitespace errors.
- [ ] Review `git status --short` and ensure unrelated user files remain untouched.
