use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

use crate::config::SkillsConfig;
use crate::messages::ToolResult;
use crate::tools::ToolContext;

static SKILL_SCAN_CACHE: OnceLock<Mutex<HashMap<Vec<(String, Option<u128>)>, Arc<SkillIndex>>>> =
    OnceLock::new();

fn skill_scan_cache() -> &'static Mutex<HashMap<Vec<(String, Option<u128>)>, Arc<SkillIndex>>> {
    SKILL_SCAN_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

const CREATE_COLIBRI_SKILL_CONTENT: &str = r#"# Create Colibri Skill

Use this skill when the user wants to create, write, add, or design a local Colibri skill.

Colibri skills are local filesystem instructions. Do not install packages, fetch remote skill catalogs, or use a marketplace.

Create this layout:

```text
~/.colibri/skills/<skill-name>/
  SKILL.md
  skill.toml        # optional
  scripts/...       # optional
```

`SKILL.md` is required. Keep it focused on when to use the skill, what context to gather, and the exact workflow the assistant should follow. Prefer progressive disclosure: put the essential instructions in `SKILL.md`, and reference extra local files only when needed.

Optional `skill.toml` can describe local commands for `skill.run`:

```toml
description = "Short description shown in the skill catalog."

[[commands]]
name = "check"
description = "Run the local verification command."
command = "python"
args = ["scripts/check.py"]
read_only = true
```

After creating a skill, Colibri lists it in the skill catalog. Use `skill.read` with the skill name when you need the full instructions. Keep command permissions explicit and avoid long resident processes on small devices.
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillCommand {
    pub name: String,
    pub description: String,
    pub command: String,
    pub args: Vec<String>,
    pub read_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub root: PathBuf,
    pub skill_file: PathBuf,
    pub commands: Vec<SkillCommand>,
    pub content: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillIndex {
    pub skills: Vec<SkillMetadata>,
}

impl SkillIndex {
    pub fn scan(skill_dir: &Path) -> Arc<Self> {
        let fingerprint = dir_fingerprint(skill_dir);
        if let Ok(cache) = skill_scan_cache().lock() {
            if let Some(cached) = cache.get(&fingerprint) {
                return Arc::clone(cached);
            }
        }

        let mut skills = builtin_skills();
        let mut seen = skills
            .iter()
            .map(|skill| skill.name.clone())
            .collect::<BTreeSet<_>>();
        if let Ok(entries) = fs::read_dir(skill_dir) {
            let mut entries = entries.flatten().collect::<Vec<_>>();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if seen.contains(&name) {
                    continue;
                }
                let skill_file = path.join("SKILL.md");
                let Ok(first_text) = fs::read_to_string(&skill_file) else {
                    continue;
                };
                let metadata = read_skill_toml(&path);
                let description = metadata
                    .get("description")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        derive_description(&first_text).unwrap_or_else(|| name.clone())
                    });
                let commands = parse_commands(&metadata);
                skills.push(SkillMetadata {
                    name: name.clone(),
                    description,
                    root: path,
                    skill_file,
                    commands,
                    content: None,
                });
                seen.insert(name);
            }
        }
        let index = Arc::new(Self { skills });
        if let Ok(mut cache) = skill_scan_cache().lock() {
            cache.insert(fingerprint, Arc::clone(&index));
        }
        index
    }

    pub fn get(&self, name: &str) -> Option<&SkillMetadata> {
        self.skills.iter().find(|skill| skill.name == name)
    }

    pub fn catalog(&self, config: &SkillsConfig) -> (String, Vec<String>, bool) {
        if config.max_catalog == 0 {
            return (String::new(), Vec::new(), false);
        }
        let selected: Vec<&SkillMetadata> = self.skills.iter().take(config.max_catalog).collect();
        if selected.is_empty() {
            return (String::new(), Vec::new(), false);
        }
        let mut lines = vec![
            "Available skills (use skill.read with name when needed):".to_string(),
            String::new(),
        ];
        let mut names = Vec::new();
        for skill in &selected {
            let location = if skill.root.file_name().and_then(|name| name.to_str())
                == Some("builtin")
                && skill.content.is_some()
            {
                "[builtin]".to_string()
            } else {
                skill.root.display().to_string()
            };
            lines.push(format!(
                "- {}: {} [{}]",
                skill.name, skill.description, location
            ));
            names.push(skill.name.clone());
        }
        let mut text = lines.join("\n").trim().to_string();
        let truncated = text.chars().count() > config.max_catalog_chars;
        if truncated {
            text = bound_skill_text(&text, config.max_catalog_chars);
        }
        (text, names, truncated)
    }

    pub fn read_text(&self, name: &str, max_chars: usize) -> Option<(String, bool)> {
        let skill = self.get(name)?;
        let content = skill
            .content
            .clone()
            .or_else(|| fs::read_to_string(&skill.skill_file).ok())?;
        let text = format!(
            "[{}]\nBase directory: {}\n\n{}",
            skill.name,
            skill.root.display(),
            content.trim()
        );
        let truncated = text.chars().count() > max_chars;
        if truncated {
            Some((bound_skill_text(&text, max_chars), true))
        } else {
            Some((text, false))
        }
    }
}

