# Colibri Weixin Immediate Dispatch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Dispatch every Weixin inbound message immediately to the existing agent worker queue.

**Architecture:** Retain the receive-loop and single-worker queue boundary. Remove only the complementary-message batcher, timers, merge helpers, and their configuration.

**Tech Stack:** Python 3.11+, standard library queue/threading, pytest.

## Global Constraints

- Update design documentation before production code.
- Add no third-party dependencies.
- Preserve permission text waiters.
- Preserve the bounded worker queue and stop-aware shutdown.
- Do not retain compatibility for message_debounce_seconds.

### Task 1: Immediate dispatch behavior

**Files:**
- Modify: `tests/unit/test_channels.py`
- Modify: `src/colibri/channels/weixin.py`

- [ ] Replace the cross-poll aggregation test with a failing test expecting two ordered handler calls.
- [ ] Publish each non-waiter message directly with `_publish_work`.
- [ ] Delete batcher state, timers, close handling, and merge helpers.
- [ ] Run channel tests and preserve worker shutdown and permission behavior.

### Task 2: Remove obsolete configuration and documentation

**Files:**
- Modify: `src/colibri/config.py`
- Modify: `configs/agent.example.toml`
- Modify: `tests/unit/test_config.py`
- Modify: `docs/superpowers/specs/2026-07-09-colibri-weixin-media-design.md`
- Modify: `docs/superpowers/specs/2026-07-10-colibri-runtime-hardening-design.md`

- [ ] Remove message_debounce_seconds from configuration and examples.
- [ ] Remove obsolete aggregation expectations from existing design documents.
- [ ] Run configuration tests, full pytest, and `git diff --check`.
