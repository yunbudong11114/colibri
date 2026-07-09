from __future__ import annotations

from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any
import tomllib


class ConfigError(RuntimeError):
    pass


# 默认用户配置文件路径；CLI 未指定 --config 时优先读取这里。
DEFAULT_USER_CONFIG = "~/.colibri/config.toml"


def expand_user_path(value: str) -> Path:
    return Path(value).expanduser()


@dataclass(frozen=True)
class ModelConfig:
    # 模型提供方；fake 用于本地测试，openai_compatible 用于真实 API。
    provider: str = "fake"
    # OpenAI-compatible API 基础地址，通常以 /v1 结尾。
    base_url: str = "https://api.openai.com/v1"
    # 请求模型名称。
    model: str = "fake-colibri-model"
    # 模型 API Key；为空时由模型 adapter 读取 COLIBRI_API_KEY。
    api_key: str = ""
    # 单次模型请求超时时间，单位秒。
    timeout_seconds: int = 60
    # 单次模型回复最大输出 token 数。
    max_output_tokens: int = 16384


@dataclass(frozen=True)
class SessionConfig:
    # 单次用户请求内允许的最大工具调用轮数。
    max_tool_rounds: int = 32
    # session 内消息数达到该值时触发历史压缩。
    trigger_message_limit: int = 96
    # 压缩后保留的最近消息条数。
    recent_message_limit: int = 12
    # 每次发送给模型的输入字符上限，也用于限制单条用户输入长度。
    model_input_char_limit: int = 192000
    # 滚动摘要最多保留的字符数。
    summary_max_chars: int = 12000
    # 是否优先调用模型生成历史摘要；失败时回退到本地摘要。
    model_compact: bool = True
    # REPL 空闲自动退出是否启用。
    idle_exit_enabled: bool = False
    # REPL 空闲自动退出等待秒数。
    idle_exit_seconds: int = 300
    # 是否写入 transcript JSONL 日志。
    transcript: bool = True


@dataclass(frozen=True)
class ToolsConfig:
    # 启用的工具类别列表。
    enabled: list[str] = field(default_factory=lambda: ["shell", "files", "web", "memory", "skills", "mcp"])
    # 默认权限策略；allow_read_confirm_write 表示只读默认允许、写入/执行需要确认。
    default_permission: str = "allow_read_confirm_write"
    # 单次工具结果写入上下文前的最大字符数。
    max_result_chars: int = 32000
    # shell.run 单次命令超时时间，单位秒。
    max_shell_seconds: int = 30


@dataclass(frozen=True)
class ShellConfig:
    # 永远拒绝执行的 shell 可执行名。
    deny: list[str] = field(default_factory=lambda: ["rm", "shutdown", "reboot", "mkfs", "dd", "sudo"])


@dataclass(frozen=True)
class FilesConfig:
    # 默认允许文件工具访问的根目录；启动目录也会被默认允许。
    roots: list[Path] = field(default_factory=lambda: [expand_user_path("~/.colibri/workspace"), Path("/tmp/colibri")])


@dataclass(frozen=True)
class SkillsConfig:
    # 本地 skill 搜索目录。
    dirs: list[Path] = field(default_factory=lambda: [expand_user_path("~/.colibri/skills")])
    # 每轮最多加载的相关 skill 数量。
    max_loaded: int = 3
    # 单个 skill 指令注入上下文前的最大字符数。
    max_instruction_chars: int = 8000


@dataclass(frozen=True)
class ConsoleConfig:
    # 是否在 stderr 输出 [colibri] 状态行。
    status: bool = True


@dataclass(frozen=True)
class MemoryConfig:
    # 长期记忆根目录。
    root: Path = field(default_factory=lambda: expand_user_path("~/.colibri/memory"))
    # memory.search 工具最多返回的搜索结果数。
    max_search_results: int = 5
    # 是否启用长期记忆。
    enabled: bool = True
    # 保留给未来模型辅助记忆选择的 topic 数量上限；当前不做本地关键词召回。
    max_recall_topics: int = 3
    # MEMORY.md 和 USER.md 作为 always-on 记忆注入上下文前的最大字符数。
    max_recall_chars: int = 6000


