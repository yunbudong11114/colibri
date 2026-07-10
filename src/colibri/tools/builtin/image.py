from __future__ import annotations

from pathlib import Path
import mimetypes
from typing import Any

from colibri.tools.base import ToolContext, ToolResult, ToolSpec, bound_tool_text
from colibri.tools.builtin.files import is_under_allowed_file_root, resolve_file_path
from colibri.vision import VisionError


class ImageUnderstandTool:
    spec = ToolSpec(
        name="image.understand",
        description=(
            "Analyze an image with the configured vision model. The path must be under the current workspace, "
            "configured file roots, or an already authorized file root. Paths outside those roots use the normal "
            "file permission prompt. Use this only when image analysis is needed."
        ),
        input_schema={
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Local image path."},
                "prompt": {"type": "string", "description": "What to inspect or extract from the image."},
            },
            "required": ["path"],
            "additionalProperties": False,
        },
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        raw_path = arguments.get("path")
        if not isinstance(raw_path, str) or not raw_path:
            return ToolResult(ok=False, text="Missing path", error_type="invalid_arguments")
        if context.image_analyzer is None:
            return ToolResult(ok=False, text="Image understanding is unavailable", error_type="vision_unavailable")

        path = resolve_file_path(raw_path, context.cwd)
        if path is None:
            return ToolResult(ok=False, text="Invalid image path", error_type="invalid_arguments")
        if not is_under_allowed_file_root(path, context):
            return ToolResult(ok=False, text="Path is outside allowed roots", error_type="permission_denied")
        if not path.exists():
            return ToolResult(ok=False, text="Path does not exist", error_type="not_found")
        if not path.is_file():
            return ToolResult(ok=False, text="Path is not a file", error_type="not_file")
        content_type = mimetypes.guess_type(path.name)[0] or ""
        if not content_type.startswith("image/"):
            return ToolResult(ok=False, text="Path is not an image", error_type="invalid_media")

        prompt = arguments.get("prompt", "")
        if not isinstance(prompt, str):
            return ToolResult(ok=False, text="Prompt must be a string", error_type="invalid_arguments")
        try:
            text = context.image_analyzer(path, prompt)
        except VisionError as error:
            return ToolResult(ok=False, text=str(error), error_type=error.error_type)
        except Exception as error:
            return ToolResult(ok=False, text=str(error), error_type="model_error")
        bounded, truncated = bound_tool_text(text, context.config.tools.max_result_chars)
        return ToolResult(ok=True, text=bounded, truncated=truncated)
