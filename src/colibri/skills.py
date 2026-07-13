from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any
import tomllib

from colibri.config import SkillsConfig
from colibri.textutil import bound_text


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
description = "Short description shown in the skill catalog."

[[commands]]
name = "check"
description = "Run the local verification command."
command = "python"
args = ["scripts/check.py"]
read_only = true
```

After creating a skill, Colibri lists it in the skill catalog. Use `skill.read` with the skill name when you need the full instructions. Keep command permissions explicit and avoid long resident processes on small devices.
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

        lines = ["Available skills (use skill.read with name when needed):", ""]
        for skill in selected:
            location = "[builtin]" if skill.root.name == "builtin" and skill.content is not None else str(skill.root)
            lines.append(f"- {skill.name}: {skill.description} [{location}]")
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
        text = f"[{skill.name}]\nBase directory: {skill.root}\n\n{content.strip()}"
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
        for name in ("SKILL.md", "skill.toml"):
            path = entry / name
            try:
                parts.append((str(path.resolve()), path.stat().st_mtime))
            except OSError:
                parts.append((str(path), None))
    return tuple(parts)


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


def _derive_description(content: str) -> str:
    for line in content.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        if stripped.startswith("#"):
            return stripped.lstrip("#").strip()
        return stripped
    return ""
