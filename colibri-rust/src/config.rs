use std::path::{Path, PathBuf};

pub const DEFAULT_USER_CONFIG: &str = "~/.colibri/config.toml";

#[derive(Clone, Debug)]
pub struct ModelConfig {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub timeout_seconds: u64,
    pub max_output_tokens: usize,
    pub input_context_tokens: usize,
}

#[derive(Clone, Debug)]
pub struct VisionConfig {
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub timeout_seconds: u64,
    pub max_image_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub max_tool_rounds: usize,
    pub trigger_message_limit: usize,
    pub recent_message_limit: usize,
    pub summary_max_chars: usize,
    pub model_compact: bool,
    pub idle_exit_enabled: bool,
    pub idle_exit_seconds: u64,
    pub transcript: bool,
    pub restore_transcript: bool,
    pub restore_message_limit: usize,
    pub restore_char_limit: usize,
    pub restore_scan_bytes: usize,
    pub transcript_retention_days: u64,
    pub transcript_max_total_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct ToolsConfig {
    pub enabled: Vec<String>,
    pub default_permission: String,
    pub max_result_chars: usize,
    pub max_shell_seconds: f64,
}

#[derive(Clone, Debug)]
pub struct ShellConfig {
    pub deny: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct FilesConfig {
    pub roots: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct SkillsConfig {
    pub dirs: Vec<PathBuf>,
    pub max_loaded: usize,
    pub max_instruction_chars: usize,
}

#[derive(Clone, Debug)]
pub struct ConsoleConfig {
    pub status: bool,
    pub plain_answer: bool,
}

#[derive(Clone, Debug)]
pub struct MemoryConfig {
    pub root: PathBuf,
    pub max_search_results: usize,
    pub enabled: bool,
    pub max_recall_chars: usize,
}

#[derive(Clone, Debug)]
pub struct WebSearchConfig {
    pub engine: String,
    pub api_key: String,
    pub endpoint: String,
    pub max_results: usize,
    pub timeout_seconds: u64,
}

#[derive(Clone, Debug)]
pub struct GatewayConfig {
    pub enabled_channels: Vec<String>,
    pub max_sessions: usize,
    pub session_idle_seconds: u64,
}

#[derive(Clone, Debug)]
pub struct WeixinChannelConfig {
    pub enabled: bool,
    pub token: String,
    pub base_url: String,
    pub allow_from: Vec<String>,
    pub poll_timeout_seconds: u64,
    pub auth_timeout_seconds: u64,
}

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub model: ModelConfig,
    pub vision: VisionConfig,
    pub session: SessionConfig,
    pub tools: ToolsConfig,
    pub shell: ShellConfig,
    pub files: FilesConfig,
    pub skills: SkillsConfig,
    pub console: ConsoleConfig,
    pub memory: MemoryConfig,
    pub web_search: WebSearchConfig,
    pub gateway: GatewayConfig,
    pub channels_weixin: WeixinChannelConfig,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: ModelConfig {
                provider: "fake".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                model: "fake-colibri-model".to_string(),
                api_key: String::new(),
                timeout_seconds: 60,
                max_output_tokens: 16384,
                input_context_tokens: 48000,
            },
            vision: VisionConfig {
                model: String::new(),
                base_url: String::new(),
                api_key: String::new(),
                timeout_seconds: 60,
                max_image_bytes: 4 * 1024 * 1024,
            },
            session: SessionConfig {
                max_tool_rounds: 32,
                trigger_message_limit: 96,
                recent_message_limit: 12,
                summary_max_chars: 12000,
                model_compact: true,
                idle_exit_enabled: false,
                idle_exit_seconds: 300,
                transcript: true,
                restore_transcript: true,
                restore_message_limit: 24,
                restore_char_limit: 24000,
                restore_scan_bytes: 2 * 1024 * 1024,
                transcript_retention_days: 30,
                transcript_max_total_bytes: 128 * 1024 * 1024,
            },
            tools: ToolsConfig {
                enabled: vec![
                    "shell".to_string(),
                    "files".to_string(),
                    "web".to_string(),
                    "image".to_string(),
                    "memory".to_string(),
                    "skills".to_string(),
                ],
                default_permission: "allow_read_confirm_write".to_string(),
                max_result_chars: 32000,
                max_shell_seconds: 30.0,
            },
            shell: ShellConfig {
                deny: vec![
                    "rm".to_string(),
                    "shutdown".to_string(),
                    "reboot".to_string(),
                    "mkfs".to_string(),
                    "dd".to_string(),
                    "sudo".to_string(),
                ],
            },
            files: FilesConfig {
                roots: vec![
                    expand_user_path("~/.colibri/workspace"),
                    PathBuf::from("/tmp/colibri"),
                ],
            },
            skills: SkillsConfig {
                dirs: vec![expand_user_path("~/.colibri/skills")],
                max_loaded: 3,
                max_instruction_chars: 8000,
            },
            console: ConsoleConfig {
                status: true,
                plain_answer: true,
            },
            memory: MemoryConfig {
                root: expand_user_path("~/.colibri/memory"),
                max_search_results: 5,
                enabled: true,
                max_recall_chars: 6000,
            },
            web_search: WebSearchConfig {
                engine: "baidu".to_string(),
                api_key: String::new(),
                endpoint: "https://qianfan.baidubce.com/v2/ai_search/web_search".to_string(),
                max_results: 10,
                timeout_seconds: 10,
            },
            gateway: GatewayConfig {
                enabled_channels: vec!["weixin".to_string()],
                max_sessions: 4,
                session_idle_seconds: 600,
            },
            channels_weixin: WeixinChannelConfig {
                enabled: false,
                token: String::new(),
                base_url: "https://ilinkai.weixin.qq.com/".to_string(),
                allow_from: Vec::new(),
                poll_timeout_seconds: 35,
                auth_timeout_seconds: 300,
            },
        }
    }
}

