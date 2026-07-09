from __future__ import annotations

import argparse
import codecs
import json
import os
from pathlib import Path
import select
from time import monotonic
import sys
import termios
import tty
from typing import Callable, TextIO, Sequence

from colibri.console import ConsoleStatusWriter, StatusTranscript
from colibri.config import DEFAULT_USER_CONFIG, AgentConfig, ConfigError, expand_user_path
from colibri.channels.weixin import WeixinChannelError, perform_weixin_auth
from colibri.diagnostics import build_diagnostics
from colibri.gateway import GatewayRunner
from colibri.gateway_process import GatewayProcessManager, format_gateway_status
from colibri.model.errors import ModelError
from colibri.model.factory import build_model_client
from colibri.session import AgentSession
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

        if args.command == "gateway" and args.gateway_action is None:
            print("Usage: colibri gateway {run,start,stop,restart,status}", file=sys.stderr)
            return 2

        if args.command == "gateway" and args.gateway_action == "run":
            _write_ready_status(config, status)
            GatewayRunner(config=config, model=build_model_client(config.model)).run()
            return 0

        if args.command == "gateway":
            manager = GatewayProcessManager()
            if args.gateway_action == "start":
                gateway_status = manager.start(_active_config_path(args.config) if args.config is not None else None)
                for line in format_gateway_status(gateway_status):
                    print(line)
                return 0
            if args.gateway_action == "stop":
                gateway_status = manager.stop()
                for line in format_gateway_status(gateway_status):
                    print(line)
                return 0
            if args.gateway_action == "restart":
                gateway_status = manager.restart(_active_config_path(args.config) if args.config is not None else None)
                for line in format_gateway_status(gateway_status):
                    print(line)
                return 0
            if args.gateway_action == "status":
                gateway_status = manager.status()
                for line in format_gateway_status(gateway_status):
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
    except WeixinChannelError as error:
        print(f"Weixin channel error: {error}", file=sys.stderr)
        return 1


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
    history: list[str] | None = None,
) -> str | None:
    if _is_tty(stdin):
        return _read_repl_line_tty(prompt, timeout_seconds, stdin, stdout, history=history)
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


class ReplLineEditor:
    def __init__(self, prompt: str, stdout: TextIO, history: list[str] | None = None):
        self.prompt = prompt
        self.stdout = stdout
        self.history = history or []
        self._chars: list[str] = []
        self._history_index: int | None = None
        self._draft: str = ""

    @property
    def text(self) -> str:
        return "".join(self._chars)

    def start(self) -> None:
        self.stdout.write(self.prompt)
        self.stdout.flush()

    def feed_text(self, text: str) -> None:
        self._history_index = None
        self._chars.extend(text)
        self.redraw()

    def backspace(self) -> None:
        self._history_index = None
        if self._chars:
            self._chars.pop()
        self.redraw()

    def history_previous(self) -> None:
        if not self.history:
            return
        if self._history_index is None:
            self._draft = self.text
            self._history_index = len(self.history) - 1
        else:
            self._history_index = max(0, self._history_index - 1)
        self._replace_text(self.history[self._history_index])

    def history_next(self) -> None:
        if self._history_index is None:
            return
        if self._history_index >= len(self.history) - 1:
            self._history_index = None
            self._replace_text(self._draft)
            return
        self._history_index += 1
        self._replace_text(self.history[self._history_index])

    def _replace_text(self, text: str) -> None:
        self._chars = list(text)
        self.redraw()

    def redraw(self) -> None:
        self.stdout.write(f"\r\x1b[2K{self.prompt}{self.text}")
        self.stdout.flush()


def _read_repl_line_tty(
    prompt: str,
    timeout_seconds: float,
    stdin: TextIO,
    stdout: TextIO,
    history: list[str] | None = None,
) -> str | None:
    fd = stdin.fileno()
    previous = termios.tcgetattr(fd)
    editor = ReplLineEditor(prompt, stdout, history=history)
    decoder = codecs.getincrementaldecoder("utf-8")()
    try:
        tty.setraw(fd)
        editor.start()
        while True:
            if timeout_seconds > 0:
                ready, _write_ready, _error_ready = select.select([stdin], [], [], timeout_seconds)
                if not ready:
                    write_raw_tty_newline(stdout)
                    return None
            data = read_tty_byte(fd)
            if data == b"":
                raise EOFError
            if data in {b"\r", b"\n"}:
                write_raw_tty_newline(stdout)
                return editor.text
            if data == b"\x03":
                raise KeyboardInterrupt
            if data == b"\x04":
                if not editor.text:
                    raise EOFError
                continue
            if data == b"\x1b":
                decoder.reset()
                _handle_escape_sequence(editor, read_escape_sequence(fd))
                continue
            if data in {b"\x7f", b"\b"}:
                decoder.reset()
                editor.backspace()
                continue
            text = decoder.decode(data, final=False)
            if text:
                editor.feed_text(text)
    finally:
        termios.tcsetattr(fd, termios.TCSADRAIN, previous)


def _is_selectable(stream: TextIO) -> bool:
    try:
        stream.fileno()
    except (OSError, ValueError, AttributeError):
        return False
    return True


def read_tty_byte(fd: int) -> bytes:
    return os.read(fd, 1)


def write_raw_tty_newline(stdout: TextIO) -> None:
    stdout.write("\r\n")
    stdout.flush()


def read_escape_sequence(fd: int) -> bytes:
    sequence = bytearray(b"\x1b")
    while len(sequence) < 8:
        ready, _write_ready, _error_ready = select.select([fd], [], [], 0.01)
        if not ready:
            break
        next_byte = read_tty_byte(fd)
        if next_byte == b"":
            break
        sequence.extend(next_byte)
        if len(sequence) == 2 and next_byte in {b"[", b"O"}:
            continue
        if 0x40 <= next_byte[0] <= 0x7E:
            break
    return bytes(sequence)


def _handle_escape_sequence(editor: ReplLineEditor, sequence: bytes) -> None:
    if sequence in {b"\x1b[A", b"\x1bOA"}:
        editor.history_previous()
    elif sequence in {b"\x1b[B", b"\x1bOB"}:
        editor.history_next()


def _is_tty(stream: TextIO) -> bool:
    try:
        return stream.isatty()
    except (OSError, ValueError, AttributeError):
        return False


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
