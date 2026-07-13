# Input Context Token Compaction Design

## Goal

Use a model-level input context token setting to decide when the existing summary compaction path should run. Keep Python and Rust behavior aligned.

## Configuration

- Add `model.input_context_tokens`.
- Default `model.input_context_tokens = 48000`, matching the old 192000-byte default with the conservative estimate `1 token ~= 4 UTF-8 bytes`.
- For the current GLM-5.2 config, set `model.input_context_tokens = 1000000`.
- Trigger proactive compaction at `80%` of `model.input_context_tokens`.
- Remove `session.model_input_char_limit` and `model.input_byte_limit` from Python and Rust config structs, config examples, README snippets, diagnostics, tests, and project config files.
- Do not keep backward compatibility for `model_input_char_limit` or `input_byte_limit`; configs containing either field should be rejected by the existing unknown-field validation.
- Remove the old `model_input_chars` helper once no runtime path uses character-count budgeting.

## Runtime Behavior

Before each model request, Python and Rust should call the same compaction decision:

- If current session message count is at least `session.trigger_message_limit`, compact.
- If estimated model input tokens are at least `model.input_context_tokens * 0.8`, compact.
- Both triggers use the existing compaction strategy: summarize current buffered messages, keep recent message groups according to `recent_message_limit`, and emit `context_compact`.
- After compaction, messages are rebuilt and sent as-is. There is no old-message dropping pass.

If the runtime has no tokenizer, estimate tokens from the model messages by summing UTF-8 byte lengths of role and content, then rounding up `bytes / 4`. The estimate should count the messages that would be sent, including compacted summary, memory context, skill context, and current session messages. It should not truncate user input.

## Tests

- Defaults and diagnostics should report `input_context_tokens`, not `model_input_char_limit` or `input_byte_limit`.
- Loading config with `session.model_input_char_limit` should fail.
- Loading config with `model.input_byte_limit` should fail.
- A long conversation below the message-count trigger but above `80%` of `input_context_tokens` should compact before the model call.
- Python and Rust parity mappings should point to the new tests.
