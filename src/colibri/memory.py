from __future__ import annotations

from dataclasses import dataclass

from colibri.config import AgentConfig


ALWAYS_ON_MEMORY_FILES = ("MEMORY.md", "USER.md")
ALWAYS_ON_MEMORY_FILE_LIMITS = {"MEMORY.md": 1800, "USER.md": 600}
_TRUNCATED_SUFFIX = "\n...[truncated]"


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
            return MemoryContextResult(text="", files=[])

        return self._bound_result("\n".join(blocks), loaded_files, any_file_truncated=any_file_truncated)

    def _bound_result(self, text: str, files: list[str], *, any_file_truncated: bool = False) -> MemoryContextResult:
        max_chars = self.config.memory.max_recall_chars
        if len(text) <= max_chars:
            return MemoryContextResult(text=text, files=files, truncated=any_file_truncated)
        keep = max(0, max_chars - len(_TRUNCATED_SUFFIX))
        return MemoryContextResult(text=text[:keep] + _TRUNCATED_SUFFIX, files=files, truncated=True)


def _bound_file_content(filename: str, content: str) -> tuple[str, bool]:
    limit = ALWAYS_ON_MEMORY_FILE_LIMITS[filename]
    if len(content) <= limit:
        return content, False
    keep = max(0, limit - len(_TRUNCATED_SUFFIX))
    return content[:keep] + _TRUNCATED_SUFFIX, True
