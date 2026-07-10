# Colibri Shared Transcript Restore Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Restore bounded global conversation context from existing transcripts and bound transcript disk retention.

**Architecture:** Add one focused history loader, inject it into AgentSession from CLI and Gateway, and extend TranscriptWriter with opportunistic retention cleanup. Keep long-term memory and active gateway session behavior unchanged.

**Tech Stack:** Python 3.11 standard library and pytest.

## Global Constraints

- Update design documentation before production code.
- Add no third-party dependencies.
- Do not isolate restored history by entry point, channel, sender, or project.
- Read transcript tails within fixed byte, message, and character limits.
- Preserve complete user and final-assistant turns.

### Task 1: Transcript history loader

Files: create src/colibri/session_history.py and tests/unit/test_session_history.py.

- [ ] Add failing tests for event filtering, global source merging, tail scanning, attachment stripping, and whole-turn limits.
- [ ] Run focused tests and confirm the loader is absent.
- [ ] Implement bounded tail reads and completed-turn extraction.
- [ ] Run focused tests and require all to pass.

### Task 2: AgentSession lazy restore

Files: modify session.py, cli.py, gateway.py and their unit tests.

- [ ] Add failing tests proving restore occurs once before the first new user message.
- [ ] Add an optional loader callable to AgentSession and inject it from production entry points.
- [ ] Verify restored messages are not rewritten to transcript.

### Task 3: Transcript retention

Files: modify transcript.py and tests/unit/test_transcript.py.

- [ ] Add failing age, size, and active-file preservation tests.
- [ ] Implement non-fatal startup and throttled cleanup.
- [ ] Verify the complete transcript test module.

### Task 4: Configuration

Files: modify config.py, all example TOML files, test_config.py, and the active
user configuration at ~/.colibri/config.toml.

- [ ] Add tests for all six defaults and TOML overrides.
- [ ] Add dataclass fields and documented example values.
- [ ] Add the same values to the active user configuration without changing secrets.

### Task 5: Verification

- [ ] Run focused tests for history, session, CLI, gateway, transcript, and config.
- [ ] Run the full pytest suite.
- [ ] Run compileall and git diff checks.
