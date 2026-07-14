# Colibri Rust Port Design

## Status

Partially superseded by `2026-07-13-input-context-token-compaction-design.md`: Rust no longer enforces `model_input_char_limit` by dropping old messages. It uses `model.input_context_tokens` to trigger the same compaction path as message-count compaction.

## Goal

Create a new Rust implementation under `colibri-rust/` that can be built with the locally installed Cargo toolchain and used directly as a lightweight Colibri binary. The Rust port must preserve the Python project's configuration schema, configuration file format, CLI commands, default fake model responses, bounded session loop, transcripts, built-in tools, gateway process management, and Weixin iLink auth/API plumbing.

## Scope

The Rust version targets behavior parity with the Python runtime. It may use small, common Rust crates when they remove correctness risk, but dependencies must stay low-memory and avoid large async runtimes unless a feature requires them:

- `ask <text>` prints the fake or OpenAI-compatible model response.
- `repl` keeps a multi-turn session and exits on `/quit`, `/exit`, EOF, or configured idle timeout.
- `diagnostics` prints environment and active config details.
- `gateway status` prints process-state information from `~/.colibri/run/gateway.json`, verifies whether the recorded PID is still alive, and reports stale state clearly.
- `gateway start|stop|restart` manage a background `colibri-rust gateway run` process and write/read the same state and log paths as the Python runtime.
- `gateway run` starts enabled channel workers; Weixin worker support includes long-polling, text parsing, allow-list filtering, and text replies.
- `auth weixin` starts the Tencent iLink QR auth flow, prints the QR payload, polls until confirmation/expiry/timeout, and writes token/base URL back to the active TOML config.
- Terminal QR rendering must match Python's terminal-block QR behavior for Weixin auth payloads, including returning no QR art for oversized payloads.
- Config loading reads `--config <path>`, otherwise `~/.colibri/config.toml`, and falls back to built-in defaults.
- Config schema, default values, table names, nested sections, and override behavior must match Python's current `AgentConfig`: `[model]`, `[vision]`, `[session]`, `[tools]`, `[shell]`, `[files]`, `[skills]`, `[console]`, `[memory]`, `[web_search]`, `[gateway]`, and `[channels.weixin]`. Removed Python fields such as top-level `mcp`, `memory.max_recall_topics`, `shell.allow`, and `files.confirm_write` must not exist in Rust defaults or user-facing diagnostics.
- Local tools include `files.list`, `files.read`, `files.write`, `files.send`, `shell.run`, `memory.list`, `memory.read`, `memory.search`, `memory.write`, `skill.run`, `web.search`, and `image.understand`. `shell.run` matches Python by validating shell quoting, checking each unquoted compound command segment against `shell.deny`, and then executing the original command through the platform shell so pipes, redirection, and shell operators keep their normal semantics.
- `files.send` mirrors Python media behavior: it is a non-read-only file-path tool, requires an active channel media sender, resolves and validates allowed file paths, infers MIME type and `MediaPart.type`, preserves captions, returns `media_unavailable`/`not_file`/`permission_denied` with the same semantics, and writes `Sent file to channel: <filename>` into the tool result passed back to the model.
- `image.understand` mirrors Python vision behavior: it is enabled by default through the `image` tool category, uses the current workspace/configured file roots/dynamic file-root grants, rejects non-image files before model calls, enforces `vision.max_image_bytes`, builds `data:<mime>;base64,...` image URLs, calls the configured vision model or falls back to the main model when `[vision]` is empty, and serializes OpenAI-compatible multimodal requests.
- Permission handling mirrors Python's `PermissionPolicy`: read-only defaults, confirm mode, allow/deny defaults, hard-denied shell executables, session grants, executable-session shell grants, project grants in `.colibri/permissions.toml`, file-root grants for file-path subjects, `~` path expansion, and simple `shell.run` write-target classification for redirection or `tee` commands.
- Interactive permission confirmation mirrors Python's console prompter choices: shell prompts support once/session/executable-session/project/deny; file-path prompts support once/session-dir/project-dir/deny; normal tool prompts support once/session/project/deny.
- REPL terminal behavior mirrors Python's user-visible behavior for prompt handling, `/quit` and `/exit`, EOF, idle timeout, Unicode plain-stream input, and history/raw-tty behavior where the Rust standard library and terminal support allow it.
- `fake` model is deterministic and supports scripted tool calls for local smoke testing through a `tool:<name> <json>` prompt convention.
- Context compaction mirrors Python behavior: when `trigger_message_limit` is reached or estimated model input reaches 80% of `model.input_context_tokens`, Rust retains the latest user message even when outside the recent window, supports model-assisted compaction for non-fake providers, falls back to local summarized messages when model compaction fails or is disabled, and injects summary context as `Compacted conversation summary:`.
- Skill handling mirrors Python's `SkillIndex`: Rust includes the built-in `create-colibri-skill`, scans local skill directories without storing user skill bodies, derives descriptions from `skill.toml` or `SKILL.md`, parses command metadata including `description`, `args`, and `read_only`, scores skills from name/description terms, selects the built-in creation skill only for skill creation requests, loads selected contexts with `Base directory:`, and applies `max_loaded` plus `max_instruction_chars`.
- User media handling mirrors Python session behavior: inbound channel media is appended to the user message as an `Attachments saved locally:` block, transcript `user_message` payloads include structured media metadata, restored transcript history strips local attachment paths, and tool-result media is sent through the active channel before the tool response is exposed back to the model.
- Transcript handling mirrors Python's JSONL schema: each event contains `ts`, `type`, and object `payload`; `COLIBRI_HOME` controls the default transcript root; scoped gateway transcripts merge channel metadata such as `channel`, `sender_id`, and `session_key` into each payload without closing the base writer.
- Transcript restore mirrors Python's `TranscriptHistoryLoader`: new sessions optionally load completed final user/assistant turns from recent transcript tails, pair turns per source/session key, obey message/character/scan-byte limits, skip incomplete tool-call turns, and ignore malformed JSONL lines.
- Weixin media behavior mirrors Python where locally testable: inbound image/file items are parsed into separate messages with downloaded media parts, text and media messages from the same poll are dispatched separately, failed media downloads keep valid text messages, outbound media uploads before sending with the stored context token, captions are sent as text before media, and temporary inbound media cleanup enforces retention and byte-budget rules.
- Weixin gateway behavior mirrors Python's immediate-dispatch worker: channel messages are processed one by one instead of batching unrelated text/media from a poll, active permission prompts can wait for text replies from the same receive loop, and bounded work queues fail fast instead of deadlocking.
- `web.search`, Weixin, and `openai_compatible` must use a Rust-native synchronous HTTP client rather than the system `curl` executable. The client must stay low-memory, avoid a general async runtime, support HTTPS, request timeouts, JSON bodies, binary upload/download, and response-header access for Weixin CDN metadata.

