from __future__ import annotations

import argparse
import json
import threading
import time
from pathlib import Path
from time import monotonic
import sys
from typing import Callable, Sequence

from colibri.console import ConsoleStatusWriter, StatusTranscript, format_answer_for_console
from colibri.config import DEFAULT_USER_CONFIG, AgentConfig, ConfigError, expand_user_path
from colibri.channels.weixin import WeixinChannelError, perform_weixin_auth
from colibri.diagnostics import build_diagnostics
from colibri.gateway import GatewayRunner
from colibri.gateway_process import GatewayProcessManager, format_gateway_status
from colibri.model.errors import ModelError
from colibri.model.factory import build_model_client
from colibri.repl_input import _is_selectable, read_repl_line, try_read_line
from colibri.session import AgentSession
from colibri.session_history import TranscriptHistoryLoader
from colibri.transcript import TranscriptWriter


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="colibri")
    parser.add_argument("--config", type=Path, default=None)
    subparsers = parser.add_subparsers(dest="command", required=True)

    ask = subparsers.add_parser("ask")
    ask.add_argument("text")

    subparsers.add_parser("repl")
    subparsers.add_parser("diagnostics")
    gateway = subparsers.add_parser("gateway")
    gateway_subparsers = gateway.add_subparsers(dest="gateway_action")
    for action in ("run", "start", "stop", "restart", "status"):
        gateway_subparsers.add_parser(action)

    auth = subparsers.add_parser("auth")
    auth_subparsers = auth.add_subparsers(dest="auth_provider", required=True)
    auth_subparsers.add_parser("weixin")
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

        if args.command == "gateway" and args.gateway_action is None:
            print("Usage: colibri gateway {run,start,stop,restart,status}", file=sys.stderr)
            return 2

        if args.command == "gateway" and args.gateway_action != "run":
            manager = GatewayProcessManager()
            if args.gateway_action == "start":
                gateway_status = manager.start(_active_config_path(args.config) if args.config is not None else None)
            elif args.gateway_action == "stop":
                gateway_status = manager.stop()
            elif args.gateway_action == "restart":
                gateway_status = manager.restart(_active_config_path(args.config) if args.config is not None else None)
            else:
                gateway_status = manager.status()
            for line in format_gateway_status(gateway_status):
                print(line)
            return 0

        load_config = config_loader or AgentConfig.load
        config = load_config(args.config)
        status = ConsoleStatusWriter(enabled=config.console.status)

        if args.command == "diagnostics":
            for line in build_diagnostics(config, args.config):
                print(line)
            return 0

        if args.command == "auth" and args.auth_provider == "weixin":
            result = perform_weixin_auth(
                base_url=config.channels.weixin.base_url,
                timeout_seconds=config.channels.weixin.auth_timeout_seconds,
            )
            config_path = _active_config_path(args.config)
            save_weixin_auth_config(config_path, result.token, result.base_url)
            print("Weixin auth succeeded.")
            print(f"user_id={result.user_id}")
            print(f"account_id={result.account_id}")
            print(f"base_url={result.base_url}")
            print(f"Config updated: {config_path}")
            return 0

        if args.command == "gateway" and args.gateway_action == "run":
            _write_ready_status(config, status)
            GatewayRunner(config=config, model=build_model_client(config.model)).run()
            return 0

        transcript = (
            TranscriptWriter.default(
                retention_days=config.session.transcript_retention_days,
                max_total_bytes=config.session.transcript_max_total_bytes,
            )
            if config.session.transcript
            else None
        )
        session = AgentSession(
            config=config,
            model=build_model_client(config.model),
            transcript=StatusTranscript(transcript, status),
            history_loader=TranscriptHistoryLoader.default(config.session)
            if config.session.restore_transcript
            else None,
        )
        _write_ready_status(config, status)

        try:
            if args.command == "ask":
                status.write("thinking")
                response = session.submit(args.text)
                print(format_answer_for_console(response.text, config.console.plain_answer))
                return 1 if response.error_type else 0

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
    except WeixinChannelError as error:
        print(f"Weixin channel error: {error}", file=sys.stderr)
        return 1


