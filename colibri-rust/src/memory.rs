use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use crate::config::AgentConfig;

const TRUNCATED_SUFFIX: &str = "\n...[truncated]";
const SOUL_LIMIT: usize = 1000;
const USER_LIMIT: usize = 1000;
const MEMORY_LIMIT: usize = 2000;
const BOOTSTRAP_SENTINELS: &[&str] = &["SOUL.md", "USER.md", "MEMORY.md", "INDEX.md"];
const SOUL_TEMPLATE: &str = include_str!("../../src/colibri/memory_templates/SOUL.md");
const USER_TEMPLATE: &str = include_str!("../../src/colibri/memory_templates/USER.md");
const MEMORY_TEMPLATE: &str = include_str!("../../src/colibri/memory_templates/MEMORY.md");
const INDEX_TEMPLATE: &str = include_str!("../../src/colibri/memory_templates/INDEX.md");
const TOPIC_TEMPLATE: &str = include_str!("../../src/colibri/memory_templates/topics/sample.md");

static MEMORY_LOAD_CACHE: OnceLock<Mutex<HashMap<MemoryCacheKey, Arc<MemoryLoadResult>>>> =
    OnceLock::new();

fn memory_load_cache() -> &'static Mutex<HashMap<MemoryCacheKey, Arc<MemoryLoadResult>>> {
    MEMORY_LOAD_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct MemoryCacheKey {
    root: String,
    max_recall_chars: usize,
    soul_mtime: Option<u128>,
    user_mtime: Option<u128>,
    memory_mtime: Option<u128>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryLoadResult {
    pub text: String,
    pub files: Vec<String>,
    pub truncated: bool,
}

pub struct MemoryContext {
    config: std::sync::Arc<AgentConfig>,
}

impl MemoryContext {
    pub fn new(config: impl Into<std::sync::Arc<AgentConfig>>) -> Self {
        Self {
            config: config.into(),
        }
    }

    pub fn load(&self) -> Result<Arc<MemoryLoadResult>, String> {
        if !self.config.memory.enabled {
            return Ok(Arc::new(MemoryLoadResult {
                text: String::new(),
                files: Vec::new(),
                truncated: false,
            }));
        }
        let _ = bootstrap(&self.config);
        let cache_key = memory_cache_key(
            &self.config.memory.root,
            self.config.memory.max_recall_chars,
        );
        if let Ok(cache) = memory_load_cache().lock() {
            if let Some(cached) = cache.get(&cache_key) {
                return Ok(Arc::clone(cached));
            }
        }

        let mut files = Vec::new();
        let mut blocks = vec!["Always-on memory:".to_string()];
        let mut any_file_truncated = false;
        for (name, limit) in [
            ("SOUL.md", SOUL_LIMIT),
            ("USER.md", USER_LIMIT),
            ("MEMORY.md", MEMORY_LIMIT),
        ] {
            let path = self.config.memory.root.join(name);
            let Some(text) = read_text_lossy(&path) else {
                continue;
            };
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let (text, truncated) = truncate(text.to_string(), limit);
            any_file_truncated |= truncated;
            files.push(name.to_string());
            blocks.push(format!("[{}]\n{}", name, text));
        }
        let result = if files.is_empty() {
            Arc::new(MemoryLoadResult {
                text: String::new(),
                files,
                truncated: false,
            })
        } else {
            let (text, total_truncated) =
                truncate(blocks.join("\n\n"), self.config.memory.max_recall_chars);
            Arc::new(MemoryLoadResult {
                text,
                files,
                truncated: any_file_truncated || total_truncated,
            })
        };
        if let Ok(mut cache) = memory_load_cache().lock() {
            cache.insert(cache_key, Arc::clone(&result));
        }
        Ok(result)
    }
}

pub fn bootstrap(config: &AgentConfig) -> Result<(), String> {
    if !config.memory.enabled {
        return Ok(());
    }
    let root = &config.memory.root;
    if has_bootstrap_sentinel(root) {
        return Ok(());
    }
    for (relative, content) in [
        ("SOUL.md", SOUL_TEMPLATE),
        ("USER.md", USER_TEMPLATE),
        ("MEMORY.md", MEMORY_TEMPLATE),
        ("INDEX.md", INDEX_TEMPLATE),
        ("topics/sample.md", TOPIC_TEMPLATE),
    ] {
        let path = root.join(relative);
        if path.exists() {
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(path, content).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn has_bootstrap_sentinel(root: &Path) -> bool {
    BOOTSTRAP_SENTINELS
        .iter()
        .any(|name| root.join(name).is_file())
}

fn memory_cache_key(root: &Path, max_recall_chars: usize) -> MemoryCacheKey {
    MemoryCacheKey {
        root: root.to_string_lossy().into_owned(),
        max_recall_chars,
        soul_mtime: file_mtime(&root.join("SOUL.md")),
        user_mtime: file_mtime(&root.join("USER.md")),
        memory_mtime: file_mtime(&root.join("MEMORY.md")),
    }
}

fn file_mtime(path: &Path) -> Option<u128> {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
}

fn read_text_lossy(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn truncate(mut text: String, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text, false);
    }
    let keep = max_chars.saturating_sub(TRUNCATED_SUFFIX.chars().count());
    text = text.chars().take(keep).collect::<String>() + TRUNCATED_SUFFIX;
    (text, true)
}
