# Colibri Model Network Resilience Design

Date: 2026-07-16
Status: Approved in conversation
Scope: Python and Rust model calls, sessions, REPL, gateway workers

## 1. Goal

Keep Colibri running when a network switch, DNS interruption, connection
failure, timeout, rate limit, or transient provider failure interrupts a model
request. A failed model turn must not terminate the REPL, gateway worker,
router, channel adapter, or session cache.

## 2. Architectural Boundary

Reliability policy is channel-neutral:

```text
Channel transport
    -> generic gateway/REPL entry
    -> AgentSession turn boundary
    -> retrying ModelClient wrapper
    -> concrete provider
```

Channels only receive inbound messages and send generic outbound text/media.
They do not import model errors, classify provider failures, read retry
configuration, schedule model retries, or decide whether a session survives.

## 3. Model Retry Policy

The model layer retries transient request failures before returning an error to
the session:

- default retry attempts after the initial call: `2`;
- default backoff delays: `500ms`, then `1000ms`;
- exponential formula: `base_backoff_ms * 2^retry_index`;
- no jitter, keeping Python/Rust tests and small-device behavior deterministic;
- retry the same provider, model, messages, tools, system prompt, and limits;
- do not implement provider or model fallback in this change.

Retryable failures:

- DNS and connection establishment errors;
- connection reset, broken pipe, and temporary network-unreachable errors;
- request timeout;
- HTTP `408`, `429`, and `5xx`.

Non-retryable failures:

- HTTP `400`, `401`, `403`, and other non-transient `4xx`;
- malformed or semantically invalid provider responses;
- configuration and credential errors detected before a request;
- model output validation errors after a successful response.

Concrete providers classify errors into a structured `ModelError` category.
The retry wrapper consumes that category. Callers do not inspect error strings.

## 4. Configuration

Add:

```toml
[model]
max_retries = 2
retry_backoff_ms = 500
```

Rules:

- `max_retries` is the number of retries after the initial request;
- `0` disables automatic retry;
- `retry_backoff_ms = 0` disables sleeping while preserving retry count;
- configuration is shared by Python and Rust;
- no channel-specific retry fields are added.

## 5. Session Failure Boundary

When model retries are exhausted, `AgentSession` converts the error into a
normal failed-turn response:

```text
模型暂时不可用，请检查网络后重试。
```

The response is appended as an assistant message and returned through the
normal `AgentResponse` path. The transcript still records a `model_error`
event containing the error type and bounded diagnostic message.

The failed user message remains in session history. The session resets
turn-active and steering state through its existing guard/finally path and can
accept the next message.

Configuration/build errors raised before a user turn remain startup errors and
may terminate the command.

## 6. REPL Behavior

REPL code consumes `AgentResponse` exactly as it does for a successful turn.
It prints the failure text and returns to the prompt. It contains no model retry
or provider classification logic.

One-shot `ask` continues to return a non-zero exit status when the turn fails,
because there is no future prompt to recover through. It prints the same
bounded user-facing error.

## 7. Gateway Behavior

The generic gateway turn worker treats a failed model turn as a normal outbound
text response. It puts the session back into the cache, releases the router
key, and continues processing later messages.

Unexpected infrastructure failures outside the model-turn contract may still
stop a worker and surface to the gateway supervisor.

No channel adapter changes are required. Tests must prove channel modules do
not import model errors or retry configuration.

## 8. Error Representation

Python extends `ModelError` with a stable category while preserving readable
messages. Rust introduces an equivalent structured model error or typed
category at the model boundary and converts it to the existing public string
only where required by older CLI interfaces.

Required categories:

- `transient_network`;
- `timeout`;
- `rate_limit`;
- `server_error`;
- `client_error`;
- `invalid_response`;
- `configuration`.

Only the first four are retryable.

## 9. Observability

Each failed attempt may emit a concise status/transcript event:

```json
{
  "attempt": 1,
  "max_retries": 2,
  "error_type": "transient_network",
  "backoff_ms": 500
}
```

The final exhausted failure emits the existing `model_error` event. Logs must
not contain API keys, complete prompts, or full response bodies.

