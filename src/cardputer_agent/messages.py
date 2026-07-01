from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass(frozen=True)
class Message:
    role: str
    content: str


@dataclass(frozen=True)
class ToolCall:
    id: str
    name: str
    arguments: dict[str, Any]


@dataclass(frozen=True)
class ModelLimits:
    timeout_seconds: int
    max_output_tokens: int


@dataclass(frozen=True)
class ModelResponse:
    text: str
    tool_calls: list[ToolCall] = field(default_factory=list)


@dataclass(frozen=True)
class AgentResponse:
    text: str
    messages: list[Message]