impl AgentConfig {
    pub fn load(path: Option<&Path>) -> Result<Self, String> {
        let active = match path {
            Some(path) => Some(path.to_path_buf()),
            None => {
                let default = expand_user_path(DEFAULT_USER_CONFIG);
                default.exists().then_some(default)
            }
        };
        let Some(path) = active else {
            return Ok(Self::default());
        };
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("failed to read config {}: {}", path.display(), error))?;
        let mut config = Self::default();
        let value = text
            .parse::<toml::Value>()
            .map_err(|error| format!("failed to parse config {}: {}", path.display(), error))?;
        apply_toml_value(&mut config, &value)?;
        Ok(config)
    }
}

pub fn expand_user_path(value: &str) -> PathBuf {
    if value == "~" {
        home_dir()
    } else if let Some(rest) = value.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        PathBuf::from(value)
    }
}

pub fn colibri_home() -> PathBuf {
    std::env::var_os("COLIBRI_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| expand_user_path("~/.colibri"))
}

/// Process RSS in KiB. `None` = current process; `Some(pid)` = that process.
pub fn rss_kb(pid: Option<u32>) -> Option<u64> {
    let pid = pid.unwrap_or_else(std::process::id);
    let proc_path = PathBuf::from("/proc").join(pid.to_string()).join("status");
    if let Ok(text) = std::fs::read_to_string(proc_path) {
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("VmRSS:") {
                if let Some(value) = value
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse().ok())
                {
                    return Some(value);
                }
            }
        }
    }
    let output = std::process::Command::new("ps")
        .arg("-o")
        .arg("rss=")
        .arg("-p")
        .arg(pid.to_string())
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn apply_toml_value(config: &mut AgentConfig, value: &toml::Value) -> Result<(), String> {
    validate_config_fields(value)?;
    if let Some(table) = value.get("model") {
        if let Some(value) = get_string(table, "provider") {
            config.model.provider = value;
        }
        if let Some(value) = get_string(table, "base_url") {
            config.model.base_url = value;
        }
        if let Some(value) = get_string(table, "model") {
            config.model.model = value;
        }
        if let Some(value) = get_string(table, "api_key") {
            config.model.api_key = value;
        }
        if let Some(value) = get_u64(table, "timeout_seconds") {
            config.model.timeout_seconds = value;
        }
        if let Some(value) = get_usize(table, "max_output_tokens") {
            config.model.max_output_tokens = value;
        }
        if let Some(value) = get_usize(table, "input_context_tokens") {
            config.model.input_context_tokens = value;
        }
    }
    if let Some(table) = value.get("vision") {
        if let Some(value) = get_string(table, "model") {
            config.vision.model = value;
        }
        if let Some(value) = get_string(table, "base_url") {
            config.vision.base_url = value;
        }
        if let Some(value) = get_string(table, "api_key") {
            config.vision.api_key = value;
        }
        if let Some(value) = get_u64(table, "timeout_seconds") {
            config.vision.timeout_seconds = value;
        }
        if let Some(value) = get_usize(table, "max_image_bytes") {
            config.vision.max_image_bytes = value;
        }
    }
    if let Some(table) = value.get("session") {
        if let Some(value) = get_usize(table, "max_tool_rounds") {
            config.session.max_tool_rounds = value;
        }
        if let Some(value) = get_usize(table, "trigger_message_limit") {
            config.session.trigger_message_limit = value;
        }
        if let Some(value) = get_usize(table, "recent_message_limit") {
            config.session.recent_message_limit = value;
        }
        if let Some(value) = get_usize(table, "summary_max_chars") {
            config.session.summary_max_chars = value;
        }
        if let Some(value) = get_bool(table, "model_compact") {
            config.session.model_compact = value;
        }
        if let Some(value) = get_bool(table, "idle_exit_enabled") {
            config.session.idle_exit_enabled = value;
        }
        if let Some(value) = get_u64(table, "idle_exit_seconds") {
            config.session.idle_exit_seconds = value;
        }
        if let Some(value) = get_bool(table, "transcript") {
            config.session.transcript = value;
        }
        if let Some(value) = get_bool(table, "restore_transcript") {
            config.session.restore_transcript = value;
        }
        if let Some(value) = get_usize(table, "restore_message_limit") {
            config.session.restore_message_limit = value;
        }
        if let Some(value) = get_usize(table, "restore_char_limit") {
            config.session.restore_char_limit = value;
        }
        if let Some(value) = get_usize(table, "restore_scan_bytes") {
            config.session.restore_scan_bytes = value;
        }
        if let Some(value) = get_u64(table, "transcript_retention_days") {
            config.session.transcript_retention_days = value;
        }
        if let Some(value) = get_usize(table, "transcript_max_total_bytes") {
            config.session.transcript_max_total_bytes = value;
        }
    }
    if let Some(table) = value.get("tools") {
        if let Some(value) = get_string_list(table, "enabled") {
            config.tools.enabled = value;
        }
        if let Some(value) = get_string(table, "default_permission") {
            config.tools.default_permission = value;
        }
        if let Some(value) = get_usize(table, "max_result_chars") {
            config.tools.max_result_chars = value;
        }
        if let Some(value) = get_f64(table, "max_shell_seconds") {
            config.tools.max_shell_seconds = value;
        }
    }
    if let Some(table) = value.get("shell") {
        if let Some(value) = get_string_list(table, "deny") {
            config.shell.deny = value;
        }
    }
    if let Some(table) = value.get("files") {
        if let Some(value) = get_path_list(table, "roots") {
            config.files.roots = value;
        }
    }
    if let Some(table) = value.get("skills") {
        if let Some(value) = get_path_list(table, "dirs") {
            config.skills.dirs = value;
        }
        if let Some(value) = get_usize(table, "max_loaded") {
            config.skills.max_loaded = value;
        }
        if let Some(value) = get_usize(table, "max_instruction_chars") {
            config.skills.max_instruction_chars = value;
        }
    }
    if let Some(table) = value.get("console") {
        if let Some(value) = get_bool(table, "status") {
            config.console.status = value;
        }
        if let Some(value) = get_bool(table, "plain_answer") {
            config.console.plain_answer = value;
        }
    }
    if let Some(table) = value.get("memory") {
        if let Some(value) = get_string(table, "root") {
            config.memory.root = expand_user_path(&value);
        }
        if let Some(value) = get_usize(table, "max_search_results") {
            config.memory.max_search_results = value;
        }
        if let Some(value) = get_bool(table, "enabled") {
            config.memory.enabled = value;
        }
        if let Some(value) = get_usize(table, "max_recall_chars") {
            config.memory.max_recall_chars = value;
        }
    }
    if let Some(table) = value.get("web_search") {
        if let Some(value) = get_string(table, "engine") {
            config.web_search.engine = value;
        }
        if let Some(value) = get_string(table, "api_key") {
            config.web_search.api_key = value;
        }
        if let Some(value) = get_string(table, "endpoint") {
            config.web_search.endpoint = value;
        }
        if let Some(value) = get_usize(table, "max_results") {
            config.web_search.max_results = value;
        }
        if let Some(value) = get_u64(table, "timeout_seconds") {
            config.web_search.timeout_seconds = value;
        }
    }
    if let Some(table) = value.get("gateway") {
        if let Some(value) = get_string_list(table, "enabled_channels") {
            config.gateway.enabled_channels = value;
        }
        if let Some(value) = get_usize(table, "max_sessions") {
            config.gateway.max_sessions = value;
        }
        if let Some(value) = get_u64(table, "session_idle_seconds") {
            config.gateway.session_idle_seconds = value;
        }
    }
    if let Some(table) = value.get("channels").and_then(|value| value.get("weixin")) {
        if let Some(value) = get_bool(table, "enabled") {
            config.channels_weixin.enabled = value;
        }
        if let Some(value) = get_string(table, "token") {
            config.channels_weixin.token = value;
        }
        if let Some(value) = get_string(table, "base_url") {
            config.channels_weixin.base_url = value;
        }
        if let Some(value) = get_string_list(table, "allow_from") {
            config.channels_weixin.allow_from = value;
        }
        if let Some(value) = get_u64(table, "poll_timeout_seconds") {
            config.channels_weixin.poll_timeout_seconds = value;
        }
        if let Some(value) = get_u64(table, "auth_timeout_seconds") {
            config.channels_weixin.auth_timeout_seconds = value;
        }
    }
    Ok(())
}