## 10. Testing

Python and Rust tests cover:

- transient failure followed by success;
- two retries followed by a failed-turn response;
- zero retries;
- exponential delay sequence;
- permanent client error without retry;
- invalid response without retry;
- REPL continues to read the next input after a failed turn;
- gateway processes a second message after a failed turn;
- session remains cached and usable;
- one-shot `ask` returns non-zero;
- channel source files contain no model-error or retry-policy dependencies;
- Python/Rust configuration and observable behavior remain aligned.

## 11. Acceptance Criteria

- Switching networks cannot terminate a running REPL or gateway solely because
  a model request failed.
- Retry behavior is bounded and configurable.
- Permanent errors are not retried.
- The same session can process a later message after failure.
- Channels remain transport-only and unchanged.
- Python and Rust full test suites pass.
- A new Rust release binary is built and copied to `~/.local/bin/colibri`.

## 12. Gateway Agent Health

`gateway status` reports process state and one coarse Agent health state:

```text
running=true
agent_status=healthy
```

The only allowed Agent states are:

- `healthy`: the Gateway has not observed a failed Agent turn since startup, or
  the most recent completed Agent turn succeeded;
- `unhealthy`: the most recent completed Agent turn failed because the model
  was unavailable, configuration reload failed, or another Agent-runtime error
  prevented a normal response.

Tool failures such as an unavailable `web.search` are normal tool results and
do not by themselves mark the Agent unhealthy. The model can consume that
result and continue the turn.

`running` remains strictly process-oriented. An unhealthy Agent does not make
the Gateway process appear stopped.

The background Gateway owns an in-memory, thread-safe health state. Turn
workers report success or failure to that state, but do not directly edit the
process state file. The health reporter persists `agent_status` only when the
value changes, serializes writes, and uses atomic replacement. This avoids
per-turn disk writes and same-temporary-file races when multiple turns finish
concurrently.

Status reads remain config-independent and do not probe the model or channels.

## 13. Runtime Configuration Reload

Python and Rust each provide one shared partial-runtime reloader used by both
Gateway and REPL. The reloader owns the active config path, filesystem
fingerprint, last-known-good snapshot, candidate parsing, whitelist merge, and
model construction. Gateway and REPL only call `reload_if_changed()` at a turn
boundary.

This avoids a permanent watcher thread on small devices while still making
changes effective on the next user action. One-shot `ask` loads configuration
once because it has no later turn.

When the fingerprint changes:

1. parse and validate the complete file;
2. construct the replacement model client and runtime dependencies;
3. atomically publish a last-known-good Agent runtime snapshot;
4. make each session adopt that snapshot at its next turn boundary while
   preserving messages and summary;
5. continue channel polling without restarting the Gateway process.

REPL performs the same reload before submitting the next non-empty prompt. It
replaces the model and runtime dependencies while preserving the current
conversation messages and summary.

If parsing, validation, or model construction fails, the Gateway/REPL rejects
the candidate, logs `config reload skipped`, and continues the current turn
with the last-known-good runtime. A later file change retries reload. An
invalid pending edit does not by itself mark the Agent unhealthy because the
active runtime may still be usable.

The default config path is `~/.colibri/config.toml`, including when the Gateway
was started without an explicit `--config`. File appearance, replacement,
content modification, and deletion all change the fingerprint.

The hot-reload whitelist contains only `model`, `vision`, and `web_search`.
All other sections are startup-scoped and require a process restart. Candidate
files are still completely parsed and validated, but only the three whitelisted
sections are copied into the active runtime snapshot.

This narrow boundary keeps session limits/history behavior, permissions, tool
availability, memory roots, Gateway worker/router topology, and channel
lifecycles stable while a process is running.

Runtime adoption must preserve the existing `PermissionPolicy` object and all
session-scoped grants. The three hot-reloaded sections do not change permission
rules, so they must never clear or reconstruct session authorization. A normal
next turn and the first turn after a successful reload must both retain
session-level command, executable, tool, file-root, and file-path grants.
