# Colibri Runtime Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Harden Colibri runtime behavior after the full code pass without changing user-facing command names or adding dependencies.

**Architecture:** Keep the existing modules and split only the REPL line editor out of `cli.py`. Make gateway process management safer at the process-manager boundary, and make gateway session storage safe for future multi-channel concurrency.

**Tech Stack:** Python standard library, pytest, existing TOML config parser, no third-party runtime packages.

## Global Constraints

- Documentation must be updated before code.
- No third-party runtime dependencies.
- Keep existing CLI commands and config keys.
- Keep headless Linux and CardputerZero compatibility.
- Keep Weixin QR raw payload fallback.

---

### Task 1: Gateway Process Safety

**Files:**
- Modify: `src/colibri/cli.py`
- Modify: `src/colibri/gateway_process.py`
- Test: `tests/unit/test_cli.py`
- Test: `tests/unit/test_gateway_process.py`

**Interfaces:**
- Consumes: `GatewayProcessManager.start(config_path)`, `stop()`, `status()`, `restart(config_path)`
- Produces: process identity helpers inside `gateway_process.py`; CLI dispatch that lets `gateway status` and `gateway stop` run without full config load.

- [x] Add tests proving `gateway status` and `gateway stop` do not call the config loader.
- [x] Add tests proving unverified PIDs are not killed.
- [x] Add tests proving verified gateway PIDs receive termination signals.
- [x] Move gateway process actions before full config load in `main()`.
- [x] Add conservative process command verification before sending signals.
- [x] Run targeted gateway tests.

### Task 2: Gateway Session Cache Locking

**Files:**
- Modify: `src/colibri/gateway.py`
- Test: `tests/unit/test_channels.py`

**Interfaces:**
- Consumes: `GatewaySessionCache.get()`, `touch()`, `close()`
- Produces: lock-protected session cache internals with unchanged public methods.

- [x] Keep existing cache reuse and eviction behavior tests passing after adding locking.
- [x] Add `threading.Lock` to `GatewaySessionCache`.
- [x] Guard `_entries` mutations with the lock.
- [x] Keep `AgentSession.submit()` outside the cache lock.
- [x] Run channel tests.

### Task 3: Split REPL Input

**Files:**
- Create: `src/colibri/repl_input.py`
- Modify: `src/colibri/cli.py`
- Test: `tests/unit/test_cli.py`

**Interfaces:**
- Consumes: existing `read_repl_line`, `ReplLineEditor`, `read_escape_sequence`, `read_tty_byte`, `write_raw_tty_newline`
- Produces: same function/class names exported from `colibri.repl_input`.

- [x] Move REPL input code from `cli.py` into `repl_input.py`.
- [x] Update `cli.py` to import `read_repl_line`.
- [x] Update tests to import editor helpers from `colibri.repl_input`.
- [x] Run CLI tests.

### Task 4: Prompt and Payload Cleanup

**Files:**
- Modify: `src/colibri/session.py`
- Modify: `src/colibri/model/openai_compatible.py`
- Test: `tests/unit/test_session.py`
- Test: `tests/unit/test_openai_compatible_model.py`

**Interfaces:**
- Consumes: `SYSTEM_PROMPT`, `OpenAICompatibleModelClient._request_json`
- Produces: properly spaced prompt and UTF-8 JSON request body.

- [x] Add or update tests for prompt spacing.
- [x] Add or update tests that captured model request bytes contain Chinese UTF-8 text rather than escaped Unicode.
- [x] Fix `SYSTEM_PROMPT`.
- [x] Use `json.dumps(payload, ensure_ascii=False)`.
- [x] Run model and session tests.

### Task 5: QR Behavior Test Tightening

**Files:**
- Test: `tests/unit/test_channels.py`
- Test: `tests/unit/test_terminal_qr.py`

**Interfaces:**
- Consumes: `perform_weixin_auth`, `render_terminal_qr`
- Produces: tests documenting best-effort QR plus raw payload fallback.

- [x] Ensure auth output test asserts raw payload URL is still printed.
- [x] Keep QR renderer tests dependency-free.
- [x] Run QR/channel tests.

### Task 6: Full Verification

**Files:**
- All changed files.

**Interfaces:**
- Consumes: all tasks above.
- Produces: passing test suite and clean status summary.

- [x] Run `uv run pytest`.
- [x] Run secret/reference scan.
- [x] Check `git status --short`.
