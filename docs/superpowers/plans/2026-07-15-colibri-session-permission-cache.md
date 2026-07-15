# Colibri Session Permission Cache Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make session grants persist for one `AgentSession` in both runtimes and make cached user-permission updates safe under concurrent writers.

**Architecture:** `UserPermissionStore` owns metadata-invalidated read caching and an OS-locked merge operation that accepts grant deltas. `PermissionPolicy` owns session grants for the full agent-session lifetime; Rust receives the temporary permission prompter at decision time instead of borrowing it in the policy.

**Tech Stack:** Python 3 standard library (`fcntl`, `tempfile`, `tomllib`), Rust 2021 standard library plus the existing `libc` dependency, pytest, Cargo tests.

## Global Constraints

- Python and Rust behavior must remain aligned.
- Support macOS and Linux without adding third-party dependencies.
- Only `[shell].executables` is supported; `[shell].prefixes` is ignored.
- Do not change numeric permission choices, permission scopes, or public configuration.
- Modify tests before production code and observe each targeted test fail.

---

### Task 1: Python Cached Permission Store

**Files:**
- Modify: `tests/unit/test_permissions_store.py`
- Modify: `src/colibri/permissions_store.py`

**Interfaces:**
- Produces: `UserPermissionStore.merge(delta: UserGrants) -> UserGrants`
- Produces: cached `load() -> UserGrants` invalidated by `(device, inode, mtime_ns, size)`.
- Preserves: `save(grants: UserGrants) -> None` as an atomic full replacement for direct callers.

- [ ] **Step 1: Write failing store tests**

Add tests that monkeypatch `tomllib.loads` to prove two unchanged loads parse once, atomically replace the file to prove cache invalidation, call `merge` from two stale store instances and assert both deltas survive, and assert a file containing only `[shell].prefixes` loads no executable grants.

- [ ] **Step 2: Verify the Python store tests fail**

Run: `uv run python -m pytest tests/unit/test_permissions_store.py -q`

Expected: failures because `merge` and cache invalidation do not exist and the legacy prefixes test still loads values.

- [ ] **Step 3: Implement cached reads and locked merge-save**

Add an internal immutable fingerprint, copied grant values, uncached parsing, same-directory temporary writes, and an exclusive `fcntl.flock` on `permissions.toml.lock`. In `merge`, acquire the lock, reload disk without using the stale cache, union all four grant sets, atomically replace the TOML file, refresh the cache, and return a copy of the merged grants. Parse only `commands`, `executables`, `names`, and `roots`.

- [ ] **Step 4: Verify the Python store tests pass**

Run: `uv run python -m pytest tests/unit/test_permissions_store.py -q`

Expected: all permission-store tests pass.

### Task 2: Python Policy Uses Grant Deltas

**Files:**
- Modify: `tests/unit/test_permissions.py`
- Modify: `src/colibri/tools/permissions.py`

**Interfaces:**
- Consumes: `UserPermissionStore.merge(delta)` from Task 1.
- Preserves: existing `PermissionPolicy.decide(...)` signature and Python session lifetime.

- [ ] **Step 1: Write a failing stale-policy merge test**

Create two policies before either persistent approval, approve different user-level grants through each policy, then assert `permissions.toml` contains both grants.

- [ ] **Step 2: Verify the policy test fails**

Run: `uv run python -m pytest tests/unit/test_permissions.py -q`

Expected: the second stale full-snapshot save loses the first policy's grant.

- [ ] **Step 3: Replace full-snapshot saves with grant deltas**

For choices `4` and `5`, construct a `UserGrants` containing only the newly approved command, executable, tool, or file root and call `user_store.merge`. Do not construct or save a full snapshot from `user_grants`.

- [ ] **Step 4: Verify Python permission tests pass**

Run: `uv run python -m pytest tests/unit/test_permissions.py tests/unit/test_permissions_store.py -q`

Expected: both modules pass.

### Task 3: Rust Cached Permission Store and Delta Merge

**Files:**
- Modify: `colibri-rust/tests/runtime.rs`
- Modify: `colibri-rust/src/permissions.rs`

