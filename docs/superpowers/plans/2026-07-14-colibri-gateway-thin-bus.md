# Gateway Thin Bus and Extensible Channels Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make channel addition a registry-only operation, including isolated interactive permission replies, while preserving the thin inbound bus and Python/Rust behavior parity.

**Architecture:** Rust first introduces a `GatewayChannel` adapter trait, a composition-only registry, and session-keyed permission waiters; generic gateway workers dispatch solely through the trait. Python then aligns the same lifecycle and fixes active-turn draining.

**Tech Stack:** Rust + Python parity tests.

## Global Constraints

- Update the design document before production code.
- Rust implementation and tests land before Python parity changes.
- Permission waiter identity is exactly `"{channel}:{sender_id}"`.
- Adding a channel must not require a branch or channel-name match in generic gateway code.
- Default `gateway.max_concurrent_turns = 1` and `gateway.max_pending_inbound = 8` remain unchanged.
- Do not add a second production channel in this work; use fake adapters for extension tests.
- Do not add cron or heartbeat behavior.

---

### Task 1: Rust adapter contract and session-keyed permission waiters

**Files:**
- Modify: `colibri-rust/src/channel.rs`
- Test: `colibri-rust/tests/runtime.rs`

- [ ] Add failing tests proving two channels sharing `sender_id` have isolated waiters.
- [ ] Run the focused Rust test and confirm it fails because waiters use bare sender IDs.
- [ ] Add `GatewayChannel`, `ChannelRegistry`, and `ChannelPermissionWaiters`; make the prompter accept a complete waiter key.
- [ ] Run the focused tests and confirm they pass.

### Task 2: Rust Weixin adapter and registry

**Files:**
- Create: `colibri-rust/src/channel_registry.rs`
- Modify: `colibri-rust/src/lib.rs`
- Modify: `colibri-rust/src/weixin.rs`
- Test: `colibri-rust/tests/runtime.rs`

- [ ] Add failing tests for config-driven registry construction and a fake adapter's outbound/media behavior.
- [ ] Run the focused tests and confirm the adapter/registry API is missing.
- [ ] Implement `WeixinGatewayChannel`, keeping poll cursor and Weixin sink inside `weixin.rs`.
- [ ] Implement `build_enabled_channels` as the only production composition root.
- [ ] Run the focused tests and confirm they pass.

### Task 3: Remove Weixin knowledge from Rust gateway

**Files:**
- Modify: `colibri-rust/src/gateway.rs`
- Test: `colibri-rust/src/gateway.rs`
- Test: `colibri-rust/tests/runtime.rs`

- [ ] Add a failing fake-channel dispatch test covering poll envelope, media resolution, outbound text and permission waiter delivery.
- [ ] Run it and confirm the gateway cannot dispatch without a Weixin branch.
- [ ] Replace `run_weixin_poll_loop`, `resolve_envelope_media`, `outbound_for`, and the enabled-name list with registry-driven generic code.
- [ ] Ensure a taken session is put back after both successful and failed submit.
- [ ] Assert by source scan that generic gateway has no Weixin import, literal or channel-name match.
- [ ] Run Rust gateway, runtime and parity tests.

### Task 4: Python active-turn drain and explicit waiter identity

**Files:**
- Modify: `src/colibri/inbound_router.py`
- Modify: `src/colibri/gateway.py`
- Modify: `src/colibri/channels/weixin.py`
- Test: `tests/unit/test_inbound_router.py`
- Test: `tests/unit/test_channels.py`
- Test: `tests/unit/test_gateway_steering.py`

- [ ] Add failing tests showing `idle` includes active work and a slow acquired turn finishes before gateway return/session close.
- [ ] Add a failing test showing waiter storage uses the full channel session key.
- [ ] Run the focused Python tests and confirm the old pending-only drain fails.
- [ ] Implement router `active_len`/`wait_idle` and use it during finite-poller shutdown.
- [ ] Key Python channel waiter storage through the shared session-key helper.
- [ ] Run focused tests and confirm they pass.

### Task 5: Cross-runtime parity and documentation audit

**Files:**
- Modify: `colibri-rust/tests/parity.rs`
- Modify: `README.md` only if the documented channel extension or permission behavior is stale

- [ ] Map every new Python behavior test to an executable Rust counterpart.
- [ ] Verify numeric permission prompts and timeout-to-deny behavior remain identical.
- [ ] Verify config fields/defaults did not drift.
- [ ] Run full Python and Rust test suites.

### Task 6: Release verification

**Files:**
- No production changes unless verification exposes a defect.

- [ ] Run `uv run python -m pytest -q` and record the pass count.
- [ ] Run `cargo test --manifest-path colibri-rust/Cargo.toml` and record the pass count.
- [ ] Run `cargo build --release --manifest-path colibri-rust/Cargo.toml`.
- [ ] Confirm the release binary timestamp and current commit.