pub fn skill_catalog(context: &ToolContext) -> (String, Vec<String>, bool) {
    SkillIndex::scan(&context.config.skills.dir).catalog(&context.config.skills)
}

pub fn read_skill(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some(name) = args
        .get("name")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return ToolResult::error("invalid_arguments", "name is required");
    };
    let index = SkillIndex::scan(&context.config.skills.dir);
    match index.read_text(name, context.config.skills.max_instruction_chars) {
        Some((text, truncated)) => {
            let mut result = ToolResult::ok(text);
            result.truncated = truncated;
            result
        }
        None => {
            let available = index
                .skills
                .iter()
                .take(20)
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let available = if available.is_empty() {
                "none".to_string()
            } else {
                available
            };
            ToolResult::error(
                "not_found",
                format!("Unknown skill: {name}. Available: {available}"),
            )
        }
    }
}

pub fn run_skill_command(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some(skill_name) = args.get("skill") else {
        return ToolResult::error("invalid_arguments", "skill and command are required");
    };
    let Some(command_name) = args.get("command") else {
        return ToolResult::error("invalid_arguments", "skill and command are required");
    };
    let extra_args = parse_string_list_arg(args.get("args"));
    let index = SkillIndex::scan(&context.config.skills.dir);
    let Some(skill) = index.get(skill_name) else {
        return ToolResult::error("not_found", format!("Unknown skill: {skill_name}"));
    };
    let Some(command) = skill
        .commands
        .iter()
        .find(|command| command.name == *command_name)
    else {
        return ToolResult::error(
            "not_found",
            format!("Unknown skill command: {command_name}"),
        );
    };
    if command.command.is_empty() {
        return ToolResult::error("invalid_config", "Skill command is empty");
    }
    let mut argv = command.args.clone();
    argv.extend(extra_args);
    let mut child = Command::new(&command.command);
    child.args(&argv).current_dir(&skill.root);
    let timeout = Duration::from_secs_f64(context.config.tools.max_shell_seconds.max(0.001));
    match run_command_with_timeout(child, timeout) {
        Ok((status, stdout, stderr)) => {
            let text = if stdout.is_empty() { stderr } else { stdout };
            let (bounded, truncated) = truncate_result(text, context.config.tools.max_result_chars);
            if status {
                let mut result = ToolResult::ok(bounded);
                result.truncated = truncated;
                result
            } else {
                let mut result = ToolResult::error("tool_error", bounded);
                result.truncated = truncated;
                result
            }
        }
        Err(error) if error == "timeout" => {
            let mut result = ToolResult::error("timeout", "Skill command timed out");
            result.truncated = false;
            result
        }
        Err(error) => ToolResult::error("tool_error", error),
    }
}

