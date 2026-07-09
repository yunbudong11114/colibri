# Colibri Context Handoff

Date: 2026-07-01
Source thread: `cardputer-agent` build thread
Current workspace: `/Users/ybd/cardputer/colibri`

## Current Project Identity

- Project display name: Colibri.
- Python package: `colibri`.
- CLI command: `colibri`.
- Default user data directory: `~/.colibri`.
- Git remote: `https://github.com/yunbudong11114/colibri.git`.
- Local branch: `main`, tracking `origin/main`.

The project was originally created as `cardputer-agent`, then fully renamed to Colibri. Hardware references to CardputerZero are still intentional because the target device remains M5Stack CardputerZero / Raspberry Pi Compute Module 0 class Linux hardware.

## Existing Design Context

- Approved design: `docs/superpowers/specs/2026-07-01-colibri-design.md`.
- Completed Milestone 1 plan: `docs/superpowers/plans/2026-07-01-colibri-milestone-1.md`.
- Hardware reference: `docs/reference/cardputerzero-summary.md`.
- Local Claude Code, PicoClaw, and ZeroClaw research notes are private references and are intentionally not uploaded.

Important project rule from the active user instructions: before modifying code, update the design documentation first, complete the design change, and only then modify code.

Runtime requirement: Colibri must remain usable on headless Linux servers over plain SSH. Future milestones should preserve a CLI/stdin/stdout path and must not require GUI, browser, desktop, audio, or display dependencies in the core runtime.

## Implemented So Far

Milestone 1 is complete. The repository currently has:

- Python `src/` package layout.
- TOML config loader and default config dataclasses.
- Message and model abstractions.
- Deterministic fake model.
- `AgentSession.submit()` for one bounded model turn.
- CLI commands: `ask` and `repl`.
- Unit tests for config, session, and CLI.
- README with local development commands.

Last known verification from the source thread:

- Unit tests: `8 passed`.
- CLI smoke: `fake: hello colibri`.
- Git status after rename: clean on `main...origin/main`.
- Rename commit pushed: `5131b47 rename project to colibri`.

## Not Implemented Yet

The project is still a runnable skeleton, not a full tool-using agent. Missing pieces include:

- OpenAI-compatible model adapter.
- Bounded agent tool loop.
- Tool registry and tool schemas.
- Permission decision flow.
- Shell and file tools.
- Transcript JSONL logging.
- Memory files and compact summaries.
- Skill discovery and `SKILL.md` loading.
- MCP bridge/client.
- CardputerZero-specific UI, audio, GPIO, systemd, and idle behavior.

## Recommended Next Milestone

Milestone 2 should focus on the smallest useful real agent loop:

1. Update the design and add a Milestone 2 plan.
2. Add an OpenAI-compatible model adapter behind the existing `ModelClient` interface.
3. Add `ToolSpec`, registry, and a bounded tool loop.
4. Implement read-only shell/file tools first.
5. Add permission confirmation before write or high-risk operations.
6. Add JSONL transcript logging with bounded result sizes.
7. Keep dependencies minimal and preserve CardputerZero memory constraints.

## Reading Order

For a fast restart:

1. `README.md`
2. `docs/reference/2026-07-01-context-handoff.md`
3. `docs/superpowers/specs/2026-07-01-colibri-design.md`
4. `pyproject.toml`
5. `src/colibri/cli.py`
6. `src/colibri/config.py`
7. `src/colibri/session.py`
8. `src/colibri/messages.py`
9. `src/colibri/model/base.py`
10. `src/colibri/model/fake.py`
11. `tests/unit/`
