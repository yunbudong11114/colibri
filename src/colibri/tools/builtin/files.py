from __future__ import annotations

from pathlib import Path
from typing import Any

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
        description="Read a UTF-8 text file under an allowed root.",
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
        if not path.is_file():
            return ToolResult(ok=False, text="Path is not a file", error_type="not_file")

        text = path.read_text(encoding="utf-8", errors="replace")
        bounded, truncated = bound_tool_text(text, context.config.tools.max_result_chars)
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
