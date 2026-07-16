from __future__ import annotations

import sys
from typing import TextIO

from colibri.transcript import format_beijing_timestamp


def format_gateway_log(message: str) -> str:
    return f"[{format_beijing_timestamp()}] [gateway] {message}"


def gateway_log(message: str, *, stream: TextIO | None = None) -> None:
    print(format_gateway_log(message), file=stream or sys.stderr, flush=True)
