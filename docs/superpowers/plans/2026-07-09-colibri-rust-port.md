# Colibri Rust Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a new `colibri-rust/` Cargo project that compiles to a directly usable Colibri-compatible CLI and has Rust tests derived from the Python unit suite.

**Architecture:** The Rust port is a binary-plus-library crate. It mirrors Python's observable runtime behavior through focused modules for config, CLI, session, model, tools, memory, skills, transcript, gateway process management, Rust-native blocking HTTP, and Weixin iLink integration. Rust integration tests include a Python/Rust CLI parity harness for deterministic command output and a coverage map that scans every Python `tests/unit/test_*.py::test_*` function, requires an explicit Rust mapping for each function, and rejects partial parity entries.

**Tech Stack:** Rust stable, Cargo, Rust standard library, `toml`, `serde_json`, `shell-words`, Python test runner through `uv run python -m pytest`.

## Global Constraints

- Modify design documentation before code changes.
- Use focused low-memory crates only when they remove parity risk.
- Keep the Python implementation intact.
- `auth weixin` performs the iLink QR auth flow through the Rust-native HTTP client and writes token/base URL back to config.
- Gateway start/stop/restart manage a background `colibri-rust gateway run` process.
- MCP server startup remains outside this Rust port.
- Verification must include `cargo test`, `cargo build --release`, and direct binary smoke tests.

---

### Task 1: Crate Scaffold And Behavior Tests

**Files:**
- Create: `colibri-rust/Cargo.toml`
- Create: `colibri-rust/src/lib.rs`
- Create: `colibri-rust/src/main.rs`
- Create: `colibri-rust/src/*.rs`

**Interfaces:**
- Produces: `colibri_rust::cli::run_with_io(args, stdin, stdout, stderr) -> i32`
- Produces: `colibri_rust::config::AgentConfig::load(path: Option<&Path>) -> Result<AgentConfig, String>`
- Produces: `colibri_rust::session::AgentSession::submit(&mut self, text: &str) -> Result<AgentResponse, String>`

- [ ] **Step 1: Create failing tests for default config, CLI ask, diagnostics, gateway usage, Weixin auth, web search, session fake response, and tools.**

```bash
cargo test --manifest-path colibri-rust/Cargo.toml
```

Expected: tests fail because modules are empty or unimplemented.

- [ ] **Step 2: Implement the minimal modules to pass tests.**

Use std-only Rust modules listed in the design document.

- [ ] **Step 3: Run tests until green.**

```bash
cargo test --manifest-path colibri-rust/Cargo.toml
```

Expected: all Rust tests pass.

### Task 2: Buildable CLI Binary

**Files:**
- Modify: `colibri-rust/src/main.rs`
- Modify: `colibri-rust/src/cli.rs`

**Interfaces:**
- Consumes: `run_with_io`
- Produces: `target/release/colibri-rust`

- [ ] **Step 1: Wire `main` to collect process args and call the CLI runner.**
- [ ] **Step 2: Build release binary.**

```bash
cargo build --release --manifest-path colibri-rust/Cargo.toml
```

Expected: build exits with code 0 and creates `colibri-rust/target/release/colibri-rust`.

- [ ] **Step 3: Smoke test binary.**

```bash
./colibri-rust/target/release/colibri-rust ask "status"
./colibri-rust/target/release/colibri-rust diagnostics
./colibri-rust/target/release/colibri-rust gateway status
```

Expected: fake response, diagnostics lines, and gateway status print successfully.

### Task 3: README Update

**Files:**
- Modify: `README.md`

**Interfaces:**
- Documents: Rust build, test, and run commands.
- Documents: Rust build, run, HTTP, gateway, Weixin, and remaining MCP compatibility limits.

- [ ] **Step 1: Add a concise Rust port section to README.**
- [ ] **Step 2: Re-run verification commands from Task 2 after the documentation edit.**

### Task 4: Python-Derived Rust Parity Tests

**Files:**
- Create: `colibri-rust/tests/parity.rs`
- Modify: `colibri-rust/tests/runtime.rs`
- Modify: `README.md`
- Modify: `README.zh-CN.md`

