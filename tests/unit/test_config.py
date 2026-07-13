from pathlib import Path

import pytest

from colibri.config import AgentConfig, ConfigError, expand_user_path


def test_default_config_uses_small_device_limits():
    config = AgentConfig.default()

    assert config.model.provider == "fake"
    assert config.model.model == "fake-colibri-model"
    assert config.model.api_key == ""
    assert config.model.max_output_tokens == 16384
    assert config.model.input_context_tokens == 48000
    assert config.vision.model == ""
    assert config.vision.base_url == ""
    assert config.vision.api_key == ""
    assert config.vision.timeout_seconds == 60
    assert config.vision.max_image_bytes == 4 * 1024 * 1024
    assert config.session.max_tool_rounds == 32
    assert config.session.trigger_message_limit == 96
    assert config.session.recent_message_limit == 12
    assert config.session.summary_max_chars == 12000
    assert config.session.model_compact
    assert not config.session.idle_exit_enabled
    assert config.session.idle_exit_seconds == 300
    assert config.session.restore_transcript
    assert config.session.restore_message_limit == 24
    assert config.session.restore_char_limit == 24000
    assert config.session.restore_scan_bytes == 2097152
    assert config.session.transcript_retention_days == 30
    assert config.session.transcript_max_total_bytes == 134217728
    assert config.tools.max_result_chars == 32000
    assert "web" in config.tools.enabled
    assert "mcp" not in config.tools.enabled
    assert config.skills.max_catalog == 32
    assert config.skills.max_catalog_chars == 4000
    assert config.skills.max_instruction_chars == 8000
    assert config.skills.dir.name == "skills"
    assert config.memory.max_search_results == 5
    assert config.memory.max_recall_chars == 6000
    assert not hasattr(config.memory, "max_recall_topics")
    assert config.console.status
    assert config.console.plain_answer
    assert config.web_search.engine == "baidu"
    assert config.web_search.api_key == ""
    assert config.web_search.endpoint == "https://qianfan.baidubce.com/v2/ai_search/web_search"
    assert config.web_search.max_results == 10
    assert config.web_search.timeout_seconds == 10
    assert config.gateway.enabled_channels == ["weixin"]
    assert config.gateway.max_sessions == 4
    assert config.gateway.session_idle_seconds == 600
    assert not config.channels.weixin.enabled
    assert config.channels.weixin.base_url == "https://ilinkai.weixin.qq.com/"
    assert config.channels.weixin.allow_from == []
    assert not hasattr(config.channels.weixin, "message_debounce_seconds")
    assert config.shell.deny[:3] == ["rm", "shutdown", "reboot"]
    assert config.files.roots == [expand_user_path("~/.colibri/workspace"), Path("/tmp/colibri")]
    assert not hasattr(config.shell, "allow")
    assert not hasattr(config.files, "confirm_write")
    assert not hasattr(config, "mcp")


