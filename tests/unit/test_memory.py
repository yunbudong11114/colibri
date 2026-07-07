from colibri.config import AgentConfig
from colibri.memory import MemoryRecall
from colibri.messages import Message


def make_memory_config(tmp_path, **memory_overrides):
    data = {
        "memory": {
            "root": str(tmp_path / "memory"),
            "max_recall_topics": 3,
            "max_recall_chars": 4000,
        }
    }
    data["memory"].update(memory_overrides)
    return AgentConfig.default().with_overrides(data)


def write_memory(tmp_path):
    root = tmp_path / "memory"
    topics = root / "topics"
    topics.mkdir(parents=True)
    (root / "MEMORY.md").write_text(
        "\n".join(
            [
                "# Memory Index",
                "- devices: Home router wifi and hostnames.",
                "- preferences: User tone and recurring constraints.",
                "- bad/topic: invalid entry",
                "- malformed without colon",
            ]
        ),
        encoding="utf-8",
    )
    (topics / "devices.md").write_text("- Router is upstairs.\n", encoding="utf-8")
    (topics / "preferences.md").write_text("- Keep answers concise.\n", encoding="utf-8")


def test_recall_selects_topics_by_keyword_overlap(tmp_path):
    write_memory(tmp_path)
    config = make_memory_config(tmp_path)
    recall = MemoryRecall(config)

    result = recall.recall("what is the router hostname?", [])

    assert result.topics == ["devices"]
    assert result.text == "Relevant memory:\n\n[devices]\n- Router is upstairs.\n"
    assert not result.truncated


def test_recall_uses_recent_messages_and_skips_invalid_index_lines(tmp_path):
    write_memory(tmp_path)
    config = make_memory_config(tmp_path)
    recall = MemoryRecall(config)

    result = recall.recall("what should I do?", [Message(role="user", content="remember my tone preference")])

    assert result.topics == ["preferences"]
    assert "bad/topic" not in result.text


def test_recall_obeys_topic_limit_and_character_budget(tmp_path):
    write_memory(tmp_path)
    config = make_memory_config(tmp_path, max_recall_topics=1, max_recall_chars=48)
    recall = MemoryRecall(config)

    result = recall.recall("wifi router hostnames", [])

    assert result.topics == ["devices"]
    assert result.truncated
    assert result.text.endswith("\n...[truncated]")


def test_recall_disabled_returns_empty_result(tmp_path):
    write_memory(tmp_path)
    config = make_memory_config(tmp_path, enabled=False)
    recall = MemoryRecall(config)

    result = recall.recall("router", [])

    assert result.text == ""
    assert result.topics == []
    assert not result.truncated


def test_recall_missing_files_is_non_fatal(tmp_path):
    config = make_memory_config(tmp_path)
    recall = MemoryRecall(config)

    result = recall.recall("router", [])

    assert result.text == ""
    assert result.topics == []
