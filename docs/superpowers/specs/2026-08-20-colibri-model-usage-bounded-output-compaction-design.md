# Colibri Model Usage, Bounded Process Output, and Compaction Design

Date: 2026-08-20

## Scope

This design implements three related runtime hardening changes in Python and Rust with matching behavior:

1. Bound `shell.run` and `skill.run` output while the child process is running.
2. Preserve model stop reason and usage, and reject tool calls from a length-truncated response.
3. Use real model usage when available to improve compaction, reserve output capacity, and recover once from a real context overflow.

No public configuration fields or defaults are added or removed.

## Goals

- Keep command-output memory proportional to `tools.max_result_chars`, independent of total child output.
- Continuously drain stdout and stderr so a child cannot block on a full pipe.
- Keep Python and Rust process-output text, truncation flags, errors, and timeout behavior aligned.
- Never execute a tool call from an assistant response whose stop reason is `length`.
- Record provider usage in structured transcript payloads.
- Prefer provider-reported context usage over full-history character estimates.
- Leave enough context capacity for the configured model output.
- Retry a request after a real context overflow at most once.
- Preserve assistant tool-call and tool-result groups during compaction.

## Non-goals

- Tokenizer integration.
- Streaming model responses.
- Persisting complete command output to temporary files.
- New compaction, output, or cache configuration fields.
- Parallel tool execution.
- Full Pi Durable Harness state or automatic replay of interrupted tools.

## Model response metadata

`ModelResponse` gains two optional pieces of metadata:

```text
stop_reason: provider-normalized string or absent
usage: ModelUsage or absent
```

`ModelUsage` contains:

```text
input_tokens
output_tokens
cache_read_tokens
cache_write_tokens
total_tokens
```

The OpenAI-compatible adapter maps common response fields as follows:

| Colibri | OpenAI-compatible source |
|---|---|
| `stop_reason` | `choices[0].finish_reason` |
| `input_tokens` | `usage.prompt_tokens - cached - cache_write`, saturated at zero |
| `output_tokens` | `usage.completion_tokens` |
| `cache_read_tokens` | `usage.prompt_tokens_details.cached_tokens` or equivalent known field |
| `cache_write_tokens` | known cache-write detail field, otherwise zero |
| `total_tokens` | `usage.total_tokens`, falling back to normalized component sum |

Unknown or malformed usage must not fail an otherwise valid model response. Missing usage remains absent.

## Truncated tool-call rule

When `stop_reason == "length"` and the response includes tool calls:

1. Append the assistant response to conversation history as usual.
2. Do not request permission and do not execute any call in that response.
3. Append one tool result for each call with:
   - `ok = false`
   - `error_type = "truncated_tool_call"`
   - text explaining that output hit the model token limit and the call must be reissued.
4. Continue the normal model loop so the model can emit complete calls.

A `length` response without tool calls returns its available text normally. Output-length exhaustion is not itself treated as an input-context overflow.

## Usage transcript

Every successful `assistant_message` transcript event adds:

```json
{
  "stop_reason": "stop",
  "usage": {
    "input_tokens": 100,
    "output_tokens": 20,
    "cache_read_tokens": 80,
    "cache_write_tokens": 0,
    "total_tokens": 200
  }
}
```

Absent values are represented by JSON `null` so Python and Rust event shapes remain equal.

The session retains only the last usable context-usage anchor in memory. Full usage history belongs in transcript, not in every retained `Message`.

## Context usage anchor

After a successful assistant response with non-zero total usage, the session records:

```text
reported total tokens
current persistent message count
stable request-prefix fingerprint
```

The fingerprint covers model identity, system prompt, compacted summary, memory context, skill catalog, and tool specifications. It deliberately excludes the per-request time suffix.

For the next request:

- If the fingerprint and message anchor still match, estimate context as reported total plus only messages appended after the anchor.
- If the prefix changed, the session was compacted/reset, or the anchor is invalid, estimate the complete request.
- The estimate includes message roles, content, tool-call ids/names/arguments, system prompt, and serialized tool specifications.