fn validate_config_fields(value: &toml::Value) -> Result<(), String> {
    validate_table(
        value,
        "model",
        &[
            "provider",
            "base_url",
            "model",
            "api_key",
            "timeout_seconds",
            "max_output_tokens",
            "input_context_tokens",
        ],
        &[],
    )?;
    validate_table(
        value,
        "vision",
        &[
            "model",
            "base_url",
            "api_key",
            "timeout_seconds",
            "max_image_bytes",
        ],
        &[],
    )?;
    validate_table(
        value,
        "session",
        &[
            "max_tool_rounds",
            "trigger_message_limit",
            "recent_message_limit",
            "summary_max_chars",
            "model_compact",
            "idle_exit_enabled",
            "idle_exit_seconds",
            "transcript",
            "restore_transcript",
            "restore_message_limit",
            "restore_char_limit",
            "restore_scan_bytes",
            "transcript_retention_days",
            "transcript_max_total_bytes",
        ],
        &[],
    )?;
    validate_table(
        value,
        "tools",
        &[
            "enabled",
            "default_permission",
            "max_result_chars",
            "max_shell_seconds",
        ],
        &[],
    )?;
    validate_table(value, "shell", &["deny"], &[])?;
    validate_table(value, "files", &["roots"], &[])?;
    validate_table(
        value,
        "skills",
        &["dirs", "max_loaded", "max_instruction_chars"],
        &[],
    )?;
    validate_table(value, "console", &["status", "plain_answer"], &[])?;
    validate_table(
        value,
        "memory",
        &["root", "max_search_results", "enabled", "max_recall_chars"],
        &["max_recall_topics"],
    )?;
    validate_table(
        value,
        "web_search",
        &[
            "engine",
            "api_key",
            "endpoint",
            "max_results",
            "timeout_seconds",
        ],
        &[],
    )?;
    validate_table(
        value,
        "gateway",
        &["enabled_channels", "max_sessions", "session_idle_seconds"],
        &[],
    )?;
    if let Some(channels) = value.get("channels") {
        validate_nested_table(
            channels,
            "weixin",
            &[
                "enabled",
                "token",
                "base_url",
                "allow_from",
                "poll_timeout_seconds",
                "auth_timeout_seconds",
            ],
        )?;
    }
    Ok(())
}

