from __future__ import annotations

import shlex
import subprocess
from typing import Any

from colibri.tools.base import ToolContext, ToolResult, ToolSpec, bound_tool_text


class ShellRunTool:
    spec = ToolSpec(
        name="shell.run",
        description=(
            "Run a shell command after Colibri permission approval. Do not use this to create or edit files; "
            "use files.write for generated artifacts and text file changes."
        ),
        input_schema={
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"],
        },
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        command = arguments.get("command")
        if not isinstance(command, str) or not command.strip():
            return ToolResult(ok=False, text="Missing command", error_type="invalid_arguments")

        try:
            argv = shlex.split(command)
        except ValueError as error:
            return ToolResult(ok=False, text=str(error), error_type="invalid_arguments")
        if not argv:
            return ToolResult(ok=False, text="Missing command", error_type="invalid_arguments")

        executable = argv[0]
        if executable in context.config.shell.deny:
            return ToolResult(ok=False, text="Command is denied", error_type="permission_denied")

        try:
            completed = subprocess.run(
                argv,
                cwd=context.cwd,
                capture_output=True,
                text=True,
                timeout=context.config.tools.max_shell_seconds,
                check=False,
            )
        except subprocess.TimeoutExpired:
            return ToolResult(ok=False, text="Command timed out", error_type="timeout")
        except OSError as error:
            return ToolResult(ok=False, text=str(error), error_type="execution_error")

        text = completed.stdout + completed.stderr
        bounded, truncated = bound_tool_text(text, context.config.tools.max_result_chars)
        return ToolResult(
            ok=completed.returncode == 0,
            text=bounded,
            error_type=None if completed.returncode == 0 else "nonzero_exit",
            truncated=truncated,
        )
