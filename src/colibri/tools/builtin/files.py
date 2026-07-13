from __future__ import annotations

from pathlib import Path
import mimetypes
from typing import Any

from colibri.media import MediaPart
from colibri.tools.base import ToolContext, ToolResult, ToolSpec, bound_tool_text


def _path_argument(arguments: dict[str, Any]) -> str | None:
    value = arguments.get("path")
    return value if isinstance(value, str) and value else None


def resolve_file_path(raw_path: str, cwd: Path) -> Path | None:
    candidate = Path(raw_path).expanduser()
    if not candidate.is_absolute():
        candidate = cwd / candidate
    try:
        return candidate.resolve()
    except OSError:
        return None


def is_under_workspace_file_root(path: Path, context: ToolContext) -> bool:
    roots = [context.cwd, *context.config.files.roots]
    for raw_root in roots:
        try:
            path.relative_to(raw_root.expanduser().resolve())
            return True
        except (OSError, ValueError):
            continue
    return False


def is_under_allowed_file_root(path: Path, context: ToolContext) -> bool:
    if is_under_workspace_file_root(path, context):
        return True
    for raw_root in context.allowed_file_roots:
        try:
            path.relative_to(Path(raw_root).expanduser().resolve())
            return True
        except (OSError, ValueError):
            continue
    return False


def _resolve_allowed_path(raw_path: str, context: ToolContext) -> Path | None:
    resolved = resolve_file_path(raw_path, context.cwd)
    if resolved is None:
        return None
    if is_under_allowed_file_root(resolved, context):
        return resolved
    return None


