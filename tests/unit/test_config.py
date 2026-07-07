from pathlib import Path

from colibri.config import AgentConfig, expand_user_path


def test_default_config_uses_small_device_limits():
    config = AgentConfig.default()

    assert config.model.provider == "fake"
    assert config.model.model == "fake-colibri-model"
    assert config.session.max_tool_rounds == 6
    assert config.session.recent_message_limit == 16
    assert config.session.compact_trigger_chars == 36000
    assert config.tools.max_result_chars == 12000
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

[files]
roots = ["~/notes", "/tmp"]
""".strip(),
        encoding="utf-8",
    )

    config = AgentConfig.load(config_path)

    assert config.model.provider == "openai_compatible"
    assert config.model.model == "gpt-4.1-mini"
    assert config.model.timeout_seconds == 45
    assert config.session.recent_message_limit == 8
    assert config.files.roots[0].name == "notes"
    assert config.files.roots[1] == Path("/tmp")


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
