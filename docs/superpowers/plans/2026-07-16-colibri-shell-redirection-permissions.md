# Colibri Shell Redirection Permission Classification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent file-descriptor and `/dev/null` redirections from triggering file-directory permission prompts while preserving real file-write detection.

**Architecture:** Keep the existing permission subject and grant model intact. Add narrow target filtering inside the Python and Rust redirection parsers, with matching tests that assert the resulting permission subject.

**Tech Stack:** Python 3.12 with pytest; Rust with cargo test.

## Global Constraints

- Do not change permission choice numbers or prompt text.
- Do not change session or persisted grant behavior.
- `shell.deny` remains higher priority than all user approvals.
- Python and Rust behavior must remain identical.

---

### Task 1: Python Redirection Classification

**Files:**
- Modify: `tests/unit/test_permissions.py`
- Modify: `src/colibri/tools/permissions.py`

**Interfaces:**
- Consumes: `permission_subject_for(tool, arguments, context)`
- Produces: `_redirection_target(argv: list[str]) -> str | None` with descriptor and null-target filtering

- [ ] **Step 1: Add failing permission-subject tests**

Add parameterized cases asserting that `2>&1`, `1>&2`, `2>&-`, and
`2>/dev/null` produce `subject_kind == "shell"`. Keep assertions that
`> out.txt` and `2>errors.log` produce `subject_kind == "file_path"`.

- [ ] **Step 2: Run the targeted Python tests**

Run:

```bash
uv run python -m pytest tests/unit/test_permissions.py -q
```

Expected: descriptor/null cases fail because they are currently parsed as file paths.

- [ ] **Step 3: Filter non-file redirection targets**

Update `_redirection_target` so targets matching `&` followed by a descriptor
or `-` are ignored, and normalized `/dev/null` targets are ignored. Preserve
all existing real-path and `tee` behavior.

- [ ] **Step 4: Re-run the targeted Python tests**

Run:

```bash
uv run python -m pytest tests/unit/test_permissions.py -q
```

Expected: all permission tests pass.

### Task 2: Rust Redirection Classification

**Files:**
- Modify: `colibri-rust/tests/runtime.rs`
- Modify: `colibri-rust/src/permissions.rs`

**Interfaces:**
- Consumes: `PermissionPolicy::decide`
- Produces: `redirection_target(argv: &[String]) -> Option<String>` with the same filtering as Python

- [ ] **Step 1: Add failing permission-subject tests**

Add table-driven cases asserting shell classification for descriptor/null
redirections and file-path classification for real inline and split targets.

- [ ] **Step 2: Run the targeted Rust tests**

Run:

```bash
cargo test --manifest-path colibri-rust/Cargo.toml permission_policy_classifies_shell
```

Expected: descriptor/null cases fail because they are currently parsed as file paths.

- [ ] **Step 3: Filter non-file redirection targets**

Update `redirection_target` with behavior identical to Python. Do not change
prompt formatting or permission-choice parsing.

- [ ] **Step 4: Re-run the targeted Rust tests**

Run:

```bash
cargo test --manifest-path colibri-rust/Cargo.toml permission_policy_classifies_shell
```

Expected: targeted Rust tests pass.

### Task 3: Parity Verification

**Files:**
- Modify only if required by test mapping: `colibri-rust/tests/parity.rs`

**Interfaces:**
- Consumes: completed Python and Rust behavior
- Produces: verified runtime parity and release binary

- [ ] **Step 1: Run the full Python suite**

```bash
uv run python -m pytest -q
```

- [ ] **Step 2: Run the full Rust suite**

```bash
cargo test --manifest-path colibri-rust/Cargo.toml
```

- [ ] **Step 3: Build the Rust release binary**

```bash
cargo build --release --manifest-path colibri-rust/Cargo.toml
```

- [ ] **Step 4: Check the final diff**

```bash
git diff --check
git status --short
```
