# Remove Model Input Character Limit Design

## Status

Superseded by `2026-07-13-input-context-token-compaction-design.md`: `session.model_input_char_limit` is no longer kept for compatibility. It is removed from configuration, and `model.input_context_tokens` drives proactive compaction.

## Goal

Stop using `session.model_input_char_limit` as a runtime context budget. Colibri should no longer truncate user or steering messages with this value, drop old message groups before model calls, emit `context_budget` events, or inject context pressure warnings based on model input character count.

## Compatibility

- Remove the `session.model_input_char_limit` configuration key. Existing configs containing it should fail validation.
- Replace diagnostics display with `model.input_context_tokens`.
- Keep `session.summary_max_chars` unchanged; it still limits compacted summary length.
- Keep message-count compaction controlled by `trigger_message_limit`, `recent_message_limit`, `model_compact`, and `summary_max_chars`.

## Runtime Behavior

- Python and Rust sessions build model messages from summary, memory, skills, and current session messages without character-budget pruning.
- User input and steering input are stored exactly as received after media annotations are added.
- Large tool results are still summarized for model context by the tool-result summarization path; transcript logging still keeps the configured transcript text behavior.
- If the model provider rejects an oversized request, the error should surface through the existing model error path instead of silently dropping conversation history.

## Removed Behavior

- No `budget_model_messages_with_diagnostics` pass before model calls.
- No `drop_old_message_groups` strategy.
- No `context_budget` transcript event.
- No repeated-budget `CONTEXT_PRESSURE_WARNING` injection.
- No `_budget_pressure_events` / `budget_pressure_events` state.

## Tests

- Replace budget-drop tests with tests that verify oversized input is preserved and no `context_budget` event is written.
- Keep message-count compaction tests unchanged.
- Keep Python and Rust defaults aligned at their existing values.
- Run targeted session tests plus full Python unit and Rust test suites.