**Interfaces:**
- Produces: `UserPermissionStore::merge(&mut self, delta: &UserGrants) -> Result<UserGrants, String>`.
- Produces: mutable cached `load(&mut self) -> UserGrants` invalidated by Unix metadata identity, nanosecond modification time, and size.
- Preserves: deterministic TOML output and existing `save` behavior for direct tests.

- [ ] **Step 1: Write failing Rust store tests**

Add runtime tests for ignoring `[shell].prefixes`, observing an atomic external replacement after a cached load, and retaining two different deltas merged from stale stores.

- [ ] **Step 2: Verify the Rust store tests fail**

Run: `cargo test --manifest-path colibri-rust/Cargo.toml --test runtime permission_store -- --nocapture`

Expected: failures because Rust has no cache-aware merge and still reads legacy prefixes.

- [ ] **Step 3: Implement Rust cache and locked merge**

Store cached grants and fingerprint in `UserPermissionStore`. Use the existing `libc` dependency for an exclusive `flock` on a sibling lock file. Under the lock, read current disk state directly, union the delta, write a unique same-directory temporary file, rename it over the destination, refresh the local cache, and unlock through an RAII guard. Remove `merged_string_lists_at` if it becomes unused.

- [ ] **Step 4: Use deltas in Rust policy persistence**

Make policy decisions call `user_store.merge` with only the approved value for choices `4` and `5`. Ensure a failed persistent write is not represented internally by a stale cached full snapshot.

- [ ] **Step 5: Verify Rust permission-store tests pass**

Run: `cargo test --manifest-path colibri-rust/Cargo.toml --test runtime permission_store -- --nocapture`

Expected: all targeted store tests pass.

### Task 4: Rust Policy Lifetime Matches AgentSession

**Files:**
- Modify: `colibri-rust/tests/runtime.rs`
- Modify: `colibri-rust/src/permissions.rs`
- Modify: `colibri-rust/src/session.rs`

**Interfaces:**
- Produces: owned `PermissionPolicy` stored by `AgentSession`.
- Changes: `PermissionPolicy::decide` receives an optional temporary `&mut dyn PermissionPrompter` argument.
- Preserves: `submit`, `submit_with_media`, REPL, and channel external APIs.

- [ ] **Step 1: Write a failing cross-submit session test**

Use a scripted model that requests the same permission-controlled tool in two separate `submit` calls. Approve the first call with a session scope and assert the second submit does not prompt again. Construct a second `AgentSession` and assert it does prompt.

- [ ] **Step 2: Verify the Rust session test fails**

Run: `cargo test --manifest-path colibri-rust/Cargo.toml --test runtime session_permission_grant_survives_multiple_submits -- --nocapture`

Expected: the second submit prompts because Rust currently rebuilds its policy.

- [ ] **Step 3: Move policy ownership into AgentSession**

Remove the borrowed prompter field and lifetime parameter from `PermissionPolicy`. Add an owned policy field initialized with the session config. Pass the current submit's prompter through tool-round execution into `decide`. Update direct policy callers and tests to provide the prompter argument explicitly.

- [ ] **Step 4: Verify targeted Rust permissions and session tests pass**

Run: `cargo test --manifest-path colibri-rust/Cargo.toml --test runtime permission -- --nocapture`

Expected: permission tests pass, including the cross-submit lifecycle test.

### Task 5: Parity and Full Verification

**Files:**
- Modify: `colibri-rust/tests/parity.rs` only if test names or coverage mapping require it.
- Modify: `README.md` only if it still claims `[shell].prefixes` compatibility.

**Interfaces:**
- Verifies all behavior produced by Tasks 1-4.

- [ ] **Step 1: Remove remaining compatibility references**

Search production code, tests, and current documentation for `prefixes`; remove only obsolete permission-file alias references while leaving unrelated terminology untouched.

- [ ] **Step 2: Run the full Python suite**

Run: `uv run python -m pytest -q`

Expected: all Python tests pass.

- [ ] **Step 3: Run all Rust tests**

Run: `cargo test --manifest-path colibri-rust/Cargo.toml`

Expected: library, parity, and runtime tests pass.

- [ ] **Step 4: Build the release binary**

Run: `cargo build --release --manifest-path colibri-rust/Cargo.toml`

Expected: successful release build with no warnings.

- [ ] **Step 5: Inspect the final diff**

Run: `git diff --check` and `git status --short`.

Expected: no whitespace errors and only planned files changed.
