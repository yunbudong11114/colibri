from __future__ import annotations

from subprocess import TimeoutExpired, run
from typing import Any

from colibri.skills import SkillIndex
from colibri.tools.base import ToolContext, ToolResult, ToolSpec, bound_tool_text


class SkillReadTool:
    spec = ToolSpec(
        name="skill.read",
        description=(
            "Read the full SKILL.md instructions for a skill listed in the catalog. "
            "Prefer this over guessing skill contents."
        ),
        input_schema={
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Exact skill name from the catalog."},
            },
            "required": ["name"],
        },
        read_only=True,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        name = arguments.get("name")
        if not isinstance(name, str) or not name.strip():
            return ToolResult(ok=False, text="name is required", error_type="invalid_arguments")

        index = SkillIndex.scan(context.config.skills.dir)
        text, truncated = index.read_text(name.strip(), context.config.skills.max_instruction_chars)
        if text is None:
            available = ", ".join(skill.name for skill in index.skills[:20]) or "none"
            return ToolResult(
                ok=False,
                text=f"Unknown skill: {name.strip()}. Available: {available}",
                error_type="not_found",
            )
        return ToolResult(ok=True, text=text, truncated=truncated)


class SkillRunTool:
    spec = ToolSpec(
        name="skill.run",
        description=(
            "Run a command declared in a skill's SKILL.md YAML frontmatter. "
            "Prefer this tool whenever a configured command matches the requested action; "
            "do not invoke the underlying executable through shell.run."
        ),
        input_schema={
            "type": "object",
            "properties": {
                "skill": {"type": "string"},
                "command": {"type": "string"},
                "args": {"type": "array", "items": {"type": "string"}},
            },
            "required": ["skill", "command"],
        },
        read_only=False,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        skill_name = arguments.get("skill")
        command_name = arguments.get("command")
        extra_args = arguments.get("args", [])
        if not isinstance(skill_name, str) or not isinstance(command_name, str):
            return ToolResult(ok=False, text="skill and command are required", error_type="invalid_arguments")
        if not isinstance(extra_args, list) or not all(isinstance(arg, str) for arg in extra_args):
            return ToolResult(ok=False, text="args must be a list of strings", error_type="invalid_arguments")

        index = SkillIndex.scan(context.config.skills.dir)
        skill = index.get(skill_name)
        if skill is None:
            return ToolResult(ok=False, text=f"Unknown skill: {skill_name}", error_type="not_found")
        command = next((item for item in skill.commands if item.name == command_name), None)
        if command is None:
            return ToolResult(ok=False, text=f"Unknown skill command: {command_name}", error_type="not_found")
        if not command.command:
            return ToolResult(ok=False, text="Skill command is empty", error_type="invalid_config")

        try:
            completed = run(
                [command.command, *command.args, *extra_args],
                cwd=skill.root,
                text=True,
                capture_output=True,
                timeout=context.config.tools.max_shell_seconds,
                check=False,
            )
        except TimeoutExpired as error:
            text = error.stdout or error.stderr or "Skill command timed out"
            bounded, truncated = bound_tool_text(str(text), context.config.tools.max_result_chars)
            return ToolResult(ok=False, text=bounded, error_type="timeout", truncated=truncated)
        except OSError as error:
            return ToolResult(ok=False, text=str(error), error_type="tool_error")

        output = completed.stdout if completed.stdout else completed.stderr
        bounded, truncated = bound_tool_text(output, context.config.tools.max_result_chars)
        if completed.returncode != 0:
            return ToolResult(ok=False, text=bounded, error_type="tool_error", truncated=truncated)
        return ToolResult(ok=True, text=bounded, truncated=truncated)
