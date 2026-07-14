from __future__ import annotations

from collections import deque
import threading
import time
from typing import Generic, TypeVar

T = TypeVar("T")


class InboundRouter(Generic[T]):
    """Per-session inbound queues with a global pending bound and fair RR acquire."""

    def __init__(self, max_pending: int = 8):
        self._max_pending = max(1, max_pending)
        self._condition = threading.Condition()
        self._queues: dict[str, deque[T]] = {}
        self._rr: deque[str] = deque()
        self._total = 0
        self._active: set[str] = set()
        self._closed = False

    def try_enqueue(self, key: str, item: T) -> bool:
        with self._condition:
            if self._closed or self._total >= self._max_pending:
                return False
            queue = self._queues.setdefault(key, deque())
            was_empty = not queue
            queue.append(item)
            self._total += 1
            if was_empty and key not in self._active and key not in self._rr:
                self._rr.append(key)
            self._condition.notify()
            return True

    def acquire(self, timeout: float | None = None) -> tuple[str, T] | None:
        with self._condition:
            deadline = None if timeout is None else time.monotonic() + max(0.0, timeout)
            while True:
                pair = self._try_acquire_locked()
                if pair is not None:
                    return pair
                if self._closed:
                    return None
                if deadline is None:
                    self._condition.wait()
                    continue
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return None
                self._condition.wait(timeout=remaining)

    def _try_acquire_locked(self) -> tuple[str, T] | None:
        for _ in range(len(self._rr)):
            key = self._rr.popleft()
            if key in self._active:
                self._rr.append(key)
                continue
            queue = self._queues.get(key)
            if not queue:
                continue
            item = queue.popleft()
            self._total = max(0, self._total - 1)
            self._active.add(key)
            if queue:
                self._rr.append(key)
            return key, item
        return None

    def release(self, key: str) -> None:
        with self._condition:
            self._active.discard(key)
            queue = self._queues.get(key)
            if queue and key not in self._rr:
                self._rr.append(key)
            self._condition.notify_all()

    def close(self) -> None:
        with self._condition:
            self._closed = True
            self._condition.notify_all()

    @property
    def pending_len(self) -> int:
        with self._condition:
            return self._total

    @property
    def active_len(self) -> int:
        with self._condition:
            return len(self._active)

    def wait_idle(self, timeout: float | None = None) -> bool:
        with self._condition:
            deadline = None if timeout is None else time.monotonic() + max(0.0, timeout)
            while self._total > 0 or self._active:
                if deadline is None:
                    self._condition.wait()
                    continue
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return False
                self._condition.wait(timeout=remaining)
            return True
