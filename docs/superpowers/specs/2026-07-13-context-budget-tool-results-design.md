# Context Budget and Tool Result Hygiene Design

## Status

Partially superseded by `2026-07-13-input-context-token-compaction-design.md`: the tool-result summarization and round-limit behavior remain, but `model_input_char_limit` has been removed and `model.input_context_tokens` now triggers proactive compaction.

## Goal

Keep Python and Rust behavior aligned while reducing context pressure from large tool outputs, making `context_budget` diagnostics actionable, and making continuation after `round_limit` honest and recoverable.

## Scope

This design covers these requested optimizations:

- Tool results in model context should be summarized when large, while transcripts keep the full bounded result.
- `files.read` should support optional line ranges and output caps.
- `context_budget` transcript events should include before/after sizes, largest message sources, and applied strategies.
- Repeated budget pressure should inject a model-visible warning to stop bulk file reads.
- `round_limit` should leave a continuation marker so a later "continue" turn knows the previous turn stopped early.

This change must be implemented in both Python and Rust with matching user-visible behavior and matching test coverage.

## Non-Goals

- Do not add background memory review.
- Do not change Colibri's configuration defaults.
- Do not switch `shell.run` back to `sh -c`.
- Do not remove full tool output from transcript logs.

## Design

### `files.read` Arguments

`files.read` keeps `path` required and accepts optional:

- `start_line`: 1-based inclusive line number.
- `end_line`: 1-based inclusive line number.
- `max_chars`: positive maximum characters for this read result, capped by `tools.max_result_chars`.

Invalid line ranges return `invalid_arguments`. If range arguments are omitted, the whole UTF-8 file is read as today and still bounded by `tools.max_result_chars`.

The returned text for ranged reads is the selected file content only. This avoids changing existing consumers that expect file bytes, while the tool schema description tells the model to prefer ranged reads for large files.

### Model Context Tool Result Summary

Session execution writes `tool_result` transcript events exactly as today: bounded by `tools.max_result_chars`, preserving full available tool text for audit/restore.

The `Message(role="tool")` content appended to `session.messages` is now a model-context form:

- Errors remain `error_type: text`.
- Short successful tool outputs remain unchanged.
- Large successful tool outputs become a compact summary:
  - first line: `tool_result_summary: <tool> ok chars=<N> truncated=<true|false>`
  - if known: `path=<path>`
  - then `head:` and `tail:` snippets.

This is intentionally deterministic and does not call the model.

### Context Budget Diagnostics

`budget_model_messages` returns a diagnostic object instead of only dropped count. The diagnostic includes:

- `input_chars_before`
- `input_chars_after`
- `dropped_model_messages`
- `largest_messages`: up to three `{role, tool, chars}` entries, including tool name when available.
- `applied_strategies`: currently `drop_old_message_groups`; session may add `tool_result_summary` and `context_pressure_warning`.

Transcript `context_budget` events keep the existing `input_chars` field for compatibility and add the above fields.

### Context Pressure Warning

Each session tracks consecutive budget events within a turn. When the count reaches 3, a system message is added to the model context for that model call:

`Context budget is tight. Stop bulk-reading files. Use targeted files.read ranges, files.list, or answer with the evidence already gathered. Do not claim full coverage unless the needed files were actually inspected.`

The warning is not persisted into `session.messages` and is not written as a user/assistant message. The `context_budget` event records `context_pressure_warning` in `applied_strategies` when the warning is included.

### Round Limit Continuation

When `round_limit` is reached, the assistant message remains as today but adds one more instruction:

`If the user says "continue", continue from this stopped state with targeted reads and do not claim the previous task was fully completed.`

The round limit text is stored in `session.messages`. Therefore transcript restore and in-memory continuation already carry the marker into the next turn.

## Test Plan

Python:

- `test_files_read_reads_line_range_and_respects_max_chars`
- `test_tool_result_context_summarizes_large_success_but_transcript_keeps_text`
- `test_context_budget_event_records_before_after_largest_and_strategies`
- `test_context_pressure_warning_is_injected_after_repeated_budget_events`
- update round limit text assertion.

Rust:

- Add matching runtime tests for `files.read` range/max chars.
- Add matching session tests for large tool result summary, budget diagnostics, pressure warning, and round limit continuation marker.

Parity:

- Keep Python/Rust tool schemas aligned.
- Keep public error types aligned: invalid ranges use `invalid_arguments`.
