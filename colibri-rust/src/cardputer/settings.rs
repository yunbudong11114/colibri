use crate::config::{apply_toml_value, AgentConfig};
use crate::http::http_endpoint as endpoint;
use crate::model::{build_model_for_path, ModelClient};
use serde_json::{json, Map, Value};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table};

use crate::config_io::{atomic_write, lock, Scratch};

fn defaults() -> Map<String, Value> {
    let mut values = menu_fields(&AgentConfig::default());
    values.insert("model.provider".into(), json!("openai_compatible"));
    values.insert("model.model".into(), json!("gpt-4.1-mini"));
    values
}

pub struct Settings {
    pub home: PathBuf,
}

impl Settings {
    fn stored(&self) -> Result<Map<String, Value>, String> {
        match fs::read(self.home.join("settings.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "设备设置文件格式无效".into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Map::new()),
            Err(error) => Err(error.to_string()),
        }
    }

    fn path(&self) -> PathBuf {
        self.home.join("config.toml")
    }

    fn document(&self) -> Result<DocumentMut, String> {
        match fs::read_to_string(self.path()) {
            Ok(text) => text.parse().map_err(|_| "config.toml 格式无效".into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(DocumentMut::new()),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn initialize(&self) -> Result<(), String> {
        fs::create_dir_all(&self.home).map_err(|e| e.to_string())?;
        let _lock = lock(&self.home.join("settings.lock")).map_err(|e| e.to_string())?;
        let legacy = self.home.join("settings.json");
        let archive = self.home.join("settings.json.legacy");
        let mut document = self.document()?;
        let mut changed = false;
        if !archive.exists() && legacy.is_file() {
            let stored = self.stored()?;
            let defaults = defaults();
            if stored
                .keys()
                .any(|key| key.starts_with("model.") && defaults.contains_key(key))
            {
                for name in [
                    "model.provider",
                    "model.base_url",
                    "model.model",
                    "model.api_key",
                ] {
                    set_field(
                        &mut document,
                        name,
                        stored.get(name).unwrap_or(&defaults[name]),
                    )?;
                }
            }
            for (key, value) in &stored {
                if let Some(expected) = defaults.get(key) {
                    validate_type(expected, value)?;
                    set_field(&mut document, key, value)?;
                }
            }
            let backup = self.home.join("config.toml.pre-cardputer-unification");
            if self.path().is_file() && !backup.exists() {
                let old = fs::read(self.path()).map_err(|error| error.to_string())?;
                atomic_write(&backup, &old).map_err(|error| error.to_string())?;
            }
            changed = true;
        }
        if !document.contains_key("model") {
            for key in ["model.provider", "model.base_url", "model.model"] {
                set_field(&mut document, key, &defaults()[key])?;
            }
            changed = true;
        }
        if document
            .get("hardware")
            .and_then(|table| table.get("enabled"))
            .is_none()
        {
            set_field(&mut document, "hardware.enabled", &json!(true))?;
            changed = true;
        }
        if changed {
            self.write_document(&document)?;
        }
        if !archive.exists() && legacy.is_file() {
            fs::rename(legacy, archive).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub fn values(&self) -> Result<Map<String, Value>, String> {
        Ok(menu_fields(&self.config()?))
    }

    pub fn public(&self) -> Result<Vec<Value>, String> {
        Ok(self
            .values()?
            .into_iter()
            .map(|(key, value)| {
                let secret = key.ends_with("api_key") || key == "mcp.servers";
                let display = if secret {
                    if value.as_str() == Some("") || value.as_array().is_some_and(Vec::is_empty) {
                        ""
                    } else {
                        "•••"
                    }
                    .to_string()
                } else {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string())
                };
                json!({"key":key,"value":display,"secret":secret})
            })
            .collect())
    }

    pub fn save(&self, key: &str, raw: &str) -> Result<(), String> {
        let defaults = defaults();
        let default = defaults.get(key).ok_or("未知设置项")?;
        let value = if default.is_string() {
            json!(if key.starts_with("vision.") || key == "memory.root" {
                raw.trim()
            } else {
                raw
            })
        } else {
            serde_json::from_str(raw).map_err(|_| "设置值格式错误")?
        };
        validate_type(default, &value)?;
        if key.starts_with("vision.") && value.as_u64() == Some(0) {
            return Err("请输入大于 0 的整数".into());
        }
        if key.ends_with(".base_url") || key.ends_with(".endpoint") {
            let address = value.as_str().unwrap_or_default();
            if !address.is_empty() || key != "vision.base_url" {
                endpoint(address)?;
            }
        }
        match key {
            "mcp.servers" => {
                crate::mcp::servers(&value)?;
            }
            "model.provider" if !matches!(value.as_str(), Some("fake" | "openai_compatible")) => {
                return Err("模型提供方无效".into())
            }
            "web_search.engine" if !matches!(value.as_str(), Some("baidu" | "aliyun_mcp")) => {
                return Err("搜索引擎无效".into())
            }
            "memory.root"
                if !crate::config::expand_user_path(value.as_str().unwrap_or_default())
                    .is_absolute() =>
            {
                return Err("请输入绝对路径或 ~/ 开头的目录".into())
            }
            _ => {}
        }
        self.update(|stored| {
            if key == "session.max_context_memory_mib" && value.as_u64() == Some(0) {
                if let Some(table) = stored
                    .get_mut("session")
                    .and_then(|item| item.as_table_mut())
                {
                    table.remove("max_context_memory_mib");
                }
                return Ok(());
            }
            if matches!(key, "model.provider" | "model.base_url" | "model.model") {
                let current = parse_document(stored, &self.home)?;
                let previous = match key {
                    "model.provider" => &current.model.provider,
                    "model.base_url" => &current.model.base_url,
                    _ => &current.model.model,
                };
                if value.as_str() != Some(previous.as_str()) {
                    if let Some(model) =
                        stored.get_mut("model").and_then(|item| item.as_table_mut())
                    {
                        model.remove("input_context_tokens");
                    }
                }
            }
            set_field(stored, key, &value)
        })
    }

    pub fn save_model(&self, base: &str, model: &str, key: Option<&str>) -> Result<(), String> {
        let base = base
            .trim()
            .trim_end_matches('/')
            .trim_end_matches("/chat/completions");
        let url = endpoint(base)?;
        if url.query().is_some() || url.fragment().is_some() || model.trim().is_empty() {
            return Err("请输入有效的 API 地址和模型 ID".into());
        }
        self.update(|stored| {
            let config = parse_document(stored, &self.home)?;
            let same = config.model.base_url.trim_end_matches('/') == base;
            if !same || config.model.model != model.trim() {
                if let Some(table) = stored.get_mut("model").and_then(|item| item.as_table_mut()) {
                    table.remove("input_context_tokens");
                }
            }
            let key = key
                .map(str::trim)
                .unwrap_or(if same { &config.model.api_key } else { "" });
            if crate::model::model_key_required(&url) && key.is_empty() {
                return Err("请填写这个服务商的 API Key".into());
            }
            set_field(stored, "model.api_key", &json!(key))?;
            set_field(stored, "model.base_url", &json!(base))?;
            set_field(stored, "model.model", &json!(model.trim()))?;
            set_field(stored, "model.provider", &json!("openai_compatible"))
        })
    }

    fn update(
        &self,
        edit: impl FnOnce(&mut DocumentMut) -> Result<(), String>,
    ) -> Result<(), String> {
        let _lock = lock(&self.home.join("settings.lock")).map_err(|e| e.to_string())?;
        let mut document = self.document()?;
        edit(&mut document)?;
        self.write_document(&document)
    }

    fn write_document(&self, document: &DocumentMut) -> Result<(), String> {
        parse_document(document, &self.home)?;
        atomic_write(&self.path(), document.to_string().as_bytes()).map_err(|e| e.to_string())
    }

    pub fn config(&self) -> Result<AgentConfig, String> {
        parse_document(&self.document()?, &self.home)
    }
}

fn parse_document(document: &DocumentMut, home: &Path) -> Result<AgentConfig, String> {
    let mut config = AgentConfig::user_defaults();
    config.memory.root = home.join("memory");
    let value = document
        .to_string()
        .parse()
        .map_err(|_| "config.toml 格式无效")?;
    apply_toml_value(&mut config, &value)?;
    crate::mcp::servers(&config.mcp_servers)?;
    Ok(config)
}

fn set_field(document: &mut DocumentMut, key: &str, value: &Value) -> Result<(), String> {
    let (group, field) = key.split_once('.').ok_or("设置键格式无效")?;
    let mut encoded = toml_edit::ser::to_document(&serde_json::json!({field:value}))
        .map_err(|error| error.to_string())?;
    let mut item = encoded.remove(field).ok_or("设置值无法序列化")?;
    if !document.contains_key(group) {
        document[group] = Item::Table(Table::new());
    }
    let table = document[group]
        .as_table_like_mut()
        .ok_or("设置分组格式无效")?;
    if let Some(old) = table.get(field).and_then(Item::as_value) {
        if let Some(new) = item.as_value_mut() {
            *new.decor_mut() = old.decor().clone();
        }
    }
    table.insert(field, item);
    Ok(())
}

fn menu_fields(config: &AgentConfig) -> Map<String, Value> {
    json!({
        "model.provider":config.model.provider, "model.base_url":config.model.base_url,
        "model.model":config.model.model, "model.api_key":config.model.api_key,
        "vision.model":config.vision.model, "vision.base_url":config.vision.base_url,
        "vision.api_key":config.vision.api_key, "vision.timeout_seconds":config.vision.timeout_seconds,
        "vision.max_image_bytes":config.vision.max_image_bytes,
        "web_search.engine":config.web_search.engine, "web_search.endpoint":config.web_search.endpoint,
        "web_search.api_key":config.web_search.api_key, "session.transcript":config.session.transcript,
        "session.max_context_memory_mib":config.session.max_context_memory_mib.unwrap_or(0),
        "session.restore_transcript":config.session.restore_transcript,
        "session.transcript_retention_days":config.session.transcript_retention_days,
        "session.transcript_max_total_bytes":config.session.transcript_max_total_bytes,
        "memory.enabled":config.memory.enabled, "memory.root":config.memory.root,
        "mcp.servers":config.mcp_servers
    }).as_object().unwrap().clone()
}

pub fn device_model(config: &AgentConfig, home: &Path) -> Result<Box<dyn ModelClient>, String> {
    build_model_for_path(&config.model, &home.join("config.toml"))
}

fn validate_type(default: &Value, value: &Value) -> Result<(), String> {
    if !(default.is_string() && value.is_string()
        || default.is_boolean() && value.is_boolean()
        || default.is_array() && value.is_array()
        || default.is_number() && value.as_u64().is_some())
    {
        return Err("设置值类型或范围错误".into());
    }
    Ok(())
}

pub fn migrate(legacy: &Path, home: &Path) -> io::Result<()> {
    let marker = home.join(".cardputer-layout-v1");
    if !legacy.is_dir()
        || marker.exists()
        || fs::canonicalize(home).ok() == fs::canonicalize(legacy).ok()
    {
        return Ok(());
    }
    let mut files = vec![PathBuf::from("settings.json")];
    if let Ok(entries) = fs::read_dir(legacy.join("transcripts")) {
        files.extend(
            entries
                .flatten()
                .map(|entry| entry.file_name())
                .filter(|name| {
                    Path::new(name)
                        .extension()
                        .is_some_and(|ext| ext == "jsonl")
                })
                .map(|name| Path::new("transcripts").join(name)),
        );
    }
    for relative in files {
        let source = legacy.join(&relative);
        if !fs::symlink_metadata(&source).is_ok_and(|metadata| metadata.is_file()) {
            continue;
        }
        let target = home.join(relative);
        if target.exists() {
            continue;
        }
        let parent = target.parent().unwrap_or(home);
        fs::create_dir_all(parent)?;
        let stage = Scratch::new(parent)?;
        let temp = stage.0.join("data");
        let mut output = File::create(&temp)?;
        io::copy(&mut File::open(source)?, &mut output)?;
        output.sync_all()?;
        match fs::hard_link(temp, target) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error),
        }
    }
    fs::create_dir_all(home)?;
    File::create(marker).map(|_| ())
}
