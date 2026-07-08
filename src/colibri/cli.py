from __future__ import annotations

import argparse
from pathlib import Path
import select
from time import monotonic
import sys
from typing import Callable, TextIO, Sequence

from colibri.console import ConsoleStatusWriter, StatusTranscript
from colibri.config import AgentConfig, ConfigError
from colibri.diagnostics import build_diagnostics
from colibri.model.errors import ModelError
from colibri.model.factory import build_model_client
from colibri.session import AgentSession
from colibri.tools.registry import ToolRegistry
from colibri.transcript import TranscriptWriter


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="colibri")
    parser.add_argument("--config", type=Path, default=None)
    subparsers = parser.add_subparsers(dest="command", required=True)

    ask = subparsers.add_parser("ask")
    ask.add_argument("text")

    subparsers.add_parser("repl")
    subparsers.add_parser("diagnostics")
    return parser


def main(
    argv: Sequence[str] | None = None,
    *,
    config_loader: Callable[[Path | None], AgentConfig] | None = None,
    input_func: Callable[[str], str | None] | None = None,
    monotonic_func: Callable[[], float] = monotonic,
) -> int:
    try:
        args = build_parser().parse_args(argv)
        load_config = config_loader or AgentConfig.load
        config = load_config(args.config)
        status = ConsoleStatusWriter(enabled=config.console.status)

        if args.command == "diagnostics":
            for line in build_diagnostics(config, args.config):
                print(line)
            return 0

        transcript = TranscriptWriter.default() if config.session.transcript else None
        session = AgentSession(
            config=config,
            model=build_model_client(config.model),
            transcript=StatusTranscript(transcript, status),
        )
        _write_ready_status(config, status)

        try:
            if args.command == "ask":
                status.write("thinking")
                print(session.submit(args.text).text)
                return 0

            if args.command == "repl":
                return _run_repl(session, status=status, input_func=input_func, monotonic_func=monotonic_func)

            return 2
        finally:
            session.close()
    except ConfigError as error:
        print(f"Config error: {error}", file=sys.stderr)
        return 1
    except ModelError as error:
        print(f"Model error: {error}", file=sys.stderr)
        return 1


def _run_repl(
    session: AgentSession,
    *,
    status: ConsoleStatusWriter,
    input_func: Callable[[str], str | None] | None = None,
    monotonic_func: Callable[[], float] = monotonic,
) -> int:
    last_activity = monotonic_func()
    while True:
        idle_seconds = session.config.session.idle_exit_seconds
        if idle_seconds > 0 and monotonic_func() - last_activity >= idle_seconds:
            status.write("idle_exit", seconds=idle_seconds)
            return 0
        timeout_remaining = 0
        if idle_seconds > 0:
            timeout_remaining = max(0, idle_seconds - (monotonic_func() - last_activity))
        try:
            if input_func is None:
                user_text = read_repl_line("colibri> ", timeout_remaining)
            else:
                user_text = input_func("colibri> ")
        except EOFError:
            print()
            return 0
        if user_text is None:
            status.write("idle_exit", seconds=idle_seconds)
            return 0

        if user_text.strip() in {"/quit", "/exit"}:
            return 0
        if not user_text.strip():
            continue

        try:
            status.write("thinking")
            print(session.submit(user_text).text)
            last_activity = monotonic_func()
        except ModelError as error:
            print(f"Model error: {error}", file=sys.stderr)
            return 1


def read_repl_line(
    prompt: str,
    timeout_seconds: float,
    stdin: TextIO = sys.stdin,
    stdout: TextIO = sys.stdout,
) -> str | None:
    stdout.write(prompt)
    stdout.flush()
    if timeout_seconds > 0 and _is_selectable(stdin):
        ready, _write_ready, _error_ready = select.select([stdin], [], [], timeout_seconds)
        if not ready:
            return None
    line = stdin.readline()
    if line == "":
        raise EOFError
    return line.rstrip("\n")


def _is_selectable(stream: TextIO) -> bool:
    try:
        stream.fileno()
    except (OSError, ValueError, AttributeError):
        return False
    return True


def _write_ready_status(config: AgentConfig, status: ConsoleStatusWriter) -> None:
    registry = ToolRegistry.from_config(config)
    status.write(
        "ready",
        model=config.model.model,
        tools=len(registry.specs()),
        memory="on" if config.memory.enabled else "off",
        skills=config.skills.max_loaded,
    )


if __name__ == "__main__":
    raise SystemExit(main())
