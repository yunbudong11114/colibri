from __future__ import annotations

from dataclasses import dataclass
import re

from colibri.config import AgentConfig
from colibri.messages import Message


_TOPIC_RE = re.compile(r"^[A-Za-z0-9_-]+$")
_TOKEN_RE = re.compile(r"[A-Za-z0-9]+")
_TRUNCATED_SUFFIX = "\n...[truncated]"


@dataclass(frozen=True)
class MemoryTopic:
    name: str
    description: str


@dataclass(frozen=True)
class MemoryRecallResult:
    text: str
    topics: list[str]
    truncated: bool = False


class MemoryRecall:
    def __init__(self, config: AgentConfig):
        self.config = config

    def recall(self, user_text: str, messages: list[Message]) -> MemoryRecallResult:
        if not self.config.memory.enabled:
            return MemoryRecallResult(text="", topics=[])

        topics = self._load_index()
        if not topics:
            return MemoryRecallResult(text="", topics=[])

        query_tokens = self._query_tokens(user_text, messages)
        ranked = self._rank_topics(topics, query_tokens)
        selected = ranked[: self.config.memory.max_recall_topics]
        if not selected:
            return MemoryRecallResult(text="", topics=[])

        blocks: list[str] = ["Relevant memory:"]
        included_topics: list[str] = []
        for topic in selected:
            content = self._read_topic(topic.name)
            if content is None:
                continue
            included_topics.append(topic.name)
            blocks.extend(["", f"[{topic.name}]", content])

        if not included_topics:
            return MemoryRecallResult(text="", topics=[])

        return self._bound_result("\n".join(blocks), included_topics)

    def _load_index(self) -> list[MemoryTopic]:
        index_path = self.config.memory.root.expanduser() / "MEMORY.md"
        try:
            lines = index_path.read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError:
            return []

        topics: list[MemoryTopic] = []
        for line in lines:
            topic = self._parse_index_line(line)
            if topic is not None:
                topics.append(topic)
        return topics

    @staticmethod
    def _parse_index_line(line: str) -> MemoryTopic | None:
        stripped = line.strip()
        if not stripped.startswith("- ") or ":" not in stripped:
            return None
        name, description = stripped[2:].split(":", 1)
        name = name.strip()
        description = description.strip()
        if not _TOPIC_RE.fullmatch(name):
            return None
        return MemoryTopic(name=name, description=description)

    @staticmethod
    def _query_tokens(user_text: str, messages: list[Message]) -> set[str]:
        parts = [user_text]
        parts.extend(message.content for message in messages if message.role in {"user", "assistant"})
        return _tokens("\n".join(parts))

    @staticmethod
    def _rank_topics(topics: list[MemoryTopic], query_tokens: set[str]) -> list[MemoryTopic]:
        scored: list[tuple[int, str, MemoryTopic]] = []
        for topic in topics:
            name_tokens = _tokens(topic.name)
            description_tokens = _tokens(topic.description)
            score = 2 * len(query_tokens & name_tokens) + len(query_tokens & description_tokens)
            if score > 0:
                scored.append((-score, topic.name, topic))
        scored.sort()
        return [topic for _score, _name, topic in scored]

    def _read_topic(self, topic_name: str) -> str | None:
        path = self.config.memory.root.expanduser() / "topics" / f"{topic_name}.md"
        try:
            if not path.is_file():
                return None
            return path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            return None

    def _bound_result(self, text: str, topics: list[str]) -> MemoryRecallResult:
        max_chars = self.config.memory.max_recall_chars
        if len(text) <= max_chars:
            return MemoryRecallResult(text=text, topics=topics, truncated=False)
        keep = max(0, max_chars - len(_TRUNCATED_SUFFIX))
        return MemoryRecallResult(text=text[:keep] + _TRUNCATED_SUFFIX, topics=topics, truncated=True)


def _tokens(text: str) -> set[str]:
    return {match.group(0).lower() for match in _TOKEN_RE.finditer(text) if len(match.group(0)) >= 2}
