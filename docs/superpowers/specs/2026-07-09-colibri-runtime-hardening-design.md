# Colibri Runtime Hardening Design

## Goal

Tighten the current runtime behavior without changing Colibri's public model, tool, permission, or channel concepts.

This follow-up focuses on issues found during the full code pass:

- safer gateway process lifecycle management,
- thread-safe gateway session cache,
- smaller and clearer CLI boundaries,
- cleaner Chinese request serialization,
- fixed system prompt spacing,
- clearer QR renderer behavior.

## Requirements

1. Change documentation before code.
2. Do not add third-party runtime dependencies.
3. Keep the existing CLI command names and config keys.
4. Keep CardputerZero and headless Linux compatibility.
5. Keep all runtime state under `~/.colibri` unless an explicit config path is supplied.
6. Preserve the raw Weixin QR payload fallback even when terminal QR rendering succeeds.

## Gateway Process Management

`colibri gateway status` and `colibri gateway stop` are process-management commands. They should remain usable even if the user config has a broken model key, broken TOML value in an unrelated section, or an unavailable channel configuration.

The CLI should parse arguments first. For gateway process actions:

- `status` and `stop` should call `GatewayProcessManager` before loading `AgentConfig`.
- `start` and `restart` may still pass the active config path to the background runner.
- `run` must still load config because it starts the actual gateway.

The state file should continue to live at:

```text
~/.colibri/run/gateway.json
```

The state file already stores `pid`, `cwd`, `command`, `config`, `log`, and `started_at`. Stop should use that state to verify that the live PID still looks like the gateway process before sending termination signals.

The process identity check should be conservative:

- if no PID is recorded, the process is not running,
- if the PID is gone, the process is not running,
- if command inspection is unavailable, status may report running but stop must avoid forcefully killing an unverified process,
- if command inspection is available, the command must contain `colibri.cli`, `gateway`, and `run`.

This protects long-running small servers from PID reuse mistakes.

## Gateway Session Cache

Gateway currently starts one thread per enabled channel. Future channels can make session access concurrent, so `GatewaySessionCache` must protect its internal `_entries` dictionary.

The cache should add a single `threading.Lock` and use it around:

- `get`,
- `touch`,
- `close`,
- idle eviction,
- oldest-session eviction.

The lock should not be held while calling `AgentSession.submit()`. Gateway should acquire the session, release the lock, run the turn, then touch the session.

## CLI and REPL Boundary

`cli.py` should remain responsible for command parsing, config selection, status writing, and command dispatch.

TTY line editing should move into a focused module:

```text
src/colibri/repl_input.py
```

That module owns:

- `read_repl_line`,
- `ReplLineEditor`,
- raw TTY byte reading,
- escape-sequence handling,
- raw TTY newline writing.

Existing tests should import these functions from the new module. `cli.py` should import only `read_repl_line`.

This keeps future CJK input, history, and terminal redraw fixes localized.

## Prompt and Model Payload

The system prompt string should preserve normal sentence spacing.

The OpenAI-compatible adapter should serialize request JSON with `ensure_ascii=False`. This avoids expanding Chinese text into escaped Unicode sequences and keeps request bodies smaller without changing the API shape.

## Config Source Documentation

`src/colibri/config.py` is the source of truth for supported configuration keys. Each config dataclass field should have a short Chinese comment explaining what the value controls. Example TOML files may keep longer usage-oriented comments, but the Python config definitions should remain readable without opening external documentation.

## Terminal QR Behavior

The built-in QR renderer stays dependency-free and best-effort. It targets the short iLink auth payloads used by Weixin login. The auth command must always print both:

- the terminal QR when rendering succeeds,
- the raw payload URL fallback.

Tests should keep checking these two user-visible behaviors. Deep QR decode validation can remain out of scope because adding a scanner dependency conflicts with the zero-dependency runtime goal.

## Tests

Add or update tests for:

- gateway `status` and `stop` do not load config,
- gateway stop refuses to kill an unverified PID,
- gateway stop kills a verified gateway PID,
- gateway session cache operations remain correct with locking,
- `cli.py` uses the split REPL input module,
- system prompt includes proper sentence spacing,
- OpenAI-compatible payload serialization keeps Chinese text unescaped,
- Weixin auth output includes both terminal QR and raw payload.
