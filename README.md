# Colibri

Lightweight Python agent runtime for CardputerZero-class Linux devices.

## Runtime Support

Colibri must run on headless Linux servers over plain SSH. The core runtime and milestone work should stay usable through CLI/stdin/stdout only, without requiring a graphical desktop, browser, system tray, display server, audio device, or TUI framework.

## Current Milestone

Milestone 5 provides:

- Python package skeleton.
- TOML config loader with CardputerZero-friendly defaults.
- Message and model interfaces.
- Deterministic fake model for tests and smoke runs.
- `AgentSession.submit()` for a bounded single model turn.
- CLI `ask` and `repl` commands.
- OpenAI-compatible chat completions model adapter.
- Model provider factory and concise CLI error handling.
- Bounded agent tool loop.
- Read-only built-in tools: `files.list`, `files.read`, and allowlisted `shell.run`.
- Permission decisions before tool execution.
- Headless stdin/stdout confirmation for future non-read-only tools.
- Session-scoped "always allow" grants.
- Compact JSONL transcript logging.
- File-backed memory tools: `memory.list`, `memory.read`, `memory.search`, and `memory.write`.
- Automatic memory recall from `MEMORY.md` and relevant topic files.

## Development

```bash
uv run python -m pytest
uv run python -m colibri.cli ask "hello"
uv run python -m colibri.cli repl
```

The runtime is standard-library only. `pytest` is only needed for development tests.

## Model Providers

Colibri defaults to the deterministic fake model:

```bash
uv run python -m colibri.cli ask "hello"
```

To use an OpenAI-compatible chat completions API, copy `configs/openai.example.toml`, set `OPENAI_API_KEY` in the environment, and pass the config:

```bash
uv run python -m colibri.cli --config configs/openai.example.toml ask "say hi in five words"
```

For the Qunhe GLM endpoint, set `COLIBRI_GLM_API_KEY` and use `configs/glm.example.toml`:

```bash
export COLIBRI_GLM_API_KEY="..."
uv run python -m colibri.cli --config configs/glm.example.toml ask "用中文说一句你好"
```

The runtime does not read API keys from config files. It reads the environment variable named by `model.api_key_env`.

## Built-In Tools

When the configured model returns tool calls, Colibri can execute a small built-in tool set:

- `files.list`: list direct children under configured `files.roots`.
- `files.read`: read UTF-8 text files under configured `files.roots`.
- `shell.run`: run allowlisted commands such as `ls`, `cat`, `sed`, `rg`, `python`, and `git status`.
- `memory.list`: list Markdown memory topics.
- `memory.read`: read a memory topic.
- `memory.search`: search the memory index and topic files by keyword.
- `memory.write`: append a Markdown bullet to a memory topic.

Tool calls are bounded by `session.max_tool_rounds`, and tool output is capped by `tools.max_result_chars`.

## Memory

Colibri stores persistent memory as plain Markdown under:

```text
~/.colibri/memory
```

The default layout is:

```text
memory/
  MEMORY.md
  topics/
    devices.md
    preferences.md
```

Memory tools use `memory.root` and `memory.max_search_results` from config. `memory.list`, `memory.read`, and `memory.search` are read-only. `memory.write` is not read-only, so the default permission policy asks before appending.

When `memory.enabled = true`, Colibri also reads `MEMORY.md`, scores topic names and descriptions against the current turn, and injects the top relevant topic files into the model input as a temporary context block. The injected memory is not stored in `AgentSession.messages`.

Recall is bounded by:

- `memory.max_recall_topics`
- `memory.max_recall_chars`

## Tool Permissions

Colibri checks permission before running each registered tool call.

The default `tools.default_permission = "allow_read_confirm_write"` allows read-only tools and asks for confirmation before non-read-only tools. Other supported values are:

- `allow`: allow all registered tool calls.
- `confirm`: confirm every registered tool call.
- `deny`: deny every registered tool call.

Confirmation works over stdin/stdout, so it is safe for SSH-only servers. A response of `always` allows the same tool name for the rest of the current session only.

## Transcripts

When `session.transcript = true`, the CLI writes compact JSONL events to:

```text
~/.colibri/transcripts/YYYY-MM-DD.jsonl
```

Set `COLIBRI_HOME` to change the base directory. Transcript events include user messages, assistant messages, tool calls, permission decisions, tool results, model errors, and tool round limits. API keys are not logged by the runtime.
