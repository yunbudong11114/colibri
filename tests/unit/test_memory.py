from colibri.config import AgentConfig
from colibri.memory import MemoryContext


def make_memory_config(tmp_path, **memory_overrides):
    data = {
        "memory": {
            "root": str(tmp_path / "memory"),
            "max_recall_chars": 4000,
        }
    }
    data["memory"].update(memory_overrides)
    return AgentConfig.default().with_overrides(data)


def write_memory_layout(tmp_path):
    root = tmp_path / "memory"
    topics = root / "topics"
    topics.mkdir(parents=True)
    (root / "MEMORY.md").write_text("# Memory\n\n- Colibri runs on CardputerZero.\n", encoding="utf-8")
    (root / "USER.md").write_text("# User\n\n- Prefers concise Chinese answers.\n", encoding="utf-8")
    (root / "INDEX.md").write_text(
        "# Memory Index\n\n- [system-info](topics/system-info.md): Machine info.\n",
        encoding="utf-8",
    )
    (topics / "system-info.md").write_text("- macOS on Apple Silicon.\n", encoding="utf-8")


def test_context_loads_memory_and_user_files(tmp_path):
    write_memory_layout(tmp_path)
    context = MemoryContext(make_memory_config(tmp_path))

    result = context.load()

    assert result.files == ["MEMORY.md", "USER.md"]
    assert "Always-on memory:" in result.text
    assert "[MEMORY.md]" in result.text
    assert "Colibri runs on CardputerZero" in result.text
    assert "[USER.md]" in result.text
    assert "Prefers concise Chinese answers" in result.text
    assert not result.truncated


def test_context_bootstraps_sample_files_when_memory_root_is_missing(tmp_path):
    root = tmp_path / "memory"
    context = MemoryContext(make_memory_config(tmp_path))

    result = context.load()

    assert (root / "MEMORY.md").is_file()
    assert (root / "USER.md").is_file()
    assert (root / "INDEX.md").is_file()
    assert (root / "topics" / "sample.md").is_file()
    assert "type: system" in (root / "MEMORY.md").read_text(encoding="utf-8")
    assert "type: user" in (root / "USER.md").read_text(encoding="utf-8")
    memory_text = (root / "MEMORY.md").read_text(encoding="utf-8")
    user_text = (root / "USER.md").read_text(encoding="utf-8")
    index_text = (root / "INDEX.md").read_text(encoding="utf-8")
    topic_text = (root / "topics" / "sample.md").read_text(encoding="utf-8")
    assert "首次真实写入时直接覆盖样例" in memory_text
    assert "首次真实写入时直接覆盖样例" in user_text
    assert "首次真实写入时直接覆盖样例" in index_text
    assert "首次真实写入时直接覆盖样例" in topic_text
    assert "topics/sample.md" in index_text
    assert ": sample 示例 topic 详细记忆 写法 维护 memory search index" in index_text
    assert "Always-on memory:" in result.text


def test_context_bootstraps_sample_files_when_memory_root_has_no_files(tmp_path):
    root = tmp_path / "memory"
    (root / "topics").mkdir(parents=True)
    context = MemoryContext(make_memory_config(tmp_path))

    context.load()

    assert (root / "MEMORY.md").is_file()
    assert (root / "USER.md").is_file()
    assert (root / "INDEX.md").is_file()
    assert (root / "topics" / "sample.md").is_file()


def test_context_bootstrap_does_not_overwrite_existing_memory_files(tmp_path):
    root = tmp_path / "memory"
    root.mkdir(parents=True)
    (root / "MEMORY.md").write_text("custom memory", encoding="utf-8")
    context = MemoryContext(make_memory_config(tmp_path))

    result = context.load()

    assert (root / "MEMORY.md").read_text(encoding="utf-8") == "custom memory"
    assert not (root / "USER.md").exists()
    assert result.files == ["MEMORY.md"]


def test_context_does_not_inject_index_or_topics(tmp_path):
    write_memory_layout(tmp_path)
    context = MemoryContext(make_memory_config(tmp_path))

    result = context.load()

    assert "INDEX.md" not in result.text
    assert "system-info.md" not in result.text
    assert "macOS on Apple Silicon" not in result.text


def test_context_obeys_character_budget(tmp_path):
    root = tmp_path / "memory"
    root.mkdir(parents=True)
    (root / "MEMORY.md").write_text("M" * 80, encoding="utf-8")
    (root / "USER.md").write_text("U" * 80, encoding="utf-8")
    context = MemoryContext(make_memory_config(tmp_path, max_recall_chars=70))

    result = context.load()

    assert len(result.text) == 70
    assert result.truncated
    assert result.text.endswith("\n...[truncated]")


def test_context_truncates_without_write_guidance_when_always_on_files_exceed_file_limits(tmp_path):
    root = tmp_path / "memory"
    root.mkdir(parents=True)
    (root / "MEMORY.md").write_text("M" * 1810, encoding="utf-8")
    (root / "USER.md").write_text("U" * 610, encoding="utf-8")
    context = MemoryContext(make_memory_config(tmp_path, max_recall_chars=4000))

    result = context.load()

    assert "MEMORY.md exceeds 1800 characters" not in result.text
    assert "USER.md exceeds 600 characters" not in result.text
    assert 'memory.write' not in result.text
    assert 'mode="replace"' not in result.text
    assert result.truncated


def test_context_disabled_returns_empty_result(tmp_path):
    write_memory_layout(tmp_path)
    context = MemoryContext(make_memory_config(tmp_path, enabled=False))

    result = context.load()

    assert result.text == ""
    assert result.files == []
    assert not result.truncated


def test_context_disabled_does_not_bootstrap_missing_files(tmp_path):
    context = MemoryContext(make_memory_config(tmp_path, enabled=False))

    result = context.load()

    assert result.text == ""
    assert result.files == []
    assert not (tmp_path / "memory").exists()
