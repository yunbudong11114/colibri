# Colibri Context Compacting Design

Date: 2026-07-07
Status: Approved by roadmap; enhanced on 2026-07-07
Milestone: 6
Scope: Model-assisted context compacting, deterministic fallback, and bounded model input

## 1. Goal

Milestone 6 makes Colibri keep useful continuity while bounding in-memory conversation history and model input size.

After this milestone, Colibri should:

- compact messages that fall out of the recent-message window into a rolling summary,
- use the configured model to create Claude Code style continuation summaries when enabled,
- inject the rolling summary into model input as temporary context,
- keep the summary bounded by `session.summary_max_chars`,
- cap model input messages by `session.compact_trigger_chars`,
- replace old tool results in summaries with compact metadata,
- record compact events in transcript logs.

The first implementation was deterministic and offline-safe. The enhanced implementation keeps that deterministic path as the fallback, but prefers model-assisted summaries when the session config enables them.

## 2. Headless Requirement

Context compacting must work on pure Linux servers over SSH.

Rules:

- Use only Python standard library APIs.
- Do not require network access for fallback compacting.
- Do not require GUI, browser, audio, display, notification, or TUI frameworks.
- Do not keep full transcripts in memory.
- If model-assisted compacting cannot call the configured model, use deterministic compacting and continue the session.

## 3. Summary Strategy

`AgentSession.summary` becomes the rolling compacted context for older messages.

Preferred compacting path:

1. Build a compact request from the previous rolling summary and the dropped messages.
2. Call the configured model with no tools and a compact-specific system prompt.
3. Ask the model for plain text in this shape:

```text
<analysis>
scratchpad used only to improve the summary
</analysis>

<summary>
1. Primary Request and Intent:
...
9. Optional Next Step:
...
</summary>
```

4. Strip the `<analysis>` block.
5. Replace the `<summary>` wrapper with a readable `Summary:` header.
6. Bound and append the result to `AgentSession.summary`.

The summary prompt should be close to Claude Code's compacting shape, but tuned for Colibri:

- preserve user requests and intent,
- preserve file paths, commands, tool names, memory changes, and device constraints,
- preserve errors, fixes, pending tasks, and current work,
- list user messages that affected direction,
- avoid tool calls and answer with text only.

The model-assisted compact call must not expose normal tool specs. It should use `tools=[]`, disabled tool choice by omission, `system="You are a helpful AI assistant tasked with summarizing conversations."`, and a bounded output budget.

Fallback compacting path:

When `AgentSession.messages` exceeds `session.recent_message_limit`, dropped messages are converted into short summary lines:

```text
user: asked where the router is
assistant: answered from memory
tool files.read ok: 120 chars
tool shell.run permission_denied: 16 chars
```

Rules:

- User and assistant text are trimmed to a per-message summary limit in fallback mode.
- Tool messages are summarized as metadata, not full output, in fallback mode.
- Tool metadata should include tool name when it can be found from the matching assistant tool call.
- The rolling summary is trimmed from the front to stay within `session.summary_max_chars`.
- Reset clears both recent messages and summary.
- Empty model summaries, model errors, or context budget errors trigger fallback compacting rather than failing the user turn.

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
  "mode": "model",
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

If model-assisted compacting fails, also write:

```json
{
  "error_type": "ModelError",
  "fallback": true
}
```

Event type:

```text
context_compact_error
```

## 7. Testing

Required tests:

- dropped old messages update `AgentSession.summary`,
- summary is bounded by `session.summary_max_chars`,
- summary is injected into model input without being persisted as a normal message,
- tool result summary uses metadata rather than full tool output,
- model input budget drops oldest temporary model messages while keeping the latest user message,
- transcript logs `context_compact`,
- transcript logs `context_compact_error` when model compacting falls back,
- model-assisted compacting strips `<analysis>` and keeps `<summary>` content,
- model-assisted compacting calls the model without tools,
- transcript logs `context_budget`,
- reset clears summary,
- all tests run with `uv run python -m pytest`.

## 8. Future Work

After this milestone:

- tool result budget per turn,
- safer shell permission/risk model,
- local skill loading,
- MCP bridge.
