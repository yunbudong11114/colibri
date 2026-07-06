# Colibri

Lightweight Python agent runtime for CardputerZero-class Linux devices.

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