The request-only time suffix is a small conservative estimation error. A previous suffix remains included in reported usage while the current suffix is estimated only during a full fallback estimate.

## Compaction trigger

Message-count compaction remains unchanged:

```text
len(messages) >= session.trigger_message_limit
```

The token trigger changes from fixed 80% to an output-reserve boundary:

```text
reserve_tokens = min(model.max_output_tokens, model.input_context_tokens / 2)
input_threshold = model.input_context_tokens - reserve_tokens
estimated_context_tokens >= input_threshold
```

The half-window clamp gives invalid or unusually large output settings a usable input area without adding configuration validation. With the defaults, the threshold is `48000 - 16384 = 31616` tokens.

## Recent context retention

Compaction continues to preserve complete message groups. It never separates an assistant tool call from its contiguous tool results.

`session.recent_message_limit` remains the hard message-count ceiling. A derived token ceiling is added:

```text
recent_token_limit = max(1, input_threshold / 2)
```

Groups are selected newest-first until either ceiling would be exceeded. The latest group containing a user message is still retained even when it alone exceeds a ceiling.

This adds protection against a small number of unusually large messages without changing configuration format.

## Context overflow recovery

The OpenAI-compatible adapter classifies a 400/413 response as `context_overflow` when its error body contains a known context-length signal such as:

- `context_length_exceeded`
- `context length`
- `maximum context`
- `too many tokens`
- `input token` combined with an exceeded/limit indication

This category is not handled by transport retry.

At the session layer:

1. The first `context_overflow` for a model request forces compaction with reason `overflow`.
2. The same model step is rebuilt and retried once without consuming a tool round.
3. A second overflow follows the existing model-unavailable path.
4. No other model error triggers compaction recovery.

Threshold/message-count compaction records reason `threshold`; forced overflow compaction records reason `overflow`.

## Bounded process output

Python and Rust add an internal process runner used by both `shell.run` and `skill.run`.

The runner:

- Starts stdout and stderr readers before waiting for process exit.
- Reads binary chunks continuously.
- Tracks total bytes independently for stdout and stderr.
- Stores only a bounded head and rolling tail for each stream.
- Applies the final character limit after UTF-8 lossy decoding.
- Preserves logical output order as stdout followed by stderr.
- Kills the process group on Unix and the child process on other platforms when timed out.
- Joins output readers after exit or termination.

The raw capture ceiling is derived from the existing character limit:

```text
capture_bytes = max(64, tools.max_result_chars * 4)
```

This supports worst-case UTF-8 character width while remaining bounded.

When output exceeds the final character limit, the returned text contains:

- approximately one quarter from the beginning,
- a middle `...[truncated]...` marker,
- the remaining budget from the end.

This keeps command headers and final diagnostics. `ToolResult.truncated` is true whenever raw capture or final character bounding discarded content.

Timeout result text remains compatible with existing tools:

- `shell.run`: `Command timed out`
- `skill.run`: `Skill command timed out`

Partial timeout output is drained and discarded rather than returned, keeping Python/Rust behavior deterministic.

## Testing

Both implementations require tests for:

- parsing stop reason and all supported usage fields;
- missing/malformed usage fallback;
- `length` tool calls produce synthetic errors without permission or execution;
- normal tool calls still execute;
- usage appears in transcript;
- actual usage can trigger compaction when a byte estimate would not;
- output reserve boundary replaces the 80% boundary;
- recent groups obey count and token ceilings without splitting tool groups;
- context overflow compacts and retries exactly once;
- non-overflow errors do not compact;
- large stdout and stderr are bounded with matching head/tail output;
- high-output skill commands complete instead of blocking on full pipes;
- timeout still terminates the process tree;
- Python/Rust parity mapping includes every new behavior.

## Compatibility

- Existing configuration files remain valid.
- Providers that omit usage or finish reason continue to work through estimation and normal tool handling.
- Existing short command output is unchanged.
- Truncated command output changes from prefix-only to head-and-tail in both languages.
- The only deliberately blocked behavior is executing tool calls explicitly reported as length-truncated.