def run_steering_pump(
    session,
    *,
    stop: threading.Event,
    read_line: Callable[[float], str | None],
    status: ConsoleStatusWriter,
    sleep: Callable[[float], None] = time.sleep,
) -> None:
    """Background loop: forward stdin lines to session.steer while a turn runs.

    Approach C: do not read stdin while a permission prompt may be pending, so
    permission input() is not stolen. No extra prompt is printed.
    """
    notified_pending = False
    while not stop.is_set():
        if session.is_permission_pending():
            sleep(0.05)
            continue
        notified_pending = False
        line = read_line(0.2)
        if line is None:
            continue
        stripped = line.strip()
        if not stripped:
            continue
        if not session.steer(stripped):
            if session.is_permission_pending() and not notified_pending:
                status.write("permission_pending")
                notified_pending = True


def _run_repl(
    session: AgentSession,
    *,
    status: ConsoleStatusWriter,
    input_func: Callable[[str], str | None] | None = None,
    monotonic_func: Callable[[], float] = monotonic,
) -> int:
    last_activity = monotonic_func()
    history: list[str] = []
    while True:
        idle_seconds = session.config.session.idle_exit_seconds if session.config.session.idle_exit_enabled else 0
        if idle_seconds > 0 and monotonic_func() - last_activity >= idle_seconds:
            status.write("idle_exit", seconds=idle_seconds)
            return 0
        timeout_remaining = 0
        if idle_seconds > 0:
            timeout_remaining = max(0, idle_seconds - (monotonic_func() - last_activity))
        try:
            if input_func is None:
                user_text = read_repl_line("colibri> ", timeout_remaining, history=history)
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
        history.append(user_text)

        stop = threading.Event()
        pump_thread: threading.Thread | None = None
        if input_func is None and _is_selectable(sys.stdin):
            pump_thread = threading.Thread(
                target=run_steering_pump,
                kwargs={
                    "session": session,
                    "stop": stop,
                    "read_line": lambda t: try_read_line(
                        t, abort=session.is_permission_pending
                    ),
                    "status": status,
                },
                daemon=True,
            )
            pump_thread.start()
        try:
            status.write("thinking")
            print(
                format_answer_for_console(
                    session.submit(user_text).text,
                    session.config.console.plain_answer,
                )
            )
            last_activity = monotonic_func()
        except ModelError as error:
            print(f"Model error: {error}", file=sys.stderr)
            last_activity = monotonic_func()
            continue
        finally:
            stop.set()
            if pump_thread is not None:
                pump_thread.join(timeout=1)


def _write_ready_status(config: AgentConfig, status: ConsoleStatusWriter) -> None:
    status.write(
        "ready",
        model=config.model.model,
    )


def _active_config_path(path: Path | None) -> Path:
    return path if path is not None else expand_user_path(DEFAULT_USER_CONFIG)


def save_weixin_auth_config(path: Path, token: str, base_url: str) -> None:
    path = path.expanduser()
    path.parent.mkdir(parents=True, exist_ok=True)
    text = path.read_text(encoding="utf-8") if path.exists() else ""
    path.write_text(
        _upsert_toml_section_values(
            text,
            "channels.weixin",
            {
                "enabled": "true",
                "token": _toml_string(token),
                "base_url": _toml_string(base_url),
            },
        ),
        encoding="utf-8",
    )


def _upsert_toml_section_values(text: str, section_name: str, values: dict[str, str]) -> str:
    lines = text.splitlines()
    header = f"[{section_name}]"
    start = next((index for index, line in enumerate(lines) if line.strip() == header), None)
    if start is None:
        prefix = text.rstrip()
        section_lines = [header] + [f"{key} = {value}" for key, value in values.items()]
        return (prefix + "\n\n" if prefix else "") + "\n".join(section_lines) + "\n"

    end = len(lines)
    for index in range(start + 1, len(lines)):
        stripped = lines[index].strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            end = index
            break

    seen: set[str] = set()
    next_section = [lines[start]]
    for line in lines[start + 1 : end]:
        stripped = line.strip()
        if stripped and not stripped.startswith("#") and "=" in stripped:
            key = stripped.split("=", 1)[0].strip()
            if key in values:
                next_section.append(f"{key} = {values[key]}")
                seen.add(key)
                continue
        next_section.append(line)
    for key, value in values.items():
        if key not in seen:
            next_section.append(f"{key} = {value}")

    next_lines = lines[:start] + next_section + lines[end:]
    return "\n".join(next_lines).rstrip() + "\n"


def _toml_string(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


if __name__ == "__main__":
    raise SystemExit(main())
