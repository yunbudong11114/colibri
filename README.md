# Cardputer Agent

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
PYTHONPATH=src python -m cardputer_agent.cli ask "hello"
PYTHONPATH=src python -m cardputer_agent.cli repl
```

The runtime is standard-library only. `pytest` is only needed for development tests.
