from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from subprocess import TimeoutExpired, run
from typing import Any
import re
import tomllib

from colibri.config import SkillsConfig
from colibri.tools.base import ToolContext, ToolResult, ToolSpec, bound_tool_text


CREATE_COLIBRI_SKILL_CONTENT = """# Create Colibri Skill

Use this skill when the user wants to create, write, add, or design a local Colibri skill.

Colibri skills are local filesystem instructions. Do not install packages, fetch remote skill catalogs, or use a marketplace.

Create this layout:

```text
~/.colibri/skills/<skill-name>/
  SKILL.md
  skill.toml        # optional
  scripts/...       # optional
```

`SKILL.md` is required. Keep it focused on when to use the skill, what context to gather, and the exact workflow the assistant should follow. Prefer progressive disclosure: put the essential instructions in `SKILL.md`, and reference extra local files only when needed.

Optional `skill.toml` can describe local commands for `skill.run`:

```toml
description = "Short description used for skill selection."

[[commands]]
name = "check"
description = "Run the local verification command."
command = "python"
args = ["scripts/check.py"]
read_only = true
```

After creating a skill, test that Colibri selects it for a matching user request and does not select it for unrelated turns. Keep command permissions explicit and avoid long resident processes on small devices.
"""


@dataclass(frozen=True)
class SkillCommand:
    name: str
    description: str = ""
    command: str = ""
    args: list[str] = field(default_factory=list)
    read_only: bool = False


@dataclass(frozen=True)
class SkillMetadata:
    name: str
    description: str
    root: Path
    skill_file: Path
    commands: list[SkillCommand] = field(default_factory=list)
    content: str | None = None


@dataclass(frozen=True)
class SkillContext:
    text: str
    skills: list[str]
    truncated: bool = False


class SkillIndex:
    def __init__(self, skills: list[SkillMetadata]):
        self.skills = skills
        self._by_name = {skill.name: skill for skill in skills}

    @classmethod
    def scan(cls, dirs: list[Path]) -> "SkillIndex":
        skills: list[SkillMetadata] = _builtin_skills()
        seen: set[str] = {skill.name for skill in skills}
        for root in dirs:
            try:
                entries = sorted(root.iterdir(), key=lambda path: path.name)
            except OSError:
                continue
            for entry in entries:
                if not entry.is_dir():
                    continue
                name = entry.name
                if name in seen:
                    continue
                skill_file = entry / "SKILL.md"
                try:
                    first_text = skill_file.read_text(encoding="utf-8")
                except OSError:
                    continue
                metadata = _read_skill_toml(entry)
                description = str(metadata.get("description") or _derive_description(first_text) or name)
                commands = _parse_commands(metadata)
                skills.append(
                    SkillMetadata(
                        name=name,
                        description=description,
                        root=entry,
                        skill_file=skill_file,
                        commands=commands,
                    )
                )
                seen.add(name)
        return cls(skills)

    def get(self, name: str) -> SkillMetadata | None:
        return self._by_name.get(name)

    def context_for(self, user_text: str, config: SkillsConfig) -> SkillContext:
        selected = self.select(user_text, config.max_loaded)
        if not selected:
            return SkillContext(text="", skills=[])

        chunks: list[str] = ["Relevant skills:"]
        truncated = False
        for skill in selected:
            content = skill.content
            if content is None:
                try:
                    content = skill.skill_file.read_text(encoding="utf-8")
                except OSError:
                    continue
            chunks.append(f"\n[{skill.name}]\nBase directory: {skill.root}\n\n{content.strip()}")
        text = "\n".join(chunks).strip()
        if not text:
            return SkillContext(text="", skills=[])
        if len(text) > config.max_instruction_chars:
            text, truncated = _bound_skill_context(text, config.max_instruction_chars)
        return SkillContext(text=text, skills=[skill.name for skill in selected], truncated=truncated)

    def select(self, user_text: str, limit: int) -> list[SkillMetadata]:
        if limit <= 0:
            return []
        query_terms = _terms(user_text)
        scored: list[tuple[int, str, SkillMetadata]] = []
        for skill in self.skills:
            score = _skill_score(skill, query_terms, user_text)
            if score:
                scored.append((score, skill.name, skill))
        scored.sort(key=lambda item: (-item[0], item[1]))
        return [skill for _score, _name, skill in scored[:limit]]


class SkillRunTool:
    spec = ToolSpec(
        name="skill.run",
        description="Run a configured local skill command.",
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

        index = SkillIndex.scan(context.config.skills.dirs)
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


def _read_skill_toml(root: Path) -> dict[str, Any]:
    path = root / "skill.toml"
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError):
        return {}


def _parse_commands(metadata: dict[str, Any]) -> list[SkillCommand]:
    commands = metadata.get("commands", [])
    if not isinstance(commands, list):
        return []
    parsed: list[SkillCommand] = []
    for item in commands:
        if not isinstance(item, dict):
            continue
        name = item.get("name")
        command = item.get("command")
        if not isinstance(name, str) or not isinstance(command, str):
            continue
        args = item.get("args", [])
        if not isinstance(args, list) or not all(isinstance(arg, str) for arg in args):
            args = []
        parsed.append(
            SkillCommand(
                name=name,
                description=str(item.get("description") or ""),
                command=command,
                args=args,
                read_only=bool(item.get("read_only", False)),
            )
        )
    return parsed


def _builtin_skills() -> list[SkillMetadata]:
    name = "create-colibri-skill"
    root = Path("builtin")
    return [
        SkillMetadata(
            name=name,
            description="Guide creating, writing, adding, or designing a local Colibri skill.",
            root=root,
            skill_file=root / name / "SKILL.md",
            content=CREATE_COLIBRI_SKILL_CONTENT,
        )
    ]


def _skill_score(skill: SkillMetadata, query_terms: set[str], user_text: str) -> int:
    if skill.name == "create-colibri-skill":
        if not _is_create_skill_request(user_text):
            return 0
        return 100 + len(query_terms & _terms(f"{skill.name} {skill.description}"))
    haystack = _terms(f"{skill.name} {skill.description}")
    return len(query_terms & haystack)


def _is_create_skill_request(user_text: str) -> bool:
    lowered = user_text.lower()
    has_skill_term = bool({"skill", "skills"} & _terms(lowered)) or "技能" in lowered
    if not has_skill_term:
        return False
    create_words = (
        "create",
        "new",
        "add",
        "write",
        "design",
        "build",
        "创建",
        "新增",
        "添加",
        "编写",
        "设计",
    )
    return any(word in lowered for word in create_words)


def _derive_description(content: str) -> str:
    for line in content.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        if stripped.startswith("#"):
            return stripped.lstrip("#").strip()
        return stripped
    return ""


def _terms(text: str) -> set[str]:
    return {term for term in re.findall(r"[A-Za-z0-9_]+", text.lower()) if len(term) > 1}


def _bound_skill_context(text: str, max_chars: int) -> tuple[str, bool]:
    suffix = "\n...[truncated]"
    keep = max(0, max_chars - len(suffix))
    return text[:keep] + suffix, True