class FilesListTool:
    spec = ToolSpec(
        name="files.list",
        description="List direct children of an allowed directory.",
        input_schema={
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
        },
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        raw_path = _path_argument(arguments)
        if raw_path is None:
            return ToolResult(ok=False, text="Missing path", error_type="invalid_arguments")
        path = _resolve_allowed_path(raw_path, context)
        if path is None:
            return ToolResult(ok=False, text="Path is outside allowed roots", error_type="permission_denied")
        if not path.exists():
            return ToolResult(ok=False, text="Path does not exist", error_type="not_found")
        if not path.is_dir():
            return ToolResult(ok=False, text="Path is not a directory", error_type="not_directory")

        entries = sorted(child.name + ("/" if child.is_dir() else "") for child in path.iterdir())
        text, truncated = bound_tool_text("\n".join(entries), context.config.tools.max_result_chars)
        return ToolResult(ok=True, text=text, truncated=truncated)


class FilesReadTool:
    spec = ToolSpec(
        name="files.read",
        description=(
            "Read a UTF-8 text file under an allowed root. Prefer start_line/end_line ranges for large files. "
            "Optional max_chars caps this read result and is itself capped by tools.max_result_chars."
        ),
        input_schema={
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "start_line": {"type": "integer", "minimum": 1},
                "end_line": {"type": "integer", "minimum": 1},
                "max_chars": {"type": "integer", "minimum": 1},
            },
            "required": ["path"],
        },
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        raw_path = _path_argument(arguments)
        if raw_path is None:
            return ToolResult(ok=False, text="Missing path", error_type="invalid_arguments")
        path = _resolve_allowed_path(raw_path, context)
        if path is None:
            return ToolResult(ok=False, text="Path is outside allowed roots", error_type="permission_denied")
        if not path.exists():
            return ToolResult(ok=False, text="Path does not exist", error_type="not_found")
        if not path.is_file():
            return ToolResult(ok=False, text="Path is not a file", error_type="not_file")

        range_args = _line_range_arguments(arguments)
        if isinstance(range_args, ToolResult):
            return range_args
        start_line, end_line, max_chars = range_args
        text = path.read_text(encoding="utf-8", errors="replace")
        if start_line is not None or end_line is not None:
            text = _select_line_range(text, start_line, end_line)
        limit = min(context.config.tools.max_result_chars, max_chars or context.config.tools.max_result_chars)
        bounded, truncated = bound_tool_text(text, limit)
        return ToolResult(ok=True, text=bounded, truncated=truncated)


class FilesWriteTool:
    spec = ToolSpec(
        name="files.write",
        description=(
            "Write a UTF-8 text file under an allowed root. Use this for generated artifacts and file edits; "
            "do not use shell redirection or heredocs to create files."
        ),
        input_schema={
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"},
            },
            "required": ["path", "content"],
        },
        read_only=False,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        raw_path = _path_argument(arguments)
        content = arguments.get("content")
        if raw_path is None:
            return ToolResult(ok=False, text="Missing path", error_type="invalid_arguments")
        if not isinstance(content, str):
            return ToolResult(ok=False, text="Missing content", error_type="invalid_arguments")
        path = _resolve_allowed_path(raw_path, context)
        if path is None:
            return ToolResult(ok=False, text="Path is outside allowed roots", error_type="permission_denied")
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
        except OSError as error:
            return ToolResult(ok=False, text=str(error), error_type="execution_error")
        return ToolResult(ok=True, text=f"Wrote {len(content.encode('utf-8'))} bytes to {path}")


class FilesSendTool:
    spec = ToolSpec(
        name="files.send",
        description=(
            "Send a local file to the current chat channel. This can expose host files outside Colibri, "
            "so use it only when the user asked to send a file."
        ),
        input_schema={
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "caption": {"type": "string"},
            },
            "required": ["path"],
        },
        read_only=False,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        if context.media_sender is None:
            return ToolResult(
                ok=False,
                text="No active channel can send files in this session",
                error_type="media_unavailable",
            )
        raw_path = _path_argument(arguments)
        if raw_path is None:
            return ToolResult(ok=False, text="Missing path", error_type="invalid_arguments")
        path = _resolve_allowed_path(raw_path, context)
        if path is None:
            return ToolResult(ok=False, text="Path is outside allowed roots", error_type="permission_denied")
        if not path.exists():
            return ToolResult(ok=False, text="Path does not exist", error_type="not_found")
        if not path.is_file():
            return ToolResult(ok=False, text="Path is not a file", error_type="not_file")

        content_type = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
        caption = arguments.get("caption")
        media = MediaPart(
            type=_media_type_for_content(content_type),
            path=path,
            filename=path.name,
            content_type=content_type,
            caption=caption if isinstance(caption, str) else "",
        )
        return ToolResult(ok=True, text=f"Sent file to channel: {path.name}", media=media)


def _media_type_for_content(content_type: str) -> str:
    if content_type.startswith("image/"):
        return "image"
    if content_type.startswith("video/"):
        return "video"
    if content_type.startswith("audio/"):
        return "audio"
    return "file"


def _line_range_arguments(arguments: dict[str, Any]) -> tuple[int | None, int | None, int | None] | ToolResult:
    start_line = _positive_int_argument(arguments, "start_line")
    end_line = _positive_int_argument(arguments, "end_line")
    max_chars = _positive_int_argument(arguments, "max_chars")
    for value in (start_line, end_line, max_chars):
        if value == "invalid":
            return ToolResult(ok=False, text="Invalid line range or max_chars", error_type="invalid_arguments")
    assert start_line != "invalid"
    assert end_line != "invalid"
    assert max_chars != "invalid"
    if start_line is not None and end_line is not None and start_line > end_line:
        return ToolResult(ok=False, text="start_line must be <= end_line", error_type="invalid_arguments")
    return start_line, end_line, max_chars


def _positive_int_argument(arguments: dict[str, Any], name: str) -> int | None | str:
    if name not in arguments:
        return None
    value = arguments.get(name)
    if isinstance(value, bool):
        return "invalid"
    if isinstance(value, int):
        parsed = value
    elif isinstance(value, str) and value.isdigit():
        parsed = int(value)
    else:
        return "invalid"
    return parsed if parsed >= 1 else "invalid"


def _select_line_range(text: str, start_line: int | None, end_line: int | None) -> str:
    lines = text.splitlines(keepends=True)
    start = max(0, (start_line or 1) - 1)
    end = end_line if end_line is not None else len(lines)
    return "".join(lines[start:end])