def test_load_config_overrides_nested_values(tmp_path):
    config_path = tmp_path / "agent.toml"
    config_path.write_text(
        """
[model]
provider = "openai_compatible"
model = "gpt-4.1-mini"
timeout_seconds = 45
api_key = "inline-key"
input_context_tokens = 1000000

[vision]
model = "vision-model"
base_url = "https://vision.example/v1"
api_key = "vision-key"
timeout_seconds = 33
max_image_bytes = 1234

[session]
trigger_message_limit = 20
recent_message_limit = 8
model_compact = false
idle_exit_enabled = true
idle_exit_seconds = 12
restore_transcript = false
restore_message_limit = 10
restore_char_limit = 9000
restore_scan_bytes = 123456
transcript_retention_days = 7
transcript_max_total_bytes = 7654321

[files]
roots = ["~/notes", "/tmp"]

[skills]
dir = "~/skills"
max_catalog = 2
max_catalog_chars = 1500
max_instruction_chars = 1234

[console]
status = false

[web_search]
engine = "baidu"
api_key = "search-key"
max_results = 5
timeout_seconds = 7

[gateway]
enabled_channels = ["weixin"]
max_sessions = 2
session_idle_seconds = 30

[channels.weixin]
enabled = true
token = "wx-token"
allow_from = ["user-1"]
poll_timeout_seconds = 11
auth_timeout_seconds = 22
""".strip(),
        encoding="utf-8",
    )

    config = AgentConfig.load(config_path)

    assert config.model.provider == "openai_compatible"
    assert config.model.model == "gpt-4.1-mini"
    assert config.model.api_key == "inline-key"
    assert config.model.timeout_seconds == 45
    assert config.model.input_context_tokens == 1000000
    assert config.vision.model == "vision-model"
    assert config.vision.base_url == "https://vision.example/v1"
    assert config.vision.api_key == "vision-key"
    assert config.vision.timeout_seconds == 33
    assert config.vision.max_image_bytes == 1234
    assert config.session.trigger_message_limit == 20
    assert config.session.recent_message_limit == 8
    assert not config.session.model_compact
    assert config.session.idle_exit_enabled
    assert config.session.idle_exit_seconds == 12
    assert not config.session.restore_transcript
    assert config.session.restore_message_limit == 10
    assert config.session.restore_char_limit == 9000
    assert config.session.restore_scan_bytes == 123456
    assert config.session.transcript_retention_days == 7
    assert config.session.transcript_max_total_bytes == 7654321
    assert config.files.roots[0].name == "notes"
    assert config.files.roots[1] == Path("/tmp")
    assert config.skills.dir.name == "skills"
    assert config.skills.max_catalog == 2
    assert config.skills.max_catalog_chars == 1500
    assert config.skills.max_instruction_chars == 1234
    assert not config.console.status
    assert config.web_search.api_key == "search-key"
    assert config.web_search.max_results == 5
    assert config.web_search.timeout_seconds == 7
    assert config.gateway.max_sessions == 2
    assert config.gateway.session_idle_seconds == 30
    assert config.channels.weixin.enabled
    assert config.channels.weixin.token == "wx-token"
    assert config.channels.weixin.allow_from == ["user-1"]
    assert config.channels.weixin.poll_timeout_seconds == 11
    assert config.channels.weixin.auth_timeout_seconds == 22


def test_legacy_model_input_char_limit_is_rejected(tmp_path):
    config_path = tmp_path / "agent.toml"
    config_path.write_text(
        """
[session]
model_input_char_limit = 192000
""".strip(),
        encoding="utf-8",
    )

    with pytest.raises(ConfigError, match="session.model_input_char_limit"):
        AgentConfig.load(config_path)


def test_legacy_model_input_byte_limit_is_rejected(tmp_path):
    config_path = tmp_path / "agent.toml"
    config_path.write_text(
        """
[model]
input_byte_limit = 192000
""".strip(),
        encoding="utf-8",
    )

    with pytest.raises(ConfigError, match="model.input_byte_limit"):
        AgentConfig.load(config_path)


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
max_recall_chars = 1234
""".strip(),
        encoding="utf-8",
    )

    config = AgentConfig.load(config_path)

    assert config.memory.root == memory_root
    assert config.memory.max_search_results == 3
    assert not config.memory.enabled
    assert config.memory.max_recall_chars == 1234
    assert not hasattr(config.memory, "max_recall_topics")
    assert not hasattr(config, "mcp")


def test_unknown_top_level_section_is_rejected(tmp_path):
    config_path = tmp_path / "agent.toml"
    config_path.write_text(
        """
[mcp]
enabled = true
""".strip(),
        encoding="utf-8",
    )

    with pytest.raises(ConfigError, match="unknown config field: mcp"):
        AgentConfig.load(config_path)


def test_deprecated_max_recall_topics_is_rejected(tmp_path):
    config_path = tmp_path / "agent.toml"
    config_path.write_text(
        """
[memory]
max_recall_topics = 2
""".strip(),
        encoding="utf-8",
    )

    with pytest.raises(ConfigError, match="memory.max_recall_topics"):
        AgentConfig.load(config_path)


def test_unknown_nested_field_is_rejected(tmp_path):
    config_path = tmp_path / "agent.toml"
    config_path.write_text(
        """
[model]
unknown_option = true
""".strip(),
        encoding="utf-8",
    )

    with pytest.raises(ConfigError, match="model.unknown_option"):
        AgentConfig.load(config_path)


def test_expand_user_path_expands_home():
    expanded = expand_user_path("~/.colibri")

    assert expanded.is_absolute()
    assert expanded.name == ".colibri"