## Non-Goals

- The Rust port does not replace the Python package entry point.
- MCP runtime startup is out of scope because the current Python config no longer exposes a top-level `mcp` field; Rust must follow that current Python surface rather than preserving earlier experimental MCP defaults.
- The Rust port does not try to match every internal Python class name or private helper.

## Architecture

The Rust crate is a binary-plus-library project:

- `src/main.rs` wires process exit to the CLI.
- `src/cli.rs` parses arguments with a small std-only parser and dispatches commands.
- `src/config.rs` stores defaults and uses `serde` plus `toml` to parse the same TOML syntax as Python's `tomllib`.
- `src/messages.rs` defines model messages, media parts, tool calls, tool results, and responses.
- `src/model.rs` provides `FakeModel` and `OpenAiCompatibleModel`, including multimodal `complete_image` request support.
- `src/session.rs` owns the bounded tool loop, memory/skill context injection, transcript events, and message compaction.
- `src/tools.rs` implements the local built-in tools.
- `src/permissions.rs` classifies tools, stores project grants, formats prompt subjects, and makes per-call permission decisions before execution.
- `src/http.rs` wraps the Rust-native synchronous HTTP client for JSON and binary HTTP calls and centralizes timeout/error handling.
- `src/vision.rs` implements image MIME validation, byte limits, data URL construction, and vision model selection.
- `src/session_history.rs` implements transcript history restore.
- `src/weixin.rs` implements iLink auth, polling, filtering, immediate dispatch helpers, inbound/outbound media payloads, AES helpers, and send-message payloads.
- `src/memory.rs` bootstraps and reads markdown-backed memory.
- `src/skills.rs` scans local skill directories and runs configured commands.
- `src/transcript.rs` writes JSONL events under `~/.colibri/transcripts`.
- `src/gateway.rs` provides gateway process management and foreground channel running.

The crate may use focused low-memory crates such as `serde`, `serde_json`, `toml`, and a small blocking HTTP client for exact parsing, serialization, and network I/O. It avoids heavyweight async/network stacks and must not require a system `curl` executable at runtime.

## Data Flow

For `ask` and each REPL turn:

1. CLI loads config.
2. CLI builds an `AgentSession`.
3. Session appends the user message and writes transcript events when enabled.
4. Session injects always-on memory and relevant local skill text into model input.
5. Model returns assistant text or tool calls.
6. Session executes permitted tools and feeds tool results back to the model.
7. If a tool result includes media, session sends it through the active channel sender and records any send failure as a tool error.
8. Session stops when the model returns text without tool calls or when `max_tool_rounds` is reached.

## Error Handling

User-facing errors return non-zero exit codes and short messages on stderr:

- Invalid CLI usage returns exit code `2`.
- Config, model, tool, auth, gateway, and network errors return exit code `1`.
- Disallowed file paths and denied shell executables return tool errors inside the session instead of panicking.
- Tool output and model input are bounded by config limits.

## Compatibility Notes

The Rust implementation keeps the same command names, config section names, config field names, default values, override behavior, and user-visible runtime behavior as the Python version. The Rust port must not keep functional gaps as intentional differences. MCP config parity is required; MCP runtime startup must be implemented when the Python runtime exposes a stable runtime process/config contract to mirror. HTTP-backed features must not require external `curl`; low memory is preserved by using a small blocking HTTP client rather than an async runtime.

