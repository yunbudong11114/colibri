from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
import tempfile
import tomllib

from colibri.config import expand_user_path

DEFAULT_USER_PERMISSIONS = "~/.colibri/permissions.toml"


@dataclass(frozen=True)
class UserGrants:
    shell_commands: set[str] = field(default_factory=set)
    shell_executables: set[str] = field(default_factory=set)
    tool_names: set[str] = field(default_factory=set)
    file_roots: set[str] = field(default_factory=set)


class UserPermissionStore:
    def __init__(self, path: Path):
        self.path = path

    @classmethod
    def for_user(cls) -> "UserPermissionStore":
        return cls(expand_user_path(DEFAULT_USER_PERMISSIONS))

    @classmethod
    def for_cwd(cls, cwd: Path) -> "UserPermissionStore":
        return cls.for_user()

    def load(self) -> UserGrants:
        if not self.path.exists():
            return UserGrants()
        data = tomllib.loads(self.path.read_text(encoding="utf-8"))
        shell = data.get("shell", {})
        tools = data.get("tools", {})
        files = data.get("files", {})
        shell_executables = _string_set(shell.get("executables")) | _string_set(shell.get("prefixes"))
        return UserGrants(
            shell_commands=_string_set(shell.get("commands")),
            shell_executables=shell_executables,
            tool_names=_string_set(tools.get("names")),
            file_roots=_string_set(files.get("roots")),
        )

    def save(self, grants: UserGrants) -> None:
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

    def _format(self, grants: UserGrants) -> str:
        lines = ["[shell]"]
        lines.append(f"commands = {_toml_string_list(sorted(grants.shell_commands))}")
        lines.append(f"executables = {_toml_string_list(sorted(grants.shell_executables))}")
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


def _string_set(value: object) -> set[str]:
    if not isinstance(value, list):
        return set()
    return {item for item in value if isinstance(item, str)}