fn validate_table(
    root: &toml::Value,
    section: &str,
    allowed: &[&str],
    ignored: &[&str],
) -> Result<(), String> {
    let Some(table) = root.get(section).and_then(toml::Value::as_table) else {
        return Ok(());
    };
    for key in table.keys() {
        if !allowed.contains(&key.as_str()) && !ignored.contains(&key.as_str()) {
            return Err(format!("unknown config field: {}.{}", section, key));
        }
    }
    Ok(())
}

fn validate_nested_table(
    root: &toml::Value,
    section: &str,
    allowed: &[&str],
) -> Result<(), String> {
    let Some(table) = root.get(section).and_then(toml::Value::as_table) else {
        return Ok(());
    };
    for key in table.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "unknown config field: channels.{}.{}",
                section, key
            ));
        }
    }
    Ok(())
}

fn get_string(table: &toml::Value, key: &str) -> Option<String> {
    table.get(key)?.as_str().map(ToString::to_string)
}

fn get_string_list(table: &toml::Value, key: &str) -> Option<Vec<String>> {
    Some(
        table
            .get(key)?
            .as_array()?
            .iter()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect(),
    )
}

fn get_path_list(table: &toml::Value, key: &str) -> Option<Vec<PathBuf>> {
    Some(
        get_string_list(table, key)?
            .iter()
            .map(|item| expand_user_path(item))
            .collect(),
    )
}

fn get_bool(table: &toml::Value, key: &str) -> Option<bool> {
    table.get(key)?.as_bool()
}

fn get_usize(table: &toml::Value, key: &str) -> Option<usize> {
    table.get(key)?.as_integer()?.try_into().ok()
}

fn get_u64(table: &toml::Value, key: &str) -> Option<u64> {
    table.get(key)?.as_integer()?.try_into().ok()
}

fn get_f64(table: &toml::Value, key: &str) -> Option<f64> {
    let value = table.get(key)?;
    value
        .as_float()
        .or_else(|| value.as_integer().map(|item| item as f64))
}
