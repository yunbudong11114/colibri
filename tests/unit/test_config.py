from pathlib import Path

from colibri.config import AgentConfig, expand_user_path


def test_default_config_uses_small_device_limits():
    config = AgentConfig.default()

    assert config.model.provider == "fake"
    assert config.model.model == "fake-colibri-model"
    assert config.session.max_tool_rounds == 6
    assert config.session.recent_message_limit == 16
    assert config.session.compact_trigger_chars == 36000
    assert config.session.model_compact
    assert config.tools.max_result_chars == 12000
    assert config.skills.max_loaded == 3
    assert config.skills.max_instruction_chars == 6000
    assert config.console.status
    assert config.shell.deny[:3] == ["rm", "shutdown", "reboot"]


def test_load_config_overrides_nested_values(tmp_path):
    config_path = tmp_path / "agent.toml"
    config_path.write_text(
        """
[model]
provider = "openai_compatible"
model = "gpt-4.1-mini"
timeout_seconds = 45

[session]
recent_message_limit = 8
model_compact = false

[files]
roots = ["~/notes", "/tmp"]

[skills]
dirs = ["~/skills"]
max_loaded = 2
max_instruction_chars = 1234

[console]
status = false
""".strip(),
        encoding="utf-8",
    )

    config = AgentConfig.load(config_path)

    assert config.model.provider == "openai_compatible"
    assert config.model.model == "gpt-4.1-mini"
    assert config.model.timeout_seconds == 45
    assert config.session.recent_message_limit == 8
    assert not config.session.model_compact
    assert config.files.roots[0].name == "notes"
    assert config.files.roots[1] == Path("/tmp")
    assert config.skills.dirs[0].name == "skills"
    assert config.skills.max_loaded == 2
    assert config.skills.max_instruction_chars == 1234
    assert not config.console.status


def test_load_without_path_reads_user_default_config(monkeypatch, tmp_path):
    home = tmp_path / "home"
    config_dir = home / ".colibri"
    config_dir.mkdir(parents=True)
    (config_dir / "config.toml").write_text(
        """
[model]
provider = "openai_compatible"
model = "user-default"
""".strip(),
        encoding="utf-8",
    )
    monkeypatch.setenv("HOME", str(home))

    config = AgentConfig.load()

    assert config.model.provider == "openai_compatible"
    assert config.model.model == "user-default"


def test_load_without_path_falls_back_when_user_default_missing(monkeypatch, tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))

    config = AgentConfig.load()

    assert config.model.provider == "fake"
    assert config.model.model == "fake-colibri-model"


def test_explicit_config_path_overrides_user_default(monkeypatch, tmp_path):
    home = tmp_path / "home"
    config_dir = home / ".colibri"
    config_dir.mkdir(parents=True)
    (config_dir / "config.toml").write_text(
        """
[model]
model = "user-default"
""".strip(),
        encoding="utf-8",
    )
    explicit = tmp_path / "explicit.toml"
    explicit.write_text(
        """
[model]
model = "explicit"
""".strip(),
        encoding="utf-8",
    )
    monkeypatch.setenv("HOME", str(home))

    config = AgentConfig.load(explicit)

    assert config.model.model == "explicit"


def test_load_config_overrides_memory_values(tmp_path):
    memory_root = tmp_path / "memory"
    config_path = tmp_path / "agent.toml"
    config_path.write_text(
        f"""
[memory]
root = "{memory_root}"
max_search_results = 3
enabled = false
max_recall_topics = 2
max_recall_chars = 1234
""".strip(),
        encoding="utf-8",
    )

    config = AgentConfig.load(config_path)

    assert config.memory.root == memory_root
    assert config.memory.max_search_results == 3
    assert not config.memory.enabled
    assert config.memory.max_recall_topics == 2
    assert config.memory.max_recall_chars == 1234


def test_expand_user_path_expands_home():
    expanded = expand_user_path("~/.colibri")

    assert expanded.is_absolute()
    assert expanded.name == ".colibri"
