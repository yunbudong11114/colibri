use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use crate::config::AgentConfig;

const TRUNCATED_SUFFIX: &str = "\n...[truncated]";
const MEMORY_LIMIT: usize = 1800;
const USER_LIMIT: usize = 600;
const BOOTSTRAP_SENTINELS: &[&str] = &["MEMORY.md", "USER.md", "INDEX.md"];

const MEMORY_TEMPLATE: &str = r#"---
type: system
description: Colibri 长期事实和项目上下文；首次真实写入时直接覆盖样例文本
updated: 2026-07-09
---

- 用途：记录稳定事实、项目决策、运行环境和未来对话需要长期记住的上下文。
- 修改规则：用户或大模型需要修改 memory 时，请先去重和合并，再用 `memory.write` 重写本文件；首次真实写入时直接覆盖样例，不要保留原本的示例文本。
"#;

const USER_TEMPLATE: &str = r#"---
type: user
description: 用户偏好和协作方式；首次真实写入时直接覆盖样例文本
updated: 2026-07-09
---

- 用途：记录用户画像、偏好、称呼、语言风格和协作习惯。
- 修改规则：用户或大模型需要修改用户记忆时，请合并同类偏好并重写本文件，保持简短；首次真实写入时直接覆盖样例，不要保留原本的示例文本。
"#;

const INDEX_TEMPLATE: &str = r#"---
type: reference
description: memory topic 索引；首次真实写入时直接覆盖样例文本
updated: 2026-07-09
---

# Memory Index

- [sample](topics/sample.md): sample 示例 topic 详细记忆 写法 维护 memory search index

修改规则：新增或实质修改 `topics/*.md` 时，也要重写本索引中的对应条目。冒号后写多个关键词、别名和描述词，方便 `memory.search` 用子串匹配检索。首次真实写入时直接覆盖样例，不要保留原本的示例文本。
"#;

const TOPIC_TEMPLATE: &str = r#"---
type: reference
description: 样例详细记忆 topic；首次真实写入时直接覆盖样例文本
updated: 2026-07-09
---

# Sample Topic

- 用途：topic 文件用于保存比 `MEMORY.md` 更长、更细的专项信息，例如设备、项目设计、环境快照或长期任务背景。
- 修改规则：用户或大模型需要修改该 topic 时，请去重、合并、重写相关段落；如果主题说明变化，也要同步更新 `INDEX.md`。首次真实写入时直接覆盖样例，不要保留原本的示例文本。
"#;

static MEMORY_LOAD_CACHE: OnceLock<Mutex<HashMap<MemoryCacheKey, MemoryLoadResult>>> =
    OnceLock::new();

fn memory_load_cache() -> &'static Mutex<HashMap<MemoryCacheKey, MemoryLoadResult>> {
    MEMORY_LOAD_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct MemoryCacheKey {
    root: String,
    max_recall_chars: usize,
    memory_mtime: Option<u128>,
    user_mtime: Option<u128>,
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

    pub fn load(&self) -> Result<MemoryLoadResult, String> {
        if !self.config.memory.enabled {
            return Ok(MemoryLoadResult {
                text: String::new(),
                files: Vec::new(),
                truncated: false,
            });
        }
        let _ = bootstrap(&self.config);
        let cache_key = memory_cache_key(
            &self.config.memory.root,
            self.config.memory.max_recall_chars,
        );
        if let Ok(cache) = memory_load_cache().lock() {
            if let Some(cached) = cache.get(&cache_key) {
                return Ok(cached.clone());
            }
        }

        let mut files = Vec::new();
        let mut blocks = vec!["Always-on memory:".to_string()];
        let mut any_file_truncated = false;
        for (name, limit) in [("MEMORY.md", MEMORY_LIMIT), ("USER.md", USER_LIMIT)] {
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
            MemoryLoadResult {
                text: String::new(),
                files,
                truncated: false,
            }
        } else {
            let (text, total_truncated) =
                truncate(blocks.join("\n\n"), self.config.memory.max_recall_chars);
            MemoryLoadResult {
                text,
                files,
                truncated: any_file_truncated || total_truncated,
            }
        };
        if let Ok(mut cache) = memory_load_cache().lock() {
            cache.insert(cache_key, result.clone());
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
        ("MEMORY.md", MEMORY_TEMPLATE),
        ("USER.md", USER_TEMPLATE),
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
        memory_mtime: file_mtime(&root.join("MEMORY.md")),
        user_mtime: file_mtime(&root.join("USER.md")),
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
