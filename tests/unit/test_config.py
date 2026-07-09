from pathlib import Path

from colibri.config import AgentConfig, expand_user_path


def test_default_config_uses_small_device_limits():
    config = AgentConfig.default()

    assert config.model.provider == "fake"
    assert config.model.model == "fake-colibri-model"
    assert config.model.api_key == ""
    assert config.model.max_output_tokens == 16384
    assert config.session.max_tool_rounds == 32
    assert config.session.trigger_message_limit == 96
    assert config.session.recent_message_limit == 12
    assert config.session.model_input_char_limit == 192000
    assert config.session.summary_max_chars == 12000
    assert config.session.model_compact
    assert not config.session.idle_exit_enabled
    assert config.session.idle_exit_seconds == 300
    assert config.tools.max_result_chars == 32000
    assert "web" in config.tools.enabled
    assert config.skills.max_loaded == 3
    assert config.skills.max_instruction_chars == 8000
    assert config.memory.max_search_results == 5
    assert config.memory.max_recall_topics == 3
    assert config.memory.max_recall_chars == 6000
    assert config.console.status
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
    assert config.shell.deny[:3] == ["rm", "shutdown", "reboot"]
    assert config.files.roots == [expand_user_path("~/.colibri/workspace"), Path("/tmp/colibri")]
    assert not hasattr(config.shell, "allow")
    assert not hasattr(config.files, "confirm_write")


def test_load_config_overrides_nested_values(tmp_path):
    config_path = tmp_path / "agent.toml"
    config_path.write_text(
        """
[model]
provider = "openai_compatible"
model = "gpt-4.1-mini"
timeout_seconds = 45
api_key = "inline-key"

[session]
trigger_message_limit = 20
recent_message_limit = 8
model_compact = false
idle_exit_enabled = true
idle_exit_seconds = 12

[files]
roots = ["~/notes", "/tmp"]

[skills]
dirs = ["~/skills"]
max_loaded = 2
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
    assert config.session.trigger_message_limit == 20
    assert config.session.recent_message_limit == 8
    assert not config.session.model_compact
    assert config.session.idle_exit_enabled
    assert config.session.idle_exit_seconds == 12
    assert config.files.roots[0].name == "notes"
    assert config.files.roots[1] == Path("/tmp")
    assert config.skills.dirs[0].name == "skills"
    assert config.skills.max_loaded == 2
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
