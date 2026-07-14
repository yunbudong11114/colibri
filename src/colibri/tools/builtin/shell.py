from __future__ import annotations

import os
import shlex
import signal
import subprocess
from typing import Any

from colibri.tools.base import ToolContext, ToolResult, ToolSpec, bound_tool_text
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

        if os.name == "nt":
            popen_args: str | list[str] = command
            popen_kwargs: dict[str, Any] = {"shell": True}
        else:
            popen_args = ["/bin/sh", "-c", command]
            popen_kwargs = {"start_new_session": True}
        try:
            process = subprocess.Popen(
                popen_args,
                cwd=context.cwd,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                **popen_kwargs,
            )
            stdout, stderr = process.communicate(timeout=context.config.tools.max_shell_seconds)
        except subprocess.TimeoutExpired:
            if os.name == "nt":
                process.kill()
            else:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            process.communicate()
            return ToolResult(ok=False, text="Command timed out", error_type="timeout")
        except OSError as error:
            return ToolResult(ok=False, text=str(error), error_type="execution_error")

        text = stdout + stderr
        bounded, truncated = bound_tool_text(text, context.config.tools.max_result_chars)
        return ToolResult(
            ok=process.returncode == 0,
            text=bounded,
            error_type=None if process.returncode == 0 else "nonzero_exit",
            truncated=truncated,
        )
