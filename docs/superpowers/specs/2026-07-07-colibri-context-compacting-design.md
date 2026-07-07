# Colibri Context Compacting Design

Date: 2026-07-07
Status: Approved by roadmap
Milestone: 6
Scope: Deterministic context compacting and bounded model input

## 1. Goal

Milestone 6 makes Colibri keep useful continuity while bounding in-memory conversation history and model input size.

After this milestone, Colibri should:

- compact messages that fall out of the recent-message window into a rolling summary,
- inject the rolling summary into model input as temporary context,
- keep the summary bounded by `session.summary_max_chars`,
- cap model input messages by `session.compact_trigger_chars`,
- replace old tool results in summaries with compact metadata,
- record compact events in transcript logs.

This milestone intentionally does not add model-assisted summarization. The first implementation is deterministic and offline-safe.

## 2. Headless Requirement

Context compacting must work on pure Linux servers over SSH.

Rules:

- Use only Python standard library APIs.
- Do not require network access.
- Do not require GUI, browser, audio, display, notification, or TUI frameworks.
- Do not keep full transcripts in memory.

## 3. Summary Strategy

`AgentSession.summary` becomes the rolling compacted context for older messages.

When `AgentSession.messages` exceeds `session.recent_message_limit`, dropped messages are converted into short summary lines:

```text
user: asked where the router is
assistant: answered from memory
tool files.read ok: 120 chars
tool shell.run permission_denied: 16 chars
```

Rules:

- User and assistant text are trimmed to a per-message summary limit.
- Tool messages are summarized as metadata, not full output.
- Tool metadata should include tool name when it can be found from the matching assistant tool call.
- The rolling summary is trimmed from the front to stay within `session.summary_max_chars`.
- Reset clears both recent messages and summary.

## 4. Model Input Context

`AgentSession` should continue storing only durable conversation messages in `self.messages`.

Temporary model input should be built from:

1. compacted summary context, if present,
2. recalled memory context, if present,
3. current recent messages.

Summary context format:

```text
Compacted conversation summary:

user: ...
assistant: ...
```

The summary context is sent as a temporary `system` message and must not be appended to `self.messages`.

## 5. Input Character Budget

Before each model call, `AgentSession` should ensure temporary model input is bounded by `session.compact_trigger_chars`.

Budget rules:

- Keep temporary system context messages when possible.
- Drop the oldest recent messages until the estimated character count is within budget.
- Always keep at least the latest user message.
- If the latest user message alone exceeds budget, it has already been bounded by `_bound_text()`.
- Dropping messages for model input must not delete them from `self.messages`; durable trimming still happens only through the recent-message window.

The estimate can be character-based. Token-accurate accounting is future work.

## 6. Transcript Behavior

When messages are compacted into summary, write:

```json
{
  "dropped_messages": 2,
  "summary_chars": 320
}
```

Event type:

```text
context_compact
```

When model input is trimmed to fit `compact_trigger_chars`, write:

```json
{
  "dropped_model_messages": 3,
  "input_chars": 35000
}
```

Event type:

```text
context_budget
```

Do not write full compacted message content to transcript events.

## 7. Testing

Required tests:

- dropped old messages update `AgentSession.summary`,
- summary is bounded by `session.summary_max_chars`,
- summary is injected into model input without being persisted as a normal message,
- tool result summary uses metadata rather than full tool output,
- model input budget drops oldest temporary model messages while keeping the latest user message,
- transcript logs `context_compact`,
- transcript logs `context_budget`,
- reset clears summary,
- all tests run with `uv run python -m pytest`.

## 8. Future Work

After this milestone:

- model-assisted summary compact when network is available,
- tool result budget per turn,
- safer shell permission/risk model,
- local skill loading,
- MCP bridge.
