use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::config::SkillsConfig;
use crate::messages::ToolResult;
use crate::tools::ToolContext;

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
description = "Short description used for skill selection."

[[commands]]
name = "check"
description = "Run the local verification command."
command = "python"
args = ["scripts/check.py"]
read_only = true
```

After creating a skill, test that Colibri selects it for a matching user request and does not select it for unrelated turns. Keep command permissions explicit and avoid long resident processes on small devices.
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
    pub fn scan(dirs: &[PathBuf]) -> Self {
        let mut skills = builtin_skills();
        let mut seen = skills
            .iter()
            .map(|skill| skill.name.clone())
            .collect::<BTreeSet<_>>();
        for root in dirs {
            let Ok(entries) = fs::read_dir(root) else {
                continue;
            };
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
                let metadata = fs::read_to_string(path.join("skill.toml")).unwrap_or_default();
                let description = parse_top_level_string(&metadata, "description")
                    .filter(|value| !value.is_empty())
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
        Self { skills }
    }

    pub fn get(&self, name: &str) -> Option<&SkillMetadata> {
        self.skills.iter().find(|skill| skill.name == name)
    }

    pub fn context_for(
        &self,
        user_text: &str,
        config: &SkillsConfig,
    ) -> (String, Vec<String>, bool) {
        let selected = self.select(user_text, config.max_loaded);
        if selected.is_empty() {
            return (String::new(), Vec::new(), false);
        }
        let mut chunks = vec!["Relevant skills:".to_string()];
        let mut names = Vec::new();
        for skill in selected {
            let content = skill
                .content
                .clone()
                .unwrap_or_else(|| fs::read_to_string(&skill.skill_file).unwrap_or_default());
            if content.is_empty() {
                continue;
            }
            chunks.push(format!(
                "\n[{}]\nBase directory: {}\n\n{}",
                skill.name,
                skill.root.display(),
                content.trim()
            ));
            names.push(skill.name.clone());
        }
        let mut text = chunks.join("\n").trim().to_string();
        if text.is_empty() {
            return (String::new(), Vec::new(), false);
        }
        let truncated = text.chars().count() > config.max_instruction_chars;
        if truncated {
            text = text
                .chars()
                .take(config.max_instruction_chars.saturating_sub(15))
                .collect::<String>()
                + "\n...[truncated]";
        }
        (text, names, truncated)
    }

    pub fn select(&self, user_text: &str, limit: usize) -> Vec<&SkillMetadata> {
        if limit == 0 {
            return Vec::new();
        }
        let query_terms = terms(user_text);
        let mut scored = Vec::new();
        for skill in &self.skills {
            let score = skill_score(skill, &query_terms, user_text);
            if score > 0 {
                scored.push((score, skill.name.clone(), skill));
            }
        }
        scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        scored
            .into_iter()
            .take(limit)
            .map(|(_, _, skill)| skill)
            .collect()
    }
}

pub fn relevant_skill_context(prompt: &str, context: &ToolContext) -> (String, Vec<String>, bool) {
    SkillIndex::scan(&context.config.skills.dirs).context_for(prompt, &context.config.skills)
}

pub fn run_skill_command(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some(skill_name) = args.get("skill") else {
        return ToolResult::error("invalid_arguments", "skill is required");
    };
    let Some(command_name) = args.get("command") else {
        return ToolResult::error("invalid_arguments", "command is required");
    };
    let index = SkillIndex::scan(&context.config.skills.dirs);
    let Some(skill) = index.get(skill_name) else {
        return ToolResult::error(
            "not_found",
            format!("Skill command not found: {} {}", skill_name, command_name),
        );
    };
    let Some(command) = skill
        .commands
        .iter()
        .find(|command| command.name == *command_name)
    else {
        return ToolResult::error(
            "not_found",
            format!("Skill command not found: {} {}", skill_name, command_name),
        );
    };
    let output = Command::new(&command.command)
        .args(&command.args)
        .current_dir(&skill.root)
        .output();
    match output {
        Ok(output) => {
            let mut text = String::new();
            text.push_str(&String::from_utf8_lossy(&output.stdout));
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            if output.status.success() {
                ToolResult::ok(text)
            } else {
                ToolResult::error("process_error", text)
            }
        }
        Err(error) => ToolResult::error("io_error", error.to_string()),
    }
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

fn parse_commands(text: &str) -> Vec<SkillCommand> {
    let mut commands = Vec::new();
    let mut current = BTreeMap::new();
    let mut in_command = false;
    for raw in text.lines().chain(std::iter::once("[[commands]]")) {
        let line = raw.trim();
        if line == "[[commands]]" {
            if in_command {
                if let (Some(name), Some(command)) = (
                    current.get("name").cloned(),
                    current.get("command").cloned(),
                ) {
                    commands.push(SkillCommand {
                        name,
                        description: current.get("description").cloned().unwrap_or_default(),
                        command,
                        args: parse_inline_list(
                            current.get("args").map(String::as_str).unwrap_or("[]"),
                        ),
                        read_only: current
                            .get("read_only")
                            .is_some_and(|value| value == "true"),
                    });
                }
            }
            current.clear();
            in_command = true;
            continue;
        }
        if !in_command {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            let parsed = if key == "args" {
                value.to_string()
            } else {
                parse_string(value)
            };
            current.insert(key.to_string(), parsed);
        }
    }
    commands
}

fn parse_top_level_string(text: &str, target: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line == "[[commands]]" {
            return None;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == target {
            return Some(parse_string(value.trim()));
        }
    }
    None
}

fn parse_string(value: &str) -> String {
    value.trim().trim_matches('"').to_string()
}

fn parse_inline_list(value: &str) -> Vec<String> {
    value
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|item| parse_string(item.trim()))
        .filter(|item| !item.is_empty())
        .collect()
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

fn skill_score(skill: &SkillMetadata, query_terms: &BTreeSet<String>, user_text: &str) -> usize {
    if skill.name == "create-colibri-skill" {
        if !is_create_skill_request(user_text) {
            return 0;
        }
        return 100
            + query_terms
                .intersection(&terms(&format!("{} {}", skill.name, skill.description)))
                .count();
    }
    let haystack = terms(&format!("{} {}", skill.name, skill.description));
    query_terms.intersection(&haystack).count()
}

fn is_create_skill_request(user_text: &str) -> bool {
    let lowered = user_text.to_lowercase();
    let term_set = terms(&lowered);
    let has_skill_term =
        term_set.contains("skill") || term_set.contains("skills") || lowered.contains("技能");
    if !has_skill_term {
        return false;
    }
    [
        "create", "new", "add", "write", "design", "build", "创建", "新增", "添加", "编写", "设计",
    ]
    .iter()
    .any(|word| lowered.contains(word))
}

fn terms(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            current.push(ch.to_ascii_lowercase());
        } else if current.len() > 1 {
            out.insert(std::mem::take(&mut current));
        } else {
            current.clear();
        }
    }
    if current.len() > 1 {
        out.insert(current);
    }
    out
}
