from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
import tempfile
import tomllib


@dataclass(frozen=True)
class ProjectGrants:
    shell_commands: set[str] = field(default_factory=set)
    tool_names: set[str] = field(default_factory=set)
    file_roots: set[str] = field(default_factory=set)


class ProjectPermissionStore:
    def __init__(self, path: Path):
        self.path = path

    @classmethod
    def for_cwd(cls, cwd: Path) -> "ProjectPermissionStore":
        return cls(cwd / ".colibri" / "permissions.toml")

    def load(self) -> ProjectGrants:
        if not self.path.exists():
            return ProjectGrants()
        data = tomllib.loads(self.path.read_text(encoding="utf-8"))
        shell = data.get("shell", {})
        tools = data.get("tools", {})
        files = data.get("files", {})
        return ProjectGrants(
            shell_commands={item for item in shell.get("commands", []) if isinstance(item, str)},
            tool_names={item for item in tools.get("names", []) if isinstance(item, str)},
            file_roots={item for item in files.get("roots", []) if isinstance(item, str)},
        )

    def save(self, grants: ProjectGrants) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        text = self._format(grants)
        with tempfile.NamedTemporaryFile(
            "w",
            encoding="utf-8",
            dir=self.path.parent,
            prefix="permissions.",
            suffix=".tmp",
            delete=False,
        ) as handle:
            handle.write(text)
            temp_path = Path(handle.name)
        temp_path.replace(self.path)

    def _format(self, grants: ProjectGrants) -> str:
        lines = ["[shell]"]
        lines.append(f"commands = {_toml_string_list(sorted(grants.shell_commands))}")
        lines.append("")
        lines.append("[tools]")
        lines.append(f"names = {_toml_string_list(sorted(grants.tool_names))}")
        lines.append("")
        lines.append("[files]")
        lines.append(f"roots = {_toml_string_list(sorted(grants.file_roots))}")
        lines.append("")
        return "\n".join(lines)


def _toml_string_list(values: list[str]) -> str:
    escaped = [value.replace("\\", "\\\\").replace('"', '\\"') for value in values]
    return "[" + ", ".join(f'"{value}"' for value in escaped) + "]"
