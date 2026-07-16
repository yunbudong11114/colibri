# Colibri Model Network Resilience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make transient model failures retry safely and keep REPL/gateway sessions alive without channel-specific logic.

**Architecture:** Concrete model clients classify and retry transient provider failures using model configuration. `AgentSession` converts exhausted model failures into a normal failed `AgentResponse`; generic REPL and gateway code handle that response while channel adapters remain unchanged.

**Tech Stack:** Python 3.11, urllib, pytest, Rust 2021, ureq, Cargo tests

## Global Constraints

- Follow `docs/superpowers/specs/2026-07-16-colibri-model-network-resilience-design.md`.
- Modify no channel adapter implementation.
- Default to two retries with deterministic 500ms/1000ms backoff.
- Retry only network, timeout, 408, 429, and 5xx failures.
- Keep one-shot `ask` non-zero on exhausted model failure.
- Preserve Python/Rust parity.

---

### Task 1: Python model classification and retry

**Files:**
- Modify: `src/colibri/config.py`
- Modify: `src/colibri/model/errors.py`
- Modify: `src/colibri/model/openai_compatible.py`
- Modify: `tests/unit/test_config.py`
- Modify: `tests/unit/test_openai_compatible_model.py`

- [ ] Add failing tests for retry categories, retry counts, 500/1000ms delay, zero retries, and permanent errors.
- [ ] Run focused tests and confirm RED.
- [ ] Add `model.max_retries` and `model.retry_backoff_ms`.
- [ ] Add stable `ModelError.category` and `retryable`.
- [ ] Implement bounded retry inside the OpenAI-compatible model client for text and image requests.
- [ ] Run focused tests and confirm GREEN.

### Task 2: Python failed-turn isolation

**Files:**
- Modify: `src/colibri/messages.py`
- Modify: `src/colibri/session.py`
- Modify: `src/colibri/cli.py`
- Modify: `src/colibri/gateway.py`
- Modify: `tests/unit/test_session.py`
- Modify: `tests/unit/test_cli.py`
- Modify: `tests/unit/test_gateway_process.py` or gateway-focused tests

- [ ] Add failing tests proving a failed turn returns `error_type`, a later turn succeeds, REPL continues, ask exits non-zero, and gateway handles a second message.
- [ ] Confirm RED.
- [ ] Extend `AgentResponse` with optional `error_type`.
- [ ] Convert exhausted `ModelError` into a bounded assistant failure response in `AgentSession`.
- [ ] Make ask inspect `error_type`; leave REPL response handling generic.
- [ ] Ensure gateway worker treats the failed response as ordinary outbound text.
- [ ] Confirm channel modules have no model/retry imports.
- [ ] Run focused tests and confirm GREEN.

### Task 3: Rust retry and failed-turn parity

**Files:**
- Modify: `colibri-rust/src/config.rs`
- Modify: `colibri-rust/src/model.rs`
- Modify: `colibri-rust/src/messages.rs`
- Modify: `colibri-rust/src/session.rs`
- Modify: `colibri-rust/src/cli.rs`
- Modify: `colibri-rust/src/gateway.rs`
- Modify: `colibri-rust/tests/runtime.rs`
- Modify: `colibri-rust/tests/parity.rs`

- [ ] Add failing Rust tests matching Tasks 1 and 2.
- [ ] Confirm RED.
- [ ] Add model retry configuration.
- [ ] Classify HTTP/network failures internally in the OpenAI-compatible model and retry transient categories.
- [ ] Return failed `AgentResponse` from session after exhaustion.
- [ ] Keep REPL and gateway loops alive; keep ask non-zero.
- [ ] Update parity maps and run focused tests to GREEN.

### Task 4: Documentation, full verification, and deployment

**Files:**
- Modify: `README.md`
- Modify: `README.zh-CN.md`
- Modify: `configs/agent.example.toml`

- [ ] Document retry fields and non-terminating REPL/gateway behavior.
- [ ] Run full Python tests.
- [ ] Run full Rust tests outside the sandbox if network/process tests require it.
- [ ] Search channel implementations for forbidden model/retry dependencies.
- [ ] Build the Rust release binary.
- [ ] Copy it to `~/.local/bin/colibri` and verify SHA-256 equality.
- [ ] Review `git diff --check` and commit intended files only.
