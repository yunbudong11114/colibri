# Colibri

Lightweight Python agent runtime for CardputerZero-class Linux devices.

## Runtime Support

Colibri must run on headless Linux servers over plain SSH. The core runtime and milestone work should stay usable through CLI/stdin/stdout only, without requiring a graphical desktop, browser, system tray, display server, audio device, or TUI framework.

## Current Milestone

Current implementation provides:

- Python package skeleton.
- TOML config loader with CardputerZero-friendly defaults.
- Message and model interfaces.
- Deterministic fake model for tests and smoke runs.
- `AgentSession.submit()` for a bounded single model turn.
- CLI `ask` and `repl` commands.
- OpenAI-compatible chat completions model adapter.
- Model provider factory and concise CLI error handling.
- Bounded agent tool loop.
- Built-in tools: `files.list`, `files.read`, `shell.run`, memory tools, and `skill.run`.
- Dynamic permission decisions before tool execution.
- Headless stdin/stdout confirmation for ungranted tools and shell commands.
- Session-scoped and project-scoped permission grants.
- Compact JSONL transcript logging.
- File-backed memory tools: `memory.list`, `memory.read`, `memory.search`, and `memory.write`.
- Automatic memory recall from `MEMORY.md` and relevant topic files.
- Model-assisted rolling summary compacting for messages outside the recent-message window.
- Deterministic compacting fallback for fake/offline model runs.
- Character-budgeted model input using `session.compact_trigger_chars`.
- Local filesystem skills with progressive disclosure.
- `skill.run` for configured local skill commands.
- SSH/serial-friendly console status lines.
- REPL idle timeout.
- Low-memory diagnostics command.
- Conservative systemd service example.

## Development

```bash
uv run python -m pytest
uv run python -m colibri.cli ask "hello"
uv run python -m colibri.cli repl
uv run python -m colibri.cli diagnostics
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

## Configuration

If `--config` is omitted, Colibri tries to read:

```text
~/.colibri/config.toml
```

If that file does not exist, Colibri uses built-in defaults. An explicit `--config` path always takes precedence over the default user config file.

## Built-In Tools

When the configured model returns tool calls, Colibri can execute a small built-in tool set:

- `files.list`: list direct children under the startup workspace, configured `files.roots`, or an approved external directory.
- `files.read`: read UTF-8 text files under the startup workspace, configured `files.roots`, or an approved external directory.
- `shell.run`: run shell commands after Colibri permission approval.
- `memory.list`: list Markdown memory topics.
- `memory.read`: read a memory topic.
- `memory.search`: search the memory index and topic files by keyword.
- `memory.write`: append a Markdown bullet to a memory topic.
- `skill.run`: run a configured command from a local skill.

Tool calls are bounded by `session.max_tool_rounds` (default `24`), and tool output is capped by `tools.max_result_chars`.

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

## Local Skills

Colibri loads user skills from configured local directories such as:

```text
~/.colibri/skills/<name>/SKILL.md
```

Optional `skill.toml` files can declare local commands for `skill.run`.

Colibri also ships a tiny built-in guidance skill, `create-colibri-skill`, so users can ask how to create a new Colibri skill without first installing a skill that explains skills.

Skill loading uses progressive disclosure: Colibri keeps a small metadata index in memory, selects relevant skills by keyword overlap for the current turn, then reads and injects only the selected skill instructions as temporary model context. The injected skill text is not stored in `AgentSession.messages`.

Skill injection is bounded by:

- `skills.max_loaded`
- `skills.max_instruction_chars`

Colibri does not install skills, fetch remote skills, or use a marketplace in v1.

## Context Compacting

Colibri keeps only `session.recent_message_limit` durable messages in memory (default `80`). Messages that fall out of that window are converted into a bounded rolling summary stored on the session.

When `session.model_compact = true` and the configured provider is not `fake`, Colibri asks the model to create a Claude Code style continuation summary. The compact request uses no tools and asks for plain text with an `<analysis>` scratchpad plus a `<summary>` section; Colibri strips the analysis block before storing the summary.

If model compacting fails, or when using the default fake provider, Colibri falls back to deterministic local compacting that keeps user/assistant text short and replaces old tool results with metadata.

The summary is injected into model input as temporary context and is not stored as a normal conversation message. Model input is also trimmed to fit `session.compact_trigger_chars` while preserving the latest user message.

## Console Status and Diagnostics

When `console.status = true`, Colibri writes concise status lines to `stderr`:

```text
[colibri] ready model=fake-colibri-model
[colibri] thinking
[colibri] tool files.read ok chars=1284
```

Model answers remain on `stdout`, so shell pipelines can still consume normal responses.

Run diagnostics with:

```bash
uv run python -m colibri.cli diagnostics
```

Diagnostics reports Python/platform details, provider/model, enabled tools, memory and skills paths, project permission file state, RSS when available, and context limits.

`session.idle_exit_seconds` controls REPL idle exit. Set it to `0` or a negative value to disable idle exit.

## Tool Permissions

Colibri checks permission before running each registered tool call.

The default `tools.default_permission = "allow_read_confirm_write"` allows read-only non-shell tools inside their normal safe boundaries and asks for confirmation before non-read-only tools. Shell commands require a grant or prompt even when they look read-only, because shell commands can leak secrets, consume resources, or behave differently across systems.

For `files.read` and `files.list`, the process startup directory is the default workspace root. Paths under that workspace root or configured `files.roots` are automatically allowed by the read-only default. Paths outside those roots use the dynamic permission prompt. A one-time approval allows only that call; session and project approvals allow the target's containing directory recursively, so Colibri does not ask again for every child path.

Other supported values are:

- `allow`: allow all registered tool calls.
- `confirm`: confirm every registered tool call.
- `deny`: deny every registered tool call.

Confirmation works over stdin/stdout, so it is safe for SSH-only servers. Prompt choices are:

- `y`: allow this call once.
- `s`: allow the same tool, exact shell command, or external file directory for this session.
- `e`: for shell only, allow the same executable for this session.
- `p`: allow the same tool, exact shell command, or external file directory for this project.
- `n`: deny this call and return a denial result to the model.

Project grants are stored in:

```text
.colibri/permissions.toml
```

Project-level shell grants are exact command matches. Allowing `git status` does not allow `git push`. Project-level file grants are recursive directory roots under `[files].roots`. `shell.deny` remains a hard deny list, and `.colibri/permissions.toml` should not be committed.

## Transcripts

When `session.transcript = true`, the CLI writes compact JSONL events to:

```text
~/.colibri/transcripts/YYYY-MM-DD.jsonl
```

Set `COLIBRI_HOME` to change the base directory. Transcript events include user messages, assistant messages, tool calls, permission decisions, tool results, model errors, and tool round limits. API keys are not logged by the runtime.

## Systemd

An example service is available at:

```text
deploy/systemd/colibri-repl.service
```

The example uses `Restart=no` because REPL idle timeout and `Restart=always` would restart the process after normal idle exits. Long-running daemon mode is intentionally left for future work.
