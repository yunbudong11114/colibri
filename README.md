# Colibri

Lightweight Python agent runtime for CardputerZero-class Linux devices.

## Current Milestone

Milestone 1 provides:

- Python package skeleton.
- TOML config loader with CardputerZero-friendly defaults.
- Message and model interfaces.
- Deterministic fake model for tests and smoke runs.
- `AgentSession.submit()` for a bounded single model turn.
- CLI `ask` and `repl` commands.

## Development

```bash
python -m pytest
PYTHONPATH=src python -m colibri.cli ask "hello"
PYTHONPATH=src python -m colibri.cli repl
```

The runtime is standard-library only. `pytest` is only needed for development tests.

## Model Providers

Colibri defaults to the deterministic fake model:

```bash
PYTHONPATH=src python -m colibri.cli ask "hello"
```

To use an OpenAI-compatible chat completions API, copy `configs/openai.example.toml`, set `OPENAI_API_KEY` in the environment, and pass the config:

```bash
PYTHONPATH=src python -m colibri.cli --config configs/openai.example.toml ask "say hi in five words"
```

The runtime does not read API keys from config files. It reads the environment variable named by `model.api_key_env`.
