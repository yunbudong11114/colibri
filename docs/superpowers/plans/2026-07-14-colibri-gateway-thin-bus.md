# Gateway Thin Bus Implementation Plan

> **For agentic workers:** Implement task-by-task. Rust first for latency/RSS; Python follows for parity.

**Goal:** Inbound light envelopes, deferred media, per-session queues, outbound sink, config `max_pending_inbound` / `max_concurrent_turns` (default 8 / 1). No cron/heartbeat.

**Architecture:** Spec `docs/superpowers/specs/2026-07-14-colibri-gateway-thin-bus-design.md`. Prefer minimal new modules; keep Weixin API code in place.

**Tech Stack:** Rust + Python parity tests.

---

### Task 1: Config fields (Rust then Python)

- Add `gateway.max_pending_inbound = 8`, `gateway.max_concurrent_turns = 1`
- Allowlist + example.toml + tests

### Task 2: Rust deferred media + light poll

- Parse keeps media item JSON refs; worker downloads before submit
- Tests: poll path does not call download; worker resolves

### Task 3: Rust per-session inbound + scheduler

- Replace single sync_channel with router: per-key queues + global pending bound
- `max_concurrent_turns` workers (default 1)
- Fair pick across non-empty session queues

### Task 4: Rust outbound sink

- Steer ack + final text + media go through small outbound helper (serial send)

### Task 5: Python parity

- take/put + SteerHandle, light poll, config, per-session queue + concurrent turns, outbound sink
- Remove `isinstance(WeixinChannel)` from gateway core

### Task 6: Docs + user config.toml + verify

- README, docs/README index, `~/.colibri/config.toml` gateway section
- pytest + cargo test
