# Colibri Milestone 3 Implementation Plan

Date: 2026-07-06
Spec: docs/superpowers/specs/2026-07-06-colibri-permissions-transcript-design.md

## Goal

Implement Milestone 3 as one complete milestone: permission decisions for tool calls plus compact JSONL transcript logging, while preserving pure headless server operation.

## Constraints

- Update design documentation before code changes.
- Keep runtime usable over CLI/stdin/stdout without GUI, browser, audio, display, notification, or TUI dependencies.
- Do not add write, network, memory, skills, MCP, or GPIO tools in this milestone.
- Keep tests deterministic and non-interactive by injecting fake prompters and transcript writers.

## Steps

1. Add permission tests and implementation.
   - Create `src/colibri/tools/permissions.py`.
   - Cover allow, deny, confirm, read-only defaults, and session-scoped always allow.

2. Add tool registry lookup support.
   - Add `ToolRegistry.get(name)`.
   - Keep unknown tool behavior intact.

3. Add transcript writer tests and implementation.
   - Create `src/colibri/transcript.py`.
   - Write append-only JSONL events with UTC timestamps.
   - Provide the documented default path.

4. Wire permissions and transcripts into `AgentSession`.
   - Resolve tools before execution.
   - Log user, assistant, tool call, permission, tool result, model error, and round-limit events.
   - Return denied tool results without executing blocked tools.
   - Close transcript writers from `AgentSession.close()`.

5. Wire CLI transcript creation.
   - Create the default transcript writer only when `config.session.transcript` is enabled.
   - Preserve fake-model CLI smoke behavior.

6. Update user-facing docs.
   - Document permission defaults.
   - Document transcript location.
   - Reaffirm headless server compatibility.

7. Verify and finish.
   - Run focused tests while developing.
   - Run `uv run python -m pytest`.
   - Run a CLI smoke test.
   - Commit and push the completed milestone.
