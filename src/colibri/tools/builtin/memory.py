from __future__ import annotations

import re
from pathlib import Path
from typing import Any

from colibri.memory import ALWAYS_ON_MEMORY_FILE_LIMITS, ALWAYS_ON_MEMORY_FILES
from colibri.tools.base import ToolContext, ToolResult, ToolSpec, bound_tool_text


_TOPIC_RE = re.compile(r"^[A-Za-z0-9_-]+$")
_BUILTIN_MEMORY_FILES = (*ALWAYS_ON_MEMORY_FILES, "INDEX.md")
_DEFAULT_WRITE_MODE = "append"


def _memory_root(context: ToolContext) -> Path:
    return context.config.memory.root.expanduser()


def _topics_dir(context: ToolContext) -> Path:
    return _memory_root(context) / "topics"


def _topic_name(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    topic = value.strip()
    return topic if _TOPIC_RE.fullmatch(topic) else None


def _display_name(path: Path, context: ToolContext) -> str:
    return path.relative_to(_memory_root(context)).as_posix()


def _memory_file_path(arguments: dict[str, Any], context: ToolContext) -> tuple[Path, str] | None:
    topic = _topic_name(arguments.get("topic"))
    if topic is not None:
        path = _topics_dir(context) / f"{topic}.md"
        return path, f"topics/{topic}.md"

    raw_file = arguments.get("file")
    if not isinstance(raw_file, str):
        return None
    filename = raw_file.strip()
    if filename in _BUILTIN_MEMORY_FILES:
        return _memory_root(context) / filename, filename
    if filename.startswith("topics/") and filename.endswith(".md"):
        topic = filename.removeprefix("topics/").removesuffix(".md")
        if _topic_name(topic) is None:
            return None
        return _topics_dir(context) / f"{topic}.md", filename
    return None


class MemoryListTool:
    spec = ToolSpec(
        name="memory.list",
        description="List available memory files.",
        input_schema={"type": "object", "properties": {}},
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        root = _memory_root(context)
        entries: list[str] = []
        for filename in _BUILTIN_MEMORY_FILES:
            if (root / filename).is_file():
                entries.append(filename)

        topics_dir = _topics_dir(context)
        if topics_dir.is_dir():
            entries.extend(
                f"topics/{path.name}" for path in sorted(topics_dir.glob("*.md")) if path.is_file()
            )

        text, truncated = bound_tool_text("\n".join(entries), context.config.tools.max_result_chars)
        return ToolResult(ok=True, text=text, truncated=truncated)


class MemoryReadTool:
    spec = ToolSpec(
        name="memory.read",
        description="Read SOUL.md, USER.md, MEMORY.md, INDEX.md, or a topic memory file.",
        input_schema={
            "type": "object",
            "properties": {
                "file": {"type": "string"},
                "topic": {"type": "string"},
            },
        },
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        target = _memory_file_path(arguments, context)
        if target is None:
            return ToolResult(ok=False, text="Invalid memory file", error_type="invalid_arguments")
        path, _label = target
        if not path.exists():
            return ToolResult(ok=False, text="Memory file does not exist", error_type="not_found")
        if not path.is_file():
            return ToolResult(ok=False, text="Memory path is not a file", error_type="not_file")

        text = path.read_text(encoding="utf-8", errors="replace")
        bounded, truncated = bound_tool_text(text, context.config.tools.max_result_chars)
        return ToolResult(ok=True, text=bounded, truncated=truncated)


class MemorySearchTool:
    spec = ToolSpec(
        name="memory.search",
        description="Search INDEX.md memory manifest lines by keyword.",
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

        needle = query.strip().casefold()
        matches: list[str] = []
        path = _memory_root(context) / "INDEX.md"
        if path.is_file():
            for line_number, line in enumerate(path.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
                if needle in line.casefold():
                    matches.append(f"INDEX.md:{line_number}: {line}")
                    if len(matches) >= context.config.memory.max_search_results:
                        break

        text, truncated = bound_tool_text("\n".join(matches), context.config.tools.max_result_chars)
        return ToolResult(ok=True, text=text, truncated=truncated)


class MemoryWriteTool:
    spec = ToolSpec(
        name="memory.write",
        description=(
            "Append to or replace a memory file. Memory files must use frontmatter:\n"
            "---\n"
            "type: soul|user|feedback|project|reference|system\n"
            "description: one-line description\n"
            "updated: YYYY-MM-DD\n"
            "---\n"
            "Choose SOUL.md for Colibri persona, principles, expression style, and durable self-constraints; keep it under 400 characters. "
            "Choose USER.md for user profile, preferences, and collaboration style; keep it under 400 characters. "
            "Choose MEMORY.md for short stable general, project, or system facts; keep it under 1200 characters. "
            "Choose INDEX.md for the searchable topic manifest used by memory.search. "
            "Choose topics/<name>.md for detailed topic notes. "
            "When creating or materially changing a topic file, also update INDEX.md with a searchable one-line pointer. "
            "Consolidate or replace SOUL.md, USER.md, and MEMORY.md instead of appending forever."
        ),
        input_schema={
            "type": "object",
            "properties": {
                "file": {"type": "string"},
                "topic": {"type": "string"},
                "content": {"type": "string"},
                "mode": {"type": "string", "enum": ["append", "replace"]},
            },
            "required": ["content"],
        },
        read_only=False,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        target = _memory_file_path(arguments, context)
        if target is None:
            return ToolResult(ok=False, text="Invalid memory file", error_type="invalid_arguments")
        path, label = target

        raw_content = arguments.get("content")
        if not isinstance(raw_content, str) or not raw_content.strip():
            return ToolResult(ok=False, text="Missing content", error_type="invalid_arguments")

        mode = arguments.get("mode", _DEFAULT_WRITE_MODE)
        if mode not in {"append", "replace"}:
            return ToolResult(ok=False, text="Invalid write mode", error_type="invalid_arguments")

        path.parent.mkdir(parents=True, exist_ok=True)
        content = raw_content.strip()
        if mode == "replace":
            path.write_text(content + ("\n" if not content.endswith("\n") else ""), encoding="utf-8")
        else:
            with path.open("a", encoding="utf-8") as handle:
                handle.write(content)
                if not content.endswith("\n"):
                    handle.write("\n")

        message = f"Updated memory file: {label}"
        if label.startswith("topics/"):
            message += "\nRemember to update INDEX.md so this topic can be found by memory.search."
        if label in ALWAYS_ON_MEMORY_FILE_LIMITS:
            size = len(path.read_text(encoding="utf-8", errors="replace"))
            limit = ALWAYS_ON_MEMORY_FILE_LIMITS[label]
            if size > limit:
                message += (
                    f"\n{label} exceeds {limit} characters. Summarize or consolidate it, then call "
                    f'memory.write with file="{label}", mode="replace" to keep it within the limit.'
                )
        text, truncated = bound_tool_text(message, context.config.tools.max_result_chars)
        return ToolResult(ok=True, text=text, truncated=truncated)
