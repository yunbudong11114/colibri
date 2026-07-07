from __future__ import annotations

import re
from pathlib import Path
from typing import Any

from colibri.tools.base import ToolContext, ToolResult, ToolSpec, bound_tool_text


_TOPIC_RE = re.compile(r"^[A-Za-z0-9_-]+$")


def _topic_name(arguments: dict[str, Any]) -> str | None:
    value = arguments.get("topic")
    if not isinstance(value, str):
        return None
    topic = value.strip()
    return topic if _TOPIC_RE.fullmatch(topic) else None


def _memory_root(context: ToolContext) -> Path:
    return context.config.memory.root.expanduser()


def _topics_dir(context: ToolContext) -> Path:
    return _memory_root(context) / "topics"


def _topic_path(topic: str, context: ToolContext) -> Path:
    return _topics_dir(context) / f"{topic}.md"


class MemoryListTool:
    spec = ToolSpec(
        name="memory.list",
        description="List available memory topics.",
        input_schema={"type": "object", "properties": {}},
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        topics_dir = _topics_dir(context)
        if not topics_dir.exists():
            return ToolResult(ok=True, text="")
        if not topics_dir.is_dir():
            return ToolResult(ok=False, text="Memory topics path is not a directory", error_type="not_directory")

        topics = sorted(path.stem for path in topics_dir.glob("*.md") if path.is_file())
        text, truncated = bound_tool_text("\n".join(topics), context.config.tools.max_result_chars)
        return ToolResult(ok=True, text=text, truncated=truncated)


class MemoryReadTool:
    spec = ToolSpec(
        name="memory.read",
        description="Read a memory topic by name.",
        input_schema={
            "type": "object",
            "properties": {"topic": {"type": "string"}},
            "required": ["topic"],
        },
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        topic = _topic_name(arguments)
        if topic is None:
            return ToolResult(ok=False, text="Invalid topic", error_type="invalid_arguments")

        path = _topic_path(topic, context)
        if not path.exists():
            return ToolResult(ok=False, text="Memory topic does not exist", error_type="not_found")
        if not path.is_file():
            return ToolResult(ok=False, text="Memory topic path is not a file", error_type="not_file")

        text = path.read_text(encoding="utf-8", errors="replace")
        bounded, truncated = bound_tool_text(text, context.config.tools.max_result_chars)
        return ToolResult(ok=True, text=bounded, truncated=truncated)


class MemorySearchTool:
    spec = ToolSpec(
        name="memory.search",
        description="Search memory index and topic files by keyword.",
        input_schema={
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"],
        },
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        query = arguments.get("query")
        if not isinstance(query, str) or not query.strip():
            return ToolResult(ok=False, text="Missing query", error_type="invalid_arguments")

        needle = query.strip().lower()
        matches: list[str] = []
        for label, path in self._search_files(context):
            if len(matches) >= context.config.memory.max_search_results:
                break
            if not path.is_file():
                continue
            for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
                if needle in line.lower():
                    matches.append(f"{label}: {line}")
                    if len(matches) >= context.config.memory.max_search_results:
                        break

        text, truncated = bound_tool_text("\n".join(matches), context.config.tools.max_result_chars)
        return ToolResult(ok=True, text=text, truncated=truncated)

    @staticmethod
    def _search_files(context: ToolContext) -> list[tuple[str, Path]]:
        root = _memory_root(context)
        files: list[tuple[str, Path]] = [("index", root / "MEMORY.md")]
        topics_dir = _topics_dir(context)
        if topics_dir.is_dir():
            files.extend((path.stem, path) for path in sorted(topics_dir.glob("*.md")) if path.is_file())
        return files


class MemoryWriteTool:
    spec = ToolSpec(
        name="memory.write",
        description="Append a Markdown bullet to a memory topic.",
        input_schema={
            "type": "object",
            "properties": {
                "topic": {"type": "string"},
                "text": {"type": "string"},
            },
            "required": ["topic", "text"],
        },
        read_only=False,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        topic = _topic_name(arguments)
        if topic is None:
            return ToolResult(ok=False, text="Invalid topic", error_type="invalid_arguments")

        raw_text = arguments.get("text")
        if not isinstance(raw_text, str) or not raw_text.strip():
            return ToolResult(ok=False, text="Missing text", error_type="invalid_arguments")

        topics_dir = _topics_dir(context)
        topics_dir.mkdir(parents=True, exist_ok=True)
        path = _topic_path(topic, context)
        with path.open("a", encoding="utf-8") as handle:
            handle.write(f"- {raw_text.strip()}\n")

        text, truncated = bound_tool_text(f"Appended memory topic: {topic}", context.config.tools.max_result_chars)
        return ToolResult(ok=True, text=text, truncated=truncated)