## Testing

Rust tests are derived from the Python unit suite, not from ad hoc Rust-only expectations. The Rust suite keeps a parity map for every Python `tests/unit/test_*.py` file and uses two styles of verification:

- CLI parity tests run the Python CLI and Rust CLI with isolated `HOME`/config directories, normalize expected path/process differences, and compare exit code, stdout, and stderr for commands whose output is deterministic.
- Library parity tests mirror Python unit scenarios with Rust-native assertions when direct process-output comparison is not meaningful, such as permission policy state, tool result structs, memory bootstrapping, transcript JSONL, gateway state parsing, and HTTP payloads captured through local in-process TCP test servers.

Rust tests cover the locally verifiable behavior:

- Default config values, full TOML parsing, nested config overrides, and all current Python config fields including vision/session restore/media retention defaults, while excluding removed fields such as MCP.
- CLI `ask`, `diagnostics`, `gateway` usage behavior, including Python/Rust output parity for deterministic commands.
- Gateway state, start/stop formatting, and stale PID handling.
- Weixin auth, text, media upload/download, inbound media parsing, immediate dispatch, permission reply waiting, and API request construction using local HTTP test servers.
- Web search request construction and response formatting using local HTTP test servers.
- Fake model session responses and bounded tool rounds.
- File, shell, image, memory, and skill tools.
- Session media attachments, files.send media delivery, media send failure propagation, image understanding from received media paths, and transcript restore.
- Transcript JSONL creation.

The parity map records every Python test function and must move entries from partial coverage to covered coverage as Rust behavior is implemented. Functional gaps are not considered accepted differences. External network/API behavior is tested through local HTTP test servers, not live services or fake `curl`.

Verification commands:

```bash
uv run python -m pytest
cargo test --manifest-path colibri-rust/Cargo.toml
cargo build --release --manifest-path colibri-rust/Cargo.toml
./colibri-rust/target/release/colibri-rust ask "status"
./colibri-rust/target/release/colibri-rust diagnostics
```

## 2026-07-10 Full-Parity Closure

The Python implementation at `src/colibri/` and every assertion in
`tests/unit/` are the normative behavior specification. A Rust test-file map,
similar output, or a passing Rust-only test is not evidence of parity unless
the corresponding Python scenario and observable result are reproduced.

The remaining gaps are release blockers and must not be documented as
intentional Rust differences:

- Weixin must support the complete Python text/media path: inbound image and
  file parsing, AES-ECB/PKCS7 download decryption, bounded temporary storage,
  cleanup by age and byte budget, outbound encrypted upload, media send using
  the conversation context token, caption ordering, permission replies through
  the active receive loop, bounded immediate dispatch, and non-deadlocking
  worker failure propagation.
- `memory.read` and `memory.write` must accept exactly the Python `file` and
  `topic` forms, reject traversal and arbitrary filenames, use the same append
  and replace newline behavior, emit the same topic/index and short-file size
  guidance, and return matching error types and text.
- `shell.run` and skill commands must enforce `tools.max_shell_seconds` while
  the process is running. A timed-out child must be terminated and reaped; for
  shell execution Rust must terminate the process group so shell-spawned
  children do not survive timeout. A duration check after a blocking wait is
  not acceptable. Non-zero process exits use Python's `nonzero_exit` result.
- Transcript events must preserve JSON value types and the full Python payload
  schema. The writer uses the same daily filename, flush behavior, scoped
  metadata merge, retention-day cleanup, and total-byte cleanup. Parsed
  retention settings must affect runtime behavior.
- Tool schemas are compared as JSON values. Property types, required fields,
  enums, `additionalProperties`, descriptions, and tool order must match
  Python. Rust may not flatten model tool arguments into strings when Python
  preserves JSON booleans, numbers, arrays, and objects.
- Web search must match Python freshness validation and date expansion, Dumate
  proxy routing, headers, response validation, API-error mapping, JSON output,
  and timeout/error types.
- Gateway process management must verify that a recorded PID belongs to a
  Colibri gateway before sending a signal, expose RSS where Python does, close
  evicted sessions, and preserve Python status formatting and stale-state
  semantics.
- Config fields and defaults remain identical. Unknown sections stay ignored as
  Python documents, while unknown fields and wrong value types must produce the
  same success or failure behavior as Python rather than silently falling back.
- Memory bootstrap files and per-file truncation must use the exact Python
  content and limits so first-run prompts are identical.

Parity completion requires all Python test modules to be marked `covered`, and
each Python test function must have either a Rust-native equivalent named in a
machine-checked case map or a deterministic cross-runtime test. The map must
fail when a Python test is added without a Rust counterpart. `partial` is not a
passing status. Network services remain faked locally, but request method, URL,
headers, body bytes, ordering, and returned errors are observable behavior and
must be compared.

Low-memory constraints remain in force. Prefer focused crates for AES, process
timeouts, and serialization over a general async runtime. Media bytes may be
held only for the bounded encryption/decryption operation and must not enter
session message history.
