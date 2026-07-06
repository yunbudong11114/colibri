# Colibri

Lightweight Python agent runtime for CardputerZero-class Linux devices.

## Runtime Support

Colibri must run on headless Linux servers over plain SSH. The core runtime and milestone work should stay usable through CLI/stdin/stdout only, without requiring a graphical desktop, browser, system tray, display server, audio device, or TUI framework.

## Current Milestone

Milestone 2 provides:

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

When the configured model returns tool calls, Colibri can execute a small read-only tool set:

- `files.list`: list direct children under configured `files.roots`.
- `files.read`: read UTF-8 text files under configured `files.roots`.
- `shell.run`: run allowlisted commands such as `ls`, `cat`, `sed`, `rg`, `python`, and `git status`.

Tool calls are bounded by `session.max_tool_rounds`, and tool output is capped by `tools.max_result_chars`.
