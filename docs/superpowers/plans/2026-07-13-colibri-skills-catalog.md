# Skills Catalog Redesign Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans or implement task-by-task. Steps use checkbox syntax.

**Goal:** Replace keyword full-text skill injection with bounded catalog + `skill.read`, single `skills.dir`, Python/Rust parity.

**Architecture:** `SkillIndex.catalog()` builds prompt text; `skill.read` loads bodies by name; config uses `dir` / `max_catalog` / `max_catalog_chars`.

**Tech Stack:** Python 3.11+, Rust, pytest, cargo test

**Spec:** `docs/superpowers/specs/2026-07-13-colibri-skills-catalog-design.md`

---

### Task 1: Python config

**Files:** `src/colibri/config.py`, `tests/unit/test_config.py`, `configs/agent.example.toml`

- [ ] Replace `dirs` → `dir`, `max_loaded` → `max_catalog`, add `max_catalog_chars`
- [ ] Reject `dirs` / `max_loaded` with clear `ConfigError`
- [ ] Update allowlist + tests + example toml

### Task 2: Python skills + tools

**Files:** `src/colibri/skills.py`, `src/colibri/tools/builtin/skills.py`, `src/colibri/tools/registry.py`, `tests/unit/test_skills.py`, `tests/unit/test_tools.py`

- [ ] Replace `context_for`/`select` hot path with `catalog`
- [ ] Add `SkillReadTool`; keep `SkillRunTool` using `config.skills.dir`
- [ ] Update unit tests (TDD)

### Task 3: Python session / diagnostics / console

**Files:** `src/colibri/session.py`, diagnostics, console, `tests/unit/test_session.py`

- [ ] Inject catalog; transcript `skill_catalog`

### Task 4: Rust mirror

**Files:** `colibri-rust/src/{config,skills,tools,session,cli}.rs`, `tests/runtime.rs`, `tests/parity.rs`

- [ ] Same behavior as Python

### Task 5: Docs + verify

- [ ] README.md, README.zh-CN.md, docs/README.md index
- [ ] `pytest` + `cargo test`