**Interfaces:**
- Consumes: Python CLI command `uv run python -m colibri.cli`.
- Consumes: Rust test binary path from `env!("CARGO_BIN_EXE_colibri-rust")`.
- Produces: `python_test_coverage_map_covers_all_unit_files()` with one row for every `tests/unit/test_*.py` file and a function-level inventory for every Python `test_*`.
- Produces: CLI parity helpers that compare exit code, stdout, and stderr for deterministic commands.

- [ ] **Step 1: Write failing coverage-map and CLI parity tests.**

```rust
#[test]
fn python_test_coverage_map_covers_all_unit_files() {
    let expected = [
        "test_channels.py",
        "test_cli.py",
        "test_config.py",
        "test_console.py",
        "test_context.py",
        "test_diagnostics.py",
        "test_gateway_process.py",
        "test_memory.py",
        "test_model_factory.py",
        "test_openai_compatible_model.py",
        "test_permissions.py",
        "test_permissions_store.py",
        "test_session.py",
        "test_skills.py",
        "test_terminal_qr.py",
        "test_tools.py",
        "test_transcript.py",
    ];
    let mapped = parity_coverage_map();
    for file in expected {
        assert!(mapped.iter().any(|entry| entry.python_file == file), "{file}");
    }
}
```

Run: `cargo test --manifest-path colibri-rust/Cargo.toml python_test_coverage_map_covers_all_unit_files`
Expected: FAIL because the coverage map does not exist.

- [ ] **Step 2: Implement the coverage map and Python/Rust process runner.**

Implement `ParityEntry { python_file, rust_tests, status }`, `parity_coverage_map()`, `run_python_cli()`, `run_rust_cli()`, and deterministic tests for `ask`, `diagnostics`, and `gateway` usage.

- [ ] **Step 3: Add Rust-native parity tests for Python-only areas that are not pure CLI output.**

Add tests for config user-default loading, permission store formatting, permission hard-deny precedence over shell redirection, memory disabled behavior, transcript scoped metadata, gateway missing-state formatting, and model factory error behavior.

- [ ] **Step 4: Run Python and Rust full suites.**

```bash
uv run python -m pytest
cargo test --manifest-path colibri-rust/Cargo.toml
cargo build --release --manifest-path colibri-rust/Cargo.toml
```

Expected: all commands exit 0. If a Python/Rust output comparison fails, either fix Rust behavior or record a documented intentional difference with a narrow test.

## Self-Review

- Spec coverage: The tasks cover the design document's crate, CLI, config, model, tools, memory, skills, transcript, gateway, Weixin, web search, build, and README requirements.
- Placeholder scan: This plan contains no open TODO/TBD implementation gaps.
- Type consistency: The named interfaces are consistent across tasks and point to Rust modules in the new crate.

### Task 5: Close Tool, Config, Transcript, And Process Parity Gaps

**Files:**
- Modify: `colibri-rust/src/config.rs`
- Modify: `colibri-rust/src/messages.rs`
- Modify: `colibri-rust/src/model.rs`
- Modify: `colibri-rust/src/tools.rs`
- Modify: `colibri-rust/src/memory.rs`
- Modify: `colibri-rust/src/transcript.rs`
- Modify: `colibri-rust/src/gateway.rs`
- Modify: `colibri-rust/tests/runtime.rs`
- Modify: `colibri-rust/tests/parity.rs`

**Interfaces:**
- Tool-call arguments preserve `serde_json::Value` types through model parsing,
  permission classification, execution, and transcript output.
- Transcript writer accepts object payloads and applies session retention
  settings.
- Child-process execution returns on configured timeout after killing and
  reaping the child.

- [x] **Step 1: Add failing tests copied from the Python config, tools, memory,
  transcript, Web, gateway-process, and model cases.**
- [x] **Step 2: Run each focused Rust test and confirm it fails for the audited
  behavior gap.**
- [x] **Step 3: Implement exact schemas, structured arguments, safe memory
  routing, real process timeout, transcript retention, Web validation/proxy
  routing, and verified gateway PID handling.**
- [x] **Step 4: Run the focused tests and the complete Rust suite.**

### Task 6: Implement Complete Weixin Channel Parity

**Files:**
- Modify: `colibri-rust/Cargo.toml`
- Modify: `colibri-rust/src/weixin.rs`
- Modify: `colibri-rust/src/gateway.rs`
- Modify: `colibri-rust/src/cli.rs`
- Modify: `colibri-rust/tests/runtime.rs`
- Modify: `colibri-rust/tests/parity.rs`

