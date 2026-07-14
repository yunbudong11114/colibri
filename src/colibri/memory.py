from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from colibri.config import AgentConfig
from colibri.textutil import bound_text


ALWAYS_ON_MEMORY_FILES = ("SOUL.md", "USER.md", "MEMORY.md")
ALWAYS_ON_MEMORY_FILE_LIMITS = {"SOUL.md": 400, "USER.md": 400, "MEMORY.md": 1200}
_BOOTSTRAP_SENTINELS = ("SOUL.md", "USER.md", "MEMORY.md", "INDEX.md")
_SAMPLE_MEMORY_FILES = {
    "SOUL.md": """---
type: soul
description: Colibri 人格、原则和表达风格；首次真实写入时直接覆盖样例文本
updated: 2026-07-14
---

- 用途：记录 Colibri 长期稳定的人格定位、协作原则、表达风格和自我约束。
- 修改规则：只保留真正长期有效的行为准则，保持 400 字符以内；首次真实写入时直接覆盖样例，不要保留原本的示例文本。
""",
    "USER.md": """---
type: user
description: 用户偏好和协作方式；首次真实写入时直接覆盖样例文本
updated: 2026-07-14
---

- 用途：记录用户画像、偏好、称呼、语言风格和协作习惯。
- 修改规则：用户或大模型需要修改用户记忆时，请合并同类偏好并重写本文件，保持简短；首次真实写入时直接覆盖样例，不要保留原本的示例文本。
""",
    "MEMORY.md": """---
type: system
description: Colibri 长期事实和项目上下文；首次真实写入时直接覆盖样例文本
updated: 2026-07-14
---

- 用途：记录稳定事实、项目决策、运行环境和未来对话需要长期记住的上下文。
- 修改规则：用户或大模型需要修改 memory 时，请先去重和合并，再用 `memory.write` 重写本文件；首次真实写入时直接覆盖样例，不要保留原本的示例文本。
""",
    "INDEX.md": """---
type: reference
description: memory topic 索引；首次真实写入时直接覆盖样例文本
updated: 2026-07-14
---

# Memory Index

- [sample](topics/sample.md): sample 示例 topic 详细记忆 写法 维护 memory search index

修改规则：新增或实质修改 `topics/*.md` 时，也要重写本索引中的对应条目。冒号后写多个关键词、别名和描述词，方便 `memory.search` 用子串匹配检索。首次真实写入时直接覆盖样例，不要保留原本的示例文本。
""",
    "topics/sample.md": """---
type: reference
description: 样例详细记忆 topic；首次真实写入时直接覆盖样例文本
updated: 2026-07-14
---

# Sample Topic

- 用途：topic 文件用于保存比 `MEMORY.md` 更长、更细的专项信息，例如设备、项目设计、环境快照或长期任务背景。
- 修改规则：用户或大模型需要修改该 topic 时，请去重、合并、重写相关段落；如果主题说明变化，也要同步更新 `INDEX.md`。首次真实写入时直接覆盖样例，不要保留原本的示例文本。
""",
}

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
        for relative_name, content in _SAMPLE_MEMORY_FILES.items():
            path = root / relative_name
            if path.exists():
                continue
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
    except OSError:
        return
