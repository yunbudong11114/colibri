# Colibri Console Display Design (CM0-first)

Date: 2026-07-10  
Status: Approved  
Scope: Local `ask` / `repl` console output for Python and Rust (aligned)

## 1. Goal

Make Colibri's local console readable on CardputerZero / CM0-class devices
(1.9" screen + SSH), while keeping headless safety and Python/Rust parity.

After this change:

- status, permission, and answer layers are visually distinct,
- Rust emits the same `[colibri]` tool/skill/compact status lines as Python,
- assistant answers can be lightly de-noised for small screens,
- behavior is config-gated and rollback-friendly.

## 2. Non-Goals

- No rich / full-screen TUI dependency
- No mandatory ANSI colors
- No change to Weixin / gateway outbound message formatting
- No change to model-visible prompts or tool results
- No truncation of assistant answer body by default

## 3. Display Layers

| Layer | Stream | Content |
|-------|--------|---------|
| Status | stderr | One-line `[colibri] ...` events |
| Permission | stdout | Existing `shell:` / `file:` / `tool:` + `[y]/[n]` prompts |
| Answer | stdout | Final assistant text (optionally plain) |

## 4. Status Events (B1)

When `console.status = true`, both runtimes emit:

```text
[colibri] ready model=...
[colibri] thinking
[colibri] memory files=MEMORY.md,USER.md
[colibri] skill skills=...
[colibri] tool shell.run wait_permission
[colibri] tool shell.run ok chars=123
[colibri] compact mode=fallback removed=2 summary_chars=...
[colibri] model_error type=...
[colibri] idle_exit seconds=...
```

Rules:

- Prefix every status line with `[colibri]`.
- One plain line per event; no colors or cursor control in status lines.
- Do not print API keys, full prompts, full tool bodies, or full model messages.
- Python keeps `StatusTranscript`; Rust adds an equivalent wrapper around transcript writes used by CLI `ask`/`repl`.

## 5. Plain Answer Mode (B2)

Config:

```toml
[console]
status = true
plain_answer = true
```

Defaults:

- `status = true` (unchanged)
- `plain_answer = true` (new; CM0-friendly default)

When `plain_answer = true`, before printing the final assistant text:

1. Strip common markdown decoration: `**`, `__`, inline backticks, leading `#` heading markers.
2. Convert markdown tables into plain multi-line text (no `|` column layout).
3. Surround the answer with a blank line before and after.
4. Do not truncate the answer body.

When `plain_answer = false`, print the raw assistant text (desktop/debug).

Plain formatting applies only to local CLI answer printing. It must not mutate
session history, transcripts, or gateway channel payloads.

## 6. Python / Rust Parity

| Behavior | Python | Rust |
|----------|--------|------|
| StatusTranscript-style tool/skill/compact lines | existing | add |
| `console.plain_answer` config | add | add |
| Answer plain formatter | add | add (same rules) |
| Permission prompt text | keep | keep |

## 7. Rollback

- Set `plain_answer = false` to restore raw markdown answers.
- Set `status = false` to silence status lines.
- Code changes are isolated to console/cli/config and can be reverted as a unit.

## 8. Tests

- Config default and override for `plain_answer`.
- StatusTranscript / Rust wrapper emits tool wait/ok and skill/compact lines.
- Plain formatter strips bold/backticks/headings and flattens a simple table.
- `ask`/`repl` path uses plain formatter when enabled and leaves raw mode when disabled.
