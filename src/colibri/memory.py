from __future__ import annotations

from dataclasses import dataclass
from importlib import resources
from pathlib import Path

from colibri.config import AgentConfig
from colibri.textutil import bound_text


ALWAYS_ON_MEMORY_FILES = ("SOUL.md", "USER.md", "MEMORY.md")
ALWAYS_ON_MEMORY_FILE_LIMITS = {"SOUL.md": 1000, "USER.md": 1000, "MEMORY.md": 2000}
_BOOTSTRAP_SENTINELS = ("SOUL.md", "USER.md", "MEMORY.md", "INDEX.md")
_SAMPLE_MEMORY_FILES = ("SOUL.md", "USER.md", "MEMORY.md", "INDEX.md", "topics/sample.md")

_MEMORY_LOAD_CACHE: dict[tuple, "MemoryContextResult"] = {}


@dataclass(frozen=True)
class MemoryContextResult:
    text: str
    files: list[str]
    truncated: bool = False


class MemoryContext:
    def __init__(self, config: AgentConfig):
        self.config = config

    def load(self) -> MemoryContextResult:
        if not self.config.memory.enabled:
            return MemoryContextResult(text="", files=[])

        root = self.config.memory.root.expanduser()
        _bootstrap_memory_root(root)
        cache_key = _memory_cache_key(root, self.config.memory.max_recall_chars)
        cached = _MEMORY_LOAD_CACHE.get(cache_key)
        if cached is not None:
            return cached

        blocks: list[str] = ["Always-on memory:"]
        loaded_files: list[str] = []
        any_file_truncated = False

        for filename in ALWAYS_ON_MEMORY_FILES:
            path = root / filename
            try:
                if not path.is_file():
                    continue
                content = path.read_text(encoding="utf-8", errors="replace").strip()
            except OSError:
                continue
            if not content:
                continue
            loaded_files.append(filename)
            content, file_truncated = _bound_file_content(filename, content)
            any_file_truncated = any_file_truncated or file_truncated
            blocks.extend(["", f"[{filename}]", content])

        if not loaded_files:
            result = MemoryContextResult(text="", files=[])
        else:
            result = self._bound_result("\n".join(blocks), loaded_files, any_file_truncated=any_file_truncated)
        _MEMORY_LOAD_CACHE[cache_key] = result
        return result

    def _bound_result(self, text: str, files: list[str], *, any_file_truncated: bool = False) -> MemoryContextResult:
        max_chars = self.config.memory.max_recall_chars
        if len(text) <= max_chars:
            return MemoryContextResult(text=text, files=files, truncated=any_file_truncated)
        return MemoryContextResult(text=bound_text(text, max_chars), files=files, truncated=True)


def _memory_cache_key(root: Path, max_recall_chars: int) -> tuple:
    return (
        str(root),
        max_recall_chars,
        _file_mtime(root / "SOUL.md"),
        _file_mtime(root / "USER.md"),
        _file_mtime(root / "MEMORY.md"),
    )


def _file_mtime(path: Path) -> float | None:
    try:
        return path.stat().st_mtime
    except OSError:
        return None


def _bound_file_content(filename: str, content: str) -> tuple[str, bool]:
    limit = ALWAYS_ON_MEMORY_FILE_LIMITS[filename]
    if len(content) <= limit:
        return content, False
    return bound_text(content, limit), True


def _bootstrap_memory_root(root: Path) -> None:
    try:
        if any((root / name).is_file() for name in _BOOTSTRAP_SENTINELS):
            return
        for relative_name in _SAMPLE_MEMORY_FILES:
            path = root / relative_name
            if path.exists():
                continue
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(_read_template(relative_name), encoding="utf-8")
    except OSError:
        return


def _read_template(relative_name: str) -> str:
    resource = resources.files("colibri.memory_templates")
    for part in relative_name.split("/"):
        resource = resource.joinpath(part)
    return resource.read_text(encoding="utf-8")
