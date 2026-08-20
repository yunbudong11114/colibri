from __future__ import annotations

import os
import shlex
from typing import Any

from colibri.tools.base import ToolContext, ToolResult, ToolSpec
from colibri.tools.process_output import ProcessTimedOut, run_bounded_process
from colibri.tools.shell_policy import denied_shell_executable


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

        if denied_shell_executable(command, set(context.config.shell.deny)) is not None:
            return ToolResult(ok=False, text="Command is denied", error_type="permission_denied")

        popen_args: str | list[str] = command if os.name == "nt" else ["/bin/sh", "-c", command]
        try:
            completed = run_bounded_process(
                popen_args,
                cwd=context.cwd,
                timeout_seconds=context.config.tools.max_shell_seconds,
                max_result_chars=context.config.tools.max_result_chars,
                shell=os.name == "nt",
            )
        except ProcessTimedOut:
            return ToolResult(ok=False, text="Command timed out", error_type="timeout")
        except OSError as error:
            return ToolResult(ok=False, text=str(error), error_type="execution_error")

        return ToolResult(
            ok=completed.returncode == 0,
            text=completed.text,
            error_type=None if completed.returncode == 0 else "nonzero_exit",
            truncated=completed.truncated,
        )
