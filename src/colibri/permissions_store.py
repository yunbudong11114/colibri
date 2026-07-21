from __future__ import annotations

from contextlib import contextmanager
from dataclasses import dataclass, field
import fcntl
from pathlib import Path
import tempfile
import tomllib
from typing import Iterator

from colibri.config import expand_user_path

DEFAULT_USER_PERMISSIONS = "~/.colibri/permissions.toml"


@dataclass(frozen=True)
class UserGrants:
    shell_commands: set[str] = field(default_factory=set)
    shell_executables: set[str] = field(default_factory=set)
    tool_names: set[str] = field(default_factory=set)
    file_roots: set[str] = field(default_factory=set)
    hardware_devices: set[str] = field(default_factory=set)


@dataclass(frozen=True)
class _FileFingerprint:
    device: int
    inode: int
    mtime_ns: int
    size: int


class UserPermissionStore:
    def __init__(self, path: Path):
        self.path = path
        self._cache_loaded = False
        self._cached_fingerprint: _FileFingerprint | None = None
        self._cached_grants = UserGrants()

    @classmethod
    def for_user(cls) -> "UserPermissionStore":
        return cls(expand_user_path(DEFAULT_USER_PERMISSIONS))

    @classmethod
    def for_cwd(cls, cwd: Path) -> "UserPermissionStore":
        return cls.for_user()

    def load(self) -> UserGrants:
        fingerprint = self._fingerprint()
        if self._cache_loaded and fingerprint == self._cached_fingerprint:
            return _copy_grants(self._cached_grants)
        grants, fingerprint = self._load_stable()
        self._set_cache(grants, fingerprint)
        return _copy_grants(grants)

    def _load_uncached(self) -> UserGrants:
        try:
            text = self.path.read_text(encoding="utf-8")
        except FileNotFoundError:
            return UserGrants()
        data = tomllib.loads(text)
        shell = data.get("shell", {})
        tools = data.get("tools", {})
        files = data.get("files", {})
        hardware = data.get("hardware", {})
        return UserGrants(
            shell_commands=_string_set(shell.get("commands")),
            shell_executables=_string_set(shell.get("executables")),
            tool_names=_string_set(tools.get("names")),
            file_roots=_string_set(files.get("roots")),
            hardware_devices=_string_set(hardware.get("devices")),
        )

    def save(self, grants: UserGrants) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self._exclusive_lock():
            self._write_atomic(grants)
            self._set_cache(grants, self._fingerprint())

    def merge(self, delta: UserGrants) -> UserGrants:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self._exclusive_lock():
            current = self._load_uncached()
            merged = UserGrants(
                shell_commands=set(current.shell_commands) | set(delta.shell_commands),
                shell_executables=set(current.shell_executables) | set(delta.shell_executables),
                tool_names=set(current.tool_names) | set(delta.tool_names),
                file_roots=set(current.file_roots) | set(delta.file_roots),
                hardware_devices=set(current.hardware_devices) | set(delta.hardware_devices),
            )
            self._write_atomic(merged)
            self._set_cache(merged, self._fingerprint())
        return _copy_grants(merged)

    def _load_stable(self) -> tuple[UserGrants, _FileFingerprint | None]:
        for _ in range(2):
            before = self._fingerprint()
            grants = self._load_uncached()
            after = self._fingerprint()
            if before == after:
                return grants, after
        return self._load_uncached(), self._fingerprint()

    def _fingerprint(self) -> _FileFingerprint | None:
        try:
            stat = self.path.stat()
        except FileNotFoundError:
            return None
        return _FileFingerprint(stat.st_dev, stat.st_ino, stat.st_mtime_ns, stat.st_size)

    def _set_cache(self, grants: UserGrants, fingerprint: _FileFingerprint | None) -> None:
        self._cached_grants = _copy_grants(grants)
        self._cached_fingerprint = fingerprint
        self._cache_loaded = True

    @contextmanager
    def _exclusive_lock(self) -> Iterator[None]:
        lock_path = self.path.with_name(f"{self.path.name}.lock")
        with lock_path.open("a+") as handle:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
            try:
                yield
            finally:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)

    def _write_atomic(self, grants: UserGrants) -> None:
        text = self._format(grants)
        temp_path: Path | None = None
        try:
            with tempfile.NamedTemporaryFile(
                "w",
                encoding="utf-8",
                dir=self.path.parent,
                prefix="permissions.",
                suffix=".tmp",
                delete=False,
            ) as handle:
                handle.write(text)
                handle.flush()
                temp_path = Path(handle.name)
            temp_path.replace(self.path)
        finally:
            if temp_path is not None:
                temp_path.unlink(missing_ok=True)

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
        lines.append("[hardware]")
        lines.append(f"devices = {_toml_string_list(sorted(grants.hardware_devices))}")
        lines.append("")
        return "\n".join(lines)


def _toml_string_list(values: list[str]) -> str:
    escaped = [value.replace("\\", "\\\\").replace('"', '\\"') for value in values]
    return "[" + ", ".join(f'"{value}"' for value in escaped) + "]"


def _string_set(value: object) -> set[str]:
    if not isinstance(value, list):
        return set()
    return {item for item in value if isinstance(item, str)}


def _copy_grants(grants: UserGrants) -> UserGrants:
    return UserGrants(
        shell_commands=set(grants.shell_commands),
        shell_executables=set(grants.shell_executables),
        tool_names=set(grants.tool_names),
        file_roots=set(grants.file_roots),
        hardware_devices=set(grants.hardware_devices),
    )
