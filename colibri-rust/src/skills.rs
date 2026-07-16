use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

use crate::config::SkillsConfig;
use crate::messages::ToolResult;
use crate::tools::ToolContext;
use serde::Deserialize;

static SKILL_SCAN_CACHE: OnceLock<Mutex<HashMap<Vec<(String, Option<u128>)>, Arc<SkillIndex>>>> =
    OnceLock::new();

fn skill_scan_cache() -> &'static Mutex<HashMap<Vec<(String, Option<u128>)>, Arc<SkillIndex>>> {
    SKILL_SCAN_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

const CREATE_COLIBRI_SKILL_CONTENT: &str = r#"---
name: create-colibri-skill
description: Guide creating, writing, adding, or designing a local Colibri skill.
---

# Create Colibri Skill

Use this skill when the user wants to create, write, add, or design a local Colibri skill.

Colibri skills are local filesystem instructions. Do not install packages, fetch remote skill catalogs, or use a marketplace.

Create this layout:

```text
~/.colibri/skills/<skill-name>/
  SKILL.md
  scripts/...       # optional
```

`SKILL.md` is required and must start with YAML frontmatter. The `name` must match the skill directory name and `description` must clearly state when the skill is useful:

```yaml
---
name: example-skill
description: Use when the user needs the example workflow.
commands:
  - name: check
    description: Run the local verification command.
    command: python
    args: [scripts/check.py]
    read_only: true
---
```

Keep the Markdown body focused on context gathering and the exact workflow. Prefer progressive disclosure and reference extra local files only when needed. Put reusable implementations under `scripts/`. Do not create `skill.toml`.

After creating a skill, Colibri lists it in the skill catalog. Use `skill.read` with the skill name when you need the full instructions. When a declared command matches the requested action, execute it with `skill.run`. Keep command permissions explicit and avoid long resident processes on small devices.
"#;

#[derive(Debug, Deserialize)]
struct SkillDocumentMetadata {
    name: String,
    description: String,
    #[serde(default)]
    commands: Vec<SkillDocumentCommand>,
}

#[derive(Debug, Deserialize)]
struct SkillDocumentCommand {
    name: String,
    #[serde(default)]
    description: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    read_only: bool,
}

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
                let Some((description, commands)) = parse_skill_document(&first_text, &name)
                else {
                    continue;
                };
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
            "Available skills:".to_string(),
            String::new(),
            "Use skill.read to load full instructions. When a skill has a configured command matching the requested action, use skill.run instead of invoking that command's underlying executable through shell.run.".to_string(),
            String::new(),
        ];
        let mut names = Vec::new();
        for skill in &selected {
            let location = if skill.root.file_name().and_then(|name| name.to_str())
                == Some("builtin")
                && skill.content.is_some()
            {
                "builtin".to_string()
            } else {
                skill.root.display().to_string()
            };
            let command_text = if skill.commands.is_empty() {
                String::new()
            } else {
                format!(
                    " Commands: {}",
                    skill
                        .commands
                        .iter()
                        .map(|command| command.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            lines.push(format!(
                "- {}: {}{} [{}]",
                skill.name, skill.description, command_text, location
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
        let command_text = if skill.commands.is_empty() {
            String::new()
        } else {
            let lines = skill
                .commands
                .iter()
                .map(|command| {
                    if command.description.is_empty() {
                        format!("- {}", command.name)
                    } else {
                        format!("- {}: {}", command.name, command.description)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!("\nConfigured commands:\n{lines}")
        };
        let text = format!(
            "[{}]\nBase directory: {}{}\n\n{}",
            skill.name,
            skill.root.display(),
            command_text,
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
        let file = path.join("SKILL.md");
        let resolved = file
            .canonicalize()
            .unwrap_or_else(|_| file.clone())
            .to_string_lossy()
            .into_owned();
        parts.push((resolved, file_mtime(&file)));
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
    let Some((description, commands)) =
        parse_skill_document(CREATE_COLIBRI_SKILL_CONTENT, &name)
    else {
        return Vec::new();
    };
    vec![SkillMetadata {
        name: name.clone(),
        description,
        skill_file: root.join(&name).join("SKILL.md"),
        root,
        commands,
        content: Some(CREATE_COLIBRI_SKILL_CONTENT.to_string()),
    }]
}

fn parse_skill_document(content: &str, expected_name: &str) -> Option<(String, Vec<SkillCommand>)> {
    let mut lines = content.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let mut frontmatter = Vec::new();
    let mut closed = false;
    for line in lines {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        frontmatter.push(line);
    }
    if !closed {
        return None;
    }
    let metadata =
        serde_yaml::from_str::<SkillDocumentMetadata>(&frontmatter.join("\n")).ok()?;
    if metadata.name.trim() != expected_name || metadata.description.trim().is_empty() {
        return None;
    }
    let mut parsed = Vec::new();
    let mut seen = BTreeSet::new();
    for item in metadata.commands {
        if item.name.trim().is_empty()
            || item.command.trim().is_empty()
            || !seen.insert(item.name.clone())
        {
            return None;
        }
        parsed.push(SkillCommand {
            name: item.name,
            description: item.description,
            command: item.command,
            args: item.args,
            read_only: item.read_only,
        });
    }
    Some((metadata.description, parsed))
}