@dataclass(frozen=True)
class WebSearchConfig:
    # Web 搜索引擎名称。
    engine: str = "baidu"
    # Web 搜索 API Key。
    api_key: str = ""
    # Web 搜索 API endpoint。
    endpoint: str = "https://qianfan.baidubce.com/v2/ai_search/web_search"
    # 单次搜索默认返回结果数。
    max_results: int = 10
    # Web 搜索请求超时时间，单位秒。
    timeout_seconds: int = 10


@dataclass(frozen=True)
class GatewayConfig:
    # gateway 启用的 channel 列表。
    enabled_channels: list[str] = field(default_factory=lambda: ["weixin"])
    # gateway 同时保留在内存中的 channel session 数量上限。
    max_sessions: int = 4
    # channel session 空闲驱逐时间，单位秒。
    session_idle_seconds: int = 600


@dataclass(frozen=True)
class WeixinChannelConfig:
    # 是否启用微信 channel。
    enabled: bool = False
    # 微信 iLink bot token，由 colibri auth weixin 写入。
    token: str = ""
    # 微信 iLink API 基础地址。
    base_url: str = "https://ilinkai.weixin.qq.com/"
    # 允许访问的微信用户 ID 列表；空列表表示不限制。
    allow_from: list[str] = field(default_factory=list)
    # 微信长轮询请求超时时间，单位秒。
    poll_timeout_seconds: int = 35
    # 微信扫码授权等待时间，单位秒。
    auth_timeout_seconds: int = 300


@dataclass(frozen=True)
class ChannelsConfig:
    # 微信 channel 配置。
    weixin: WeixinChannelConfig = field(default_factory=WeixinChannelConfig)


@dataclass(frozen=True)
class McpConfig:
    # 是否启用 MCP。
    enabled: bool = False
    # MCP 启动策略；lazy 表示按需启动。
    startup: str = "lazy"
    # 同时活跃的 MCP server 数量上限。
    max_active_servers: int = 1


@dataclass(frozen=True)
class AgentConfig:
    # 模型配置。
    model: ModelConfig = field(default_factory=ModelConfig)
    # session、压缩、transcript 配置。
    session: SessionConfig = field(default_factory=SessionConfig)
    # 工具总配置。
    tools: ToolsConfig = field(default_factory=ToolsConfig)
    # shell 工具配置。
    shell: ShellConfig = field(default_factory=ShellConfig)
    # 文件工具配置。
    files: FilesConfig = field(default_factory=FilesConfig)
    # skill 加载配置。
    skills: SkillsConfig = field(default_factory=SkillsConfig)
    # 控制台状态输出配置。
    console: ConsoleConfig = field(default_factory=ConsoleConfig)
    # 长期记忆配置。
    memory: MemoryConfig = field(default_factory=MemoryConfig)
    # Web 搜索工具配置。
    web_search: WebSearchConfig = field(default_factory=WebSearchConfig)
    # gateway 运行配置。
    gateway: GatewayConfig = field(default_factory=GatewayConfig)
    # 各 channel 配置集合。
    channels: ChannelsConfig = field(default_factory=ChannelsConfig)
    # MCP 配置。
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
            web_search=_replace_dataclass(self.web_search, data.get("web_search", {})),
            gateway=_replace_dataclass(self.gateway, data.get("gateway", {})),
            channels=_replace_channels(self.channels, data.get("channels", {})),
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


def _replace_channels(instance: ChannelsConfig, overrides: dict[str, Any]) -> ChannelsConfig:
    if not overrides:
        return instance
    return replace(instance, weixin=_replace_dataclass(instance.weixin, overrides.get("weixin", {})))
