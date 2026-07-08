from __future__ import annotations

from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any
import tomllib


class ConfigError(RuntimeError):
    pass


DEFAULT_USER_CONFIG = "~/.colibri/config.toml"


def expand_user_path(value: str) -> Path:
    return Path(value).expanduser()


@dataclass(frozen=True)
class ModelConfig:
    provider: str = "fake"
    base_url: str = "https://api.openai.com/v1"
    model: str = "fake-colibri-model"
    api_key_env: str = "OPENAI_API_KEY"
    timeout_seconds: int = 60
    max_output_tokens: int = 10240


@dataclass(frozen=True)
class SessionConfig:
    max_tool_rounds: int = 16
    recent_message_limit: int = 48
    compact_trigger_chars: int = 36000
    summary_max_chars: int = 6000
    model_compact: bool = True
    idle_exit_seconds: int = 300
    transcript: bool = True


@dataclass(frozen=True)
class ToolsConfig:
    enabled: list[str] = field(default_factory=lambda: ["shell", "files", "http", "memory", "skills", "mcp"])
    default_permission: str = "allow_read_confirm_write"
    max_result_chars: int = 12000
    max_shell_seconds: int = 30


@dataclass(frozen=True)
class ShellConfig:
    allow: list[str] = field(default_factory=lambda: ["ls", "cat", "sed", "rg", "python", "git status"])
    deny: list[str] = field(default_factory=lambda: ["rm", "shutdown", "reboot", "mkfs", "dd", "sudo"])


@dataclass(frozen=True)
class FilesConfig:
    roots: list[Path] = field(default_factory=lambda: [expand_user_path("~/.colibri"), Path("/tmp")])
    confirm_write: bool = True


@dataclass(frozen=True)
class SkillsConfig:
    dirs: list[Path] = field(default_factory=lambda: [expand_user_path("~/.colibri/skills")])
    max_loaded: int = 3
    max_instruction_chars: int = 6000


@dataclass(frozen=True)
class ConsoleConfig:
    status: bool = True


@dataclass(frozen=True)
class MemoryConfig:
    root: Path = field(default_factory=lambda: expand_user_path("~/.colibri/memory"))
    max_search_results: int = 5
    enabled: bool = True
    max_recall_topics: int = 3
    max_recall_chars: int = 4000


@dataclass(frozen=True)
class McpConfig:
    enabled: bool = False
    startup: str = "lazy"
    max_active_servers: int = 1


@dataclass(frozen=True)
class AgentConfig:
    model: ModelConfig = field(default_factory=ModelConfig)
    session: SessionConfig = field(default_factory=SessionConfig)
    tools: ToolsConfig = field(default_factory=ToolsConfig)
    shell: ShellConfig = field(default_factory=ShellConfig)
    files: FilesConfig = field(default_factory=FilesConfig)
    skills: SkillsConfig = field(default_factory=SkillsConfig)
    console: ConsoleConfig = field(default_factory=ConsoleConfig)
    memory: MemoryConfig = field(default_factory=MemoryConfig)
    mcp: McpConfig = field(default_factory=McpConfig)

    @classmethod
    def default(cls) -> "AgentConfig":
        return cls()

    @classmethod
    def load(cls, path: str | Path | None = None) -> "AgentConfig":
        if path is None:
            default_path = expand_user_path(DEFAULT_USER_CONFIG)
            if default_path.exists():
                path = default_path
            else:
                return cls.default()
        data = tomllib.loads(Path(path).read_text(encoding="utf-8"))
        return cls.default().with_overrides(data)

    def with_overrides(self, data: dict[str, Any]) -> "AgentConfig":
        return replace(
            self,
            model=_replace_dataclass(self.model, data.get("model", {})),
            session=_replace_dataclass(self.session, data.get("session", {})),
            tools=_replace_dataclass(self.tools, data.get("tools", {})),
            shell=_replace_dataclass(self.shell, data.get("shell", {})),
            files=_replace_dataclass(self.files, _path_list_overrides(data.get("files", {}), "roots")),
            skills=_replace_dataclass(self.skills, _path_list_overrides(data.get("skills", {}), "dirs")),
            console=_replace_dataclass(self.console, data.get("console", {})),
            memory=_replace_dataclass(self.memory, _path_overrides(data.get("memory", {}), "root")),
            mcp=_replace_dataclass(self.mcp, data.get("mcp", {})),
        )


def _replace_dataclass(instance: Any, overrides: dict[str, Any]) -> Any:
    if not overrides:
        return instance
    return replace(instance, **overrides)


def _path_list_overrides(overrides: dict[str, Any], key: str) -> dict[str, Any]:
    copied = dict(overrides)
    if key in copied:
        copied[key] = [expand_user_path(value) for value in copied[key]]
    return copied


def _path_overrides(overrides: dict[str, Any], key: str) -> dict[str, Any]:
    copied = dict(overrides)
    if key in copied:
        copied[key] = expand_user_path(copied[key])
    return copied
