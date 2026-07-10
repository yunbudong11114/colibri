from __future__ import annotations

import codecs
import os
import select
import sys
import termios
import tty
from collections.abc import Callable
from typing import TextIO


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


def try_read_line(
    timeout_seconds: float,
    stdin: TextIO = sys.stdin,
    abort: Callable[[], bool] | None = None,
) -> str | None:
    """Read one line without prompting or entering raw/tty mode.

    Returns None on timeout, EOF, or when stdin is not selectable (so callers
    can disable the concurrent steering pump). Safe to use while submit may
    call input() for permissions.

    If ``abort`` is provided, it is checked after select indicates readiness
    and before readline; when it returns True, returns None without reading.
    """
    if not _is_selectable(stdin):
        return None
    ready, _write_ready, _error_ready = select.select([stdin], [], [], timeout_seconds)
    if not ready:
        return None
    if abort is not None and abort():
        return None
    line = stdin.readline()
    if line == "":
        return None
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


def _is_selectable(stream: TextIO) -> bool:
    try:
        stream.fileno()
    except (OSError, ValueError, AttributeError):
        return False
    return True


def _is_tty(stream: TextIO) -> bool:
    try:
        return stream.isatty()
    except (OSError, ValueError, AttributeError):
        return False