**Interfaces:**
- Inbound Weixin messages carry `Vec<MediaPart>` and message identifiers.
- Gateway sessions receive a Weixin permission prompter and media sender.
- Weixin media helpers implement bounded AES-ECB/PKCS7 upload and download.

- [x] **Step 1: Port the Python channel tests as failing Rust tests, including
  text/media separation, waiters, bounded dispatch, encryption, cleanup,
  gateway media delivery, and permission prompt mapping.**
- [x] **Step 2: Run focused tests and confirm the missing behavior failures.**
- [x] **Step 3: Add focused crypto support and implement channel/API behavior
  without adding a general async runtime.**
- [x] **Step 4: Run all Weixin, gateway, and session tests.**

Progress note 2026-07-10: Weixin structured update parsing, inbound media
download/decrypt/store, outbound media encrypt/upload/send, cleanup, and
permission reply alias behavior are covered by Rust tests and pass.

Implementation note 2026-07-10: Rust foreground gateway must mirror Python's
channel runner shape with a bounded work queue, a receive loop that keeps
polling while the worker handles sessions, per-sender text waiters for Weixin
permission replies, media sender injection, and `submit` calls that pass inbound
`MediaPart` values into the session.

Completion note 2026-07-10: Rust foreground gateway now uses a bounded Weixin
work queue, keeps the receive loop active while the worker owns session
handling, routes same-sender text replies to permission waiters, injects a
Weixin media sender into sessions, and submits inbound media to the session.

Audit note 2026-07-10: The screenshot requirement also requires test parity at
the Python test-function level, not just the Python test-file level. The parity
test must scan every `tests/unit/test_*.py::test_*` function, require an
explicit mapping entry for it, and require each mapped Rust `#[test]` function to
exist. File-level coverage is not sufficient.

Audit note 2026-07-10: The screenshot's transcript-retention item needs direct
Rust coverage for deleting expired inactive transcript files, preserving the
active file, and deleting oldest inactive files to satisfy total-size limits.
Session-level budget tests alone are not enough.

### Task 7: Enforce Per-Test Parity And Release Verification

**Files:**
- Modify: `colibri-rust/tests/parity.rs`
- Modify: `colibri-rust/tests/runtime.rs`
- Modify: `README.md`
- Modify: `README.zh-CN.md`

**Interfaces:**
- A machine-checked map contains every Python `test_*` function and at least one
  Rust test for it; only `covered` is accepted.

- [x] **Step 1: Generate the expected Python test-function inventory during the
  parity test and make unmapped cases fail.**
- [x] **Step 2: Add missing Rust-native or cross-runtime cases until the map has
  no partial entries.**
- [x] **Step 3: Run `uv run python -m pytest` and full `cargo test`.**
- [x] **Step 4: Build release, run isolated-config CLI smoke tests, compare
  deterministic output, measure process RSS, and run `git diff --check`.**
- [x] **Step 5: Update both READMEs with only verified parity claims and exact
  commands.**

### Task 8: Remove Runtime Curl Dependency

**Files:**
- Modify: `colibri-rust/Cargo.toml`
- Modify: `colibri-rust/src/http.rs`
- Modify: `colibri-rust/src/model.rs`
- Modify: `colibri-rust/src/tools.rs`
- Modify: `colibri-rust/src/weixin.rs`
- Modify: `colibri-rust/tests/runtime.rs`
- Modify: `README.md`
- Modify: `README.zh-CN.md`

**Interfaces:**
- HTTP-backed Rust features use a blocking Rust HTTP client with no async
  runtime and no system `curl` executable.
- Tests capture HTTP requests through local TCP servers, not fake external
  executables.
- JSON, binary download, binary upload, timeout, status, body, and response
  headers remain available to OpenAI-compatible model calls, Web search, and
  Weixin.

- [x] **Step 1: Add tests that exercise OpenAI-compatible, Web search, and
  Weixin HTTP paths through local HTTP servers.**
- [x] **Step 2: Replace `curl` process execution with a Rust-native blocking
  HTTP client.**
- [x] **Step 3: Remove fake-executable test helpers and update request-capture tests to
  local TCP servers.**
- [x] **Step 4: Run Python full suite, Rust full suite, release build, and scan
  Rust source/docs for runtime `curl` references.**
