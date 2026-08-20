from __future__ import annotations

from dataclasses import dataclass
import os
import signal
import subprocess
import threading
from typing import Any


_TRUNCATION_MARKER = "\n...[truncated]...\n"


class ProcessTimedOut(RuntimeError):
    pass


@dataclass(frozen=True)
class BoundedProcessResult:
    returncode: int
    text: str
    truncated: bool


class _BoundedBytes:
    def __init__(self, capacity: int):
        self.capacity = max(1, capacity)
        self.head_capacity = self.capacity // 4
        self.tail_capacity = self.capacity - self.head_capacity
        self.buffer = bytearray()
        self.head = bytearray()
        self.tail = bytearray()
        self.truncated = False

    def feed(self, chunk: bytes) -> None:
        if not chunk:
            return
        if not self.truncated and len(self.buffer) + len(chunk) <= self.capacity:
            self.buffer.extend(chunk)
            return
        if not self.truncated:
            combined = bytes(self.buffer) + chunk
            self.head.extend(combined[: self.head_capacity])
            self.tail.extend(combined[-self.tail_capacity :])
            self.buffer.clear()
            self.truncated = True
            return
        self.tail.extend(chunk)
        if len(self.tail) > self.tail_capacity:
            del self.tail[: len(self.tail) - self.tail_capacity]

    def sampled(self) -> bytes:
        if not self.truncated:
            return bytes(self.buffer)
        return bytes(self.head + self.tail)


def run_bounded_process(
    popen_args: str | list[str],
    *,
    cwd: os.PathLike[str],
    timeout_seconds: float,
    max_result_chars: int,
    shell: bool = False,
) -> BoundedProcessResult:
    raw_capacity = max(64, max(0, max_result_chars) * 4)
    popen_kwargs: dict[str, Any] = {"shell": shell}
    if os.name != "nt":
        popen_kwargs["start_new_session"] = True
    process = subprocess.Popen(
        popen_args,
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        **popen_kwargs,
    )
    stream_capacity = max(32, raw_capacity // 2)
    stdout = _BoundedBytes(stream_capacity)
    stderr = _BoundedBytes(stream_capacity)
    readers = [
        threading.Thread(target=_read_stream, args=(process.stdout, stdout), daemon=True),
        threading.Thread(target=_read_stream, args=(process.stderr, stderr), daemon=True),
    ]
    for reader in readers:
        reader.start()
    try:
        process.wait(timeout=max(0.0, timeout_seconds))
    except subprocess.TimeoutExpired as error:
        _kill_process_tree(process)
        process.wait()
        for reader in readers:
            reader.join()
        raise ProcessTimedOut from error
    for reader in readers:
        reader.join()

    sampled = stdout.sampled() + stderr.sampled()
    text = sampled.decode("utf-8", errors="replace")
    text, text_truncated = _bound_head_tail_text(
        text,
        max_result_chars,
        force=stdout.truncated or stderr.truncated,
    )
    return BoundedProcessResult(
        returncode=process.returncode,
        text=text,
        truncated=text_truncated,
    )


def _read_stream(stream: Any, collector: _BoundedBytes) -> None:
    if stream is None:
        return
    with stream:
        while chunk := stream.read(8192):
            collector.feed(chunk)


def _kill_process_tree(process: subprocess.Popen[bytes]) -> None:
    if os.name == "nt":
        process.kill()
        return
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


def _bound_head_tail_text(text: str, max_chars: int, *, force: bool) -> tuple[str, bool]:
    limit = max(0, max_chars)
    if not force and len(text) <= limit:
        return text, False
    if limit == 0:
        return "", bool(text) or force
    if limit <= len(_TRUNCATION_MARKER):
        return _TRUNCATION_MARKER[:limit], True
    available = limit - len(_TRUNCATION_MARKER)
    head_chars = available // 4
    tail_chars = available - head_chars
    tail = text[-tail_chars:] if tail_chars else ""
    return text[:head_chars] + _TRUNCATION_MARKER + tail, True