fn parse_string_list_arg(raw: Option<&String>) -> Vec<String> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    match value.as_array() {
        Some(items) if items.iter().all(|item| item.as_str().is_some()) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn truncate_result(text: String, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        (text, false)
    } else {
        (bound_skill_text(&text, max_chars), true)
    }
}

fn bound_skill_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let suffix = "\n...[truncated]";
    let keep = max_chars.saturating_sub(suffix.chars().count());
    text.chars().take(keep).collect::<String>() + suffix
}

fn run_command_with_timeout(
    mut command: Command,
    timeout: Duration,
) -> Result<(bool, String, String), String> {
    use std::io::Read;
    use std::process::Stdio;

    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_string(&mut stdout);
                }
                if let Some(mut err) = child.stderr.take() {
                    let _ = err.read_to_string(&mut stderr);
                }
                return Ok((status.success(), stdout, stderr));
            }
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("timeout".to_string());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn dir_fingerprint(skill_dir: &Path) -> Vec<(String, Option<u128>)> {
    let mut parts = Vec::new();
    let resolved = skill_dir
        .canonicalize()
        .unwrap_or_else(|_| skill_dir.to_path_buf())
        .to_string_lossy()
        .into_owned();
    parts.push((resolved, None));
    let Ok(entries) = fs::read_dir(skill_dir) else {
        return parts;
    };
    let mut entries = entries.flatten().collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        for name in ["SKILL.md", "skill.toml"] {
            let file = path.join(name);
            let resolved = file
                .canonicalize()
                .unwrap_or_else(|_| file.clone())
                .to_string_lossy()
                .into_owned();
            parts.push((resolved, file_mtime(&file)));
        }
    }
    parts
}

fn file_mtime(path: &Path) -> Option<u128> {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
}

fn builtin_skills() -> Vec<SkillMetadata> {
    let name = "create-colibri-skill".to_string();
    let root = PathBuf::from("builtin");
    vec![SkillMetadata {
        name: name.clone(),
        description: "Guide creating, writing, adding, or designing a local Colibri skill."
            .to_string(),
        skill_file: root.join(&name).join("SKILL.md"),
        root,
        commands: Vec::new(),
        content: Some(CREATE_COLIBRI_SKILL_CONTENT.to_string()),
    }]
}

fn read_skill_toml(root: &std::path::Path) -> toml::Table {
    let path = root.join("skill.toml");
    let Ok(text) = fs::read_to_string(path) else {
        return toml::Table::new();
    };
    text.parse::<toml::Table>().unwrap_or_default()
}

fn parse_commands(metadata: &toml::Table) -> Vec<SkillCommand> {
    let Some(commands) = metadata.get("commands").and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut parsed = Vec::new();
    for item in commands {
        let Some(table) = item.as_table() else {
            continue;
        };
        let Some(name) = table.get("name").and_then(|value| value.as_str()) else {
            continue;
        };
        let Some(command) = table.get("command").and_then(|value| value.as_str()) else {
            continue;
        };
        let args = match table.get("args") {
            None => Vec::new(),
            Some(value) => match value.as_array() {
                Some(items) if items.iter().all(|item| item.as_str().is_some()) => items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect(),
                _ => Vec::new(),
            },
        };
        parsed.push(SkillCommand {
            name: name.to_string(),
            description: table
                .get("description")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
            command: command.to_string(),
            args,
            read_only: table
                .get("read_only")
                .and_then(|value| value.as_bool())
                .unwrap_or(false),
        });
    }
    parsed
}

fn derive_description(content: &str) -> Option<String> {
    for line in content.lines() {
        let stripped = line.trim();
        if stripped.is_empty() {
            continue;
        }
        if stripped.starts_with('#') {
            return Some(stripped.trim_start_matches('#').trim().to_string());
        }
        return Some(stripped.to_string());
    }
    None
}
