from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
import yaml

from colibri.config import SkillsConfig
from colibri.textutil import bound_text


CREATE_COLIBRI_SKILL_CONTENT = """---
name: create-colibri-skill
description: Guide creating, writing, adding, or designing a local Colibri skill.
---

# Create Colibri Skill

Use this skill when the user wants to create, write, add, or design a local Colibri skill.

Colibri skills are local filesystem instructions. Do not install packages, fetch remote skill catalogs, or use a marketplace.

Create this layout:

```text
~/.colibri/skills/<skill-name>/
  SKILL.md
  scripts/...       # optional
```

`SKILL.md` is required and must start with YAML frontmatter. The `name` must match the skill directory name and `description` must clearly state when the skill is useful:

```yaml
---
name: example-skill
description: Use when the user needs the example workflow.
commands:
  - name: check
    description: Run the local verification command.
    command: python
    args: [scripts/check.py]
    read_only: true
---
```

Keep the Markdown body focused on context gathering and the exact workflow. Prefer progressive disclosure and reference extra local files only when needed. Put reusable implementations under `scripts/`. Do not create `skill.toml`.

After creating a skill, Colibri lists it in the skill catalog. Use `skill.read` with the skill name when you need the full instructions. When a declared command matches the requested action, execute it with `skill.run`. Keep command permissions explicit and avoid long resident processes on small devices.
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
    def scan(cls, skill_dir: Path) -> "SkillIndex":
        fingerprint = _dir_fingerprint(skill_dir)
        cached = _SKILL_SCAN_CACHE.get(fingerprint)
        if cached is not None:
            return cached

        skills: list[SkillMetadata] = _builtin_skills()
        seen: set[str] = {skill.name for skill in skills}
        try:
            entries = sorted(skill_dir.iterdir(), key=lambda path: path.name)
        except OSError:
            entries = []
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
            parsed = _parse_skill_document(first_text, name)
            if parsed is None:
                continue
            description, commands = parsed
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
        index = cls(skills)
        _SKILL_SCAN_CACHE[fingerprint] = index
        return index

    def get(self, name: str) -> SkillMetadata | None:
        return self._by_name.get(name)

    def catalog(self, config: SkillsConfig) -> SkillContext:
        if config.max_catalog <= 0:
            return SkillContext(text="", skills=[])

        selected = self.skills[: config.max_catalog]
        if not selected:
            return SkillContext(text="", skills=[])

        lines = [
            "Available skills:",
            "",
            (
                "Use skill.read to load full instructions. When a skill has a configured command "
                "matching the requested action, use skill.run instead of invoking that command's "
                "underlying executable through shell.run."
            ),
            "",
        ]
        for skill in selected:
            location = "builtin" if skill.root.name == "builtin" and skill.content is not None else str(skill.root)
            command_text = (
                f" Commands: {', '.join(command.name for command in skill.commands)}"
                if skill.commands
                else ""
            )
            lines.append(f"- {skill.name}: {skill.description}{command_text} [{location}]")
        text = "\n".join(lines).strip()
        truncated = False
        if len(text) > config.max_catalog_chars:
            text, truncated = bound_text(text, config.max_catalog_chars), True
        return SkillContext(text=text, skills=[skill.name for skill in selected], truncated=truncated)

    def read_text(self, name: str, max_chars: int) -> tuple[str | None, bool]:
        skill = self.get(name)
        if skill is None:
            return None, False
        content = skill.content
        if content is None:
            try:
                content = skill.skill_file.read_text(encoding="utf-8")
            except OSError:
                return None, False
        command_text = ""
        if skill.commands:
            command_lines = [
                f"- {command.name}: {command.description}".rstrip(": ")
                for command in skill.commands
            ]
            command_text = "\nConfigured commands:\n" + "\n".join(command_lines)
        text = (
            f"[{skill.name}]\nBase directory: {skill.root}"
            f"{command_text}\n\n{content.strip()}"
        )
        if len(text) > max_chars:
            return bound_text(text, max_chars), True
        return text, False


_SKILL_SCAN_CACHE: dict[tuple, SkillIndex] = {}


def _dir_fingerprint(skill_dir: Path) -> tuple:
    parts: list[tuple[str, float | None]] = []
    try:
        resolved = str(skill_dir.expanduser().resolve())
    except OSError:
        resolved = str(skill_dir.expanduser())
    parts.append((resolved, None))
    try:
        entries = sorted(skill_dir.expanduser().iterdir(), key=lambda path: path.name)
    except OSError:
        return tuple(parts)
    for entry in entries:
        if not entry.is_dir():
            continue
        path = entry / "SKILL.md"
        try:
            parts.append((str(path.resolve()), path.stat().st_mtime))
        except OSError:
            parts.append((str(path), None))
    return tuple(parts)


def _parse_skill_document(content: str, expected_name: str) -> tuple[str, list[SkillCommand]] | None:
    lines = content.splitlines()
    if not lines or lines[0].strip() != "---":
        return None
    try:
        closing = next(index for index, line in enumerate(lines[1:], start=1) if line.strip() == "---")
    except StopIteration:
        return None
    try:
        metadata = yaml.safe_load("\n".join(lines[1:closing]))
    except yaml.YAMLError:
        return None
    if not isinstance(metadata, dict):
        return None
    name = metadata.get("name")
    description = metadata.get("description")
    if not isinstance(name, str) or name.strip() != expected_name:
        return None
    if not isinstance(description, str) or not description.strip():
        return None
    commands = metadata.get("commands", [])
    if not isinstance(commands, list):
        return None
    parsed: list[SkillCommand] = []
    seen: set[str] = set()
    for item in commands:
        if not isinstance(item, dict):
            return None
        name = item.get("name")
        command = item.get("command")
        if (
            not isinstance(name, str)
            or not name.strip()
            or name in seen
            or not isinstance(command, str)
            or not command.strip()
        ):
            return None
        args = item.get("args", [])
        if not isinstance(args, list) or not all(isinstance(arg, str) for arg in args):
            return None
        command_description = item.get("description", "")
        read_only = item.get("read_only", False)
        if not isinstance(command_description, str) or not isinstance(read_only, bool):
            return None
        parsed.append(
            SkillCommand(
                name=name,
                description=command_description,
                command=command,
                args=args,
                read_only=read_only,
            )
        )
        seen.add(name)
    return description, parsed


def _builtin_skills() -> list[SkillMetadata]:
    name = "create-colibri-skill"
    root = Path("builtin")
    parsed = _parse_skill_document(CREATE_COLIBRI_SKILL_CONTENT, name)
    if parsed is None:
        return []
    description, commands = parsed
    return [
        SkillMetadata(
            name=name,
            description=description,
            root=root,
            skill_file=root / name / "SKILL.md",
            commands=commands,
            content=CREATE_COLIBRI_SKILL_CONTENT,
        )
    ]
