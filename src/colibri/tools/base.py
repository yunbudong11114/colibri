from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol

from colibri.config import AgentConfig


@dataclass(frozen=True)
class ToolSpec:
    name: str
    description: str
    input_schema: dict[str, Any]
    read_only: bool = True

    def as_openai_tool(self) -> dict[str, Any]:
        return {
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.input_schema,
            },
        }


@dataclass(frozen=True)
class ToolResult:
    ok: bool
    text: str
    error_type: str | None = None
    truncated: bool = False


@dataclass(frozen=True)
class ToolContext:
    config: AgentConfig
    cwd: Path
    allowed_file_roots: frozenset[str] = frozenset()


class Tool(Protocol):
    spec: ToolSpec

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        ...


def bound_tool_text(text: str, max_chars: int) -> tuple[str, bool]:
    if len(text) <= max_chars:
        return text, False
    suffix = "\n...[truncated]"
    keep = max(0, max_chars - len(suffix))
    return text[:keep] + suffix, True
