use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{expand_user_path, AgentConfig};
use crate::tools::{ToolContext, ToolInfo};

const DEFAULT_USER_PERMISSIONS: &str = "~/.colibri/permissions.toml";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserGrants {
    pub shell_commands: Vec<String>,
    pub shell_executables: Vec<String>,
    pub tool_names: Vec<String>,
    pub file_roots: Vec<String>,
}

pub struct UserPermissionStore {
    pub path: PathBuf,
}

impl UserPermissionStore {
    pub fn for_user() -> Self {
        Self {
            path: expand_user_path(DEFAULT_USER_PERMISSIONS),
        }
    }

    pub fn for_cwd(cwd: PathBuf) -> Self {
        let _ = cwd;
        Self::for_user()
    }

    pub fn load(&self) -> UserGrants {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return UserGrants::default();
        };
        let Ok(value) = text.parse::<toml::Value>() else {
            return UserGrants::default();
        };
        UserGrants {
            shell_commands: string_list_at(&value, &["shell", "commands"]),
            shell_executables: merged_string_lists_at(
                &value,
                &[&["shell", "executables"], &["shell", "prefixes"]],
            ),
            tool_names: string_list_at(&value, &["tools", "names"]),
            file_roots: string_list_at(&value, &["files", "roots"]),
        }
    }

    pub fn save(&self, grants: &UserGrants) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut shell_commands = sorted_dedup(grants.shell_commands.clone());
        let mut shell_executables = sorted_dedup(grants.shell_executables.clone());
        let mut tool_names = sorted_dedup(grants.tool_names.clone());
        let mut file_roots = sorted_dedup(grants.file_roots.clone());
        let text = format!(
            "[shell]\ncommands = [{}]\nexecutables = [{}]\n\n[tools]\nnames = [{}]\n\n[files]\nroots = [{}]\n",
            toml_array(&mut shell_commands),
            toml_array(&mut shell_executables),
            toml_array(&mut tool_names),
            toml_array(&mut file_roots)
        );
        fs::write(&self.path, text).map_err(|error| error.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct PermissionRequest {
    pub tool_name: String,
    pub arguments: BTreeMap<String, String>,
    pub read_only: bool,
    pub subject_kind: String,
    pub shell_command: Option<String>,
    pub shell_executable: Option<String>,
    pub file_path: Option<String>,
    pub file_root: Option<String>,
}

pub trait PermissionPrompter {
    fn confirm(&mut self, request: PermissionRequest) -> String;
}

#[derive(Clone, Debug)]
pub struct PermissionDecision {
    pub allowed: bool,
    pub decision: String,
    pub scope: String,
    pub reason: String,
    pub subject_kind: String,
    pub file_path: Option<String>,
    pub file_root: Option<String>,
}

#[derive(Clone, Debug)]
struct PermissionSubject {
    kind: String,
    tool_name: String,
    shell_command: Option<String>,
    shell_executable: Option<String>,
    file_path: Option<String>,
    file_root: Option<String>,
    read_only: bool,
}

pub struct PermissionPolicy<'a> {
    default_permission: String,
    user_store: UserPermissionStore,
    prompter: Option<&'a mut dyn PermissionPrompter>,
    session_tool_grants: Vec<String>,
    session_shell_commands: Vec<String>,
    session_shell_executables: Vec<String>,
    session_file_roots: Vec<String>,
}

impl<'a> PermissionPolicy<'a> {
    pub fn from_config(
        config: &AgentConfig,
        cwd: PathBuf,
        prompter: Option<&'a mut dyn PermissionPrompter>,
    ) -> Self {
        let _ = cwd;
        Self {
            default_permission: config.tools.default_permission.clone(),
            user_store: UserPermissionStore::for_user(),
            prompter,
            session_tool_grants: Vec::new(),
            session_shell_commands: Vec::new(),
            session_shell_executables: Vec::new(),
            session_file_roots: Vec::new(),
        }
    }

    pub fn decide(
        &mut self,
        tool: &ToolInfo,
        arguments: &BTreeMap<String, String>,
        context: &ToolContext,
    ) -> PermissionDecision {
        let subject = permission_subject_for_internal(tool, arguments, context);
        let hard_denied = if subject.tool_name == "shell.run" {
            subject.shell_command.as_ref().is_some_and(|command| {
                crate::shell_policy::denied_shell_executable(command, &context.config.shell.deny)
                    .is_some()
            }) || subject
                .shell_executable
                .as_ref()
                .is_some_and(|executable| context.config.shell.deny.contains(executable))
        } else {
            false
        };
        if hard_denied {
            return decision(false, "deny", "none", &subject, "hard_deny");
        }

        let grants = self.user_store.load();
        if let Some(granted) = self.granted(&subject, &grants) {
            return granted;
        }
        if let Some(default) = self.default_decision(&subject) {
            return default;
        }

        let request = PermissionRequest {
            tool_name: subject.tool_name.clone(),
            arguments: arguments.clone(),
            read_only: subject.read_only,
            subject_kind: subject.kind.clone(),
            shell_command: subject.shell_command.clone(),
            shell_executable: subject.shell_executable.clone(),
            file_path: subject.file_path.clone(),
            file_root: subject.file_root.clone(),
        };
        let choice = self
            .prompter
            .as_mut()
            .map(|prompter| prompter.confirm(request))
            .unwrap_or_else(|| "0".to_string());
        self.apply_choice(&permission_choice(&choice), &subject, &grants)
    }

    fn granted(
        &self,
        subject: &PermissionSubject,
        grants: &UserGrants,
    ) -> Option<PermissionDecision> {
        if subject.kind == "shell" {
            if contains_opt(&self.session_shell_commands, subject.shell_command.as_ref()) {
                return Some(decision(true, "allow", "session", subject, ""));
            }
            if contains_opt(
                &self.session_shell_executables,
                subject.shell_executable.as_ref(),
            ) {
                return Some(decision(true, "allow", "session_executable", subject, ""));
            }
            if contains_opt(&grants.shell_commands, subject.shell_command.as_ref()) {
                return Some(decision(true, "allow", "user", subject, ""));
            }
            if shell_command_matches_user_executables(subject.shell_command.as_deref(), grants) {
                return Some(decision(true, "allow", "user_executable", subject, ""));
            }
            return None;
        }
        if subject.kind == "file_path" {
            if path_under_any_root(subject.file_path.as_ref(), &self.session_file_roots) {
                return Some(decision(true, "allow", "session_file_root", subject, ""));
            }
            if path_under_any_root(subject.file_path.as_ref(), &grants.file_roots) {
                return Some(decision(true, "allow", "user_file_root", subject, ""));
            }
            return None;
        }
        if self.session_tool_grants.contains(&subject.tool_name) {
            return Some(decision(true, "allow", "session", subject, ""));
        }
        if grants.tool_names.contains(&subject.tool_name) {
            return Some(decision(true, "allow", "user", subject, ""));
        }
        None
    }

    fn default_decision(&self, subject: &PermissionSubject) -> Option<PermissionDecision> {
        match self.default_permission.as_str() {
            "allow" => Some(decision(true, "allow", "default", subject, "")),
            "deny" => Some(decision(false, "deny", "default", subject, "")),
            "confirm" => None,
            "allow_read_confirm_write"
                if subject.kind != "shell" && subject.kind != "file_path" && subject.read_only =>
            {
                Some(decision(true, "allow", "default_read_only", subject, ""))
            }
            _ => None,
        }
    }

    fn apply_choice(
        &mut self,
        choice: &str,
        subject: &PermissionSubject,
        grants: &UserGrants,
    ) -> PermissionDecision {
        if choice == "1" {
            return decision(true, "allow", "once", subject, "");
        }
        if choice == "2" {
            if subject.kind == "shell" {
                push_unique_opt(
                    &mut self.session_shell_commands,
                    subject.shell_command.clone(),
                );
            } else if subject.kind == "file_path" {
                push_unique_opt(&mut self.session_file_roots, subject.file_root.clone());
            } else {
                push_unique(&mut self.session_tool_grants, subject.tool_name.clone());
            }
            let scope = if subject.kind == "file_path" {
                "session_file_root"
            } else {
                "session"
            };
            return decision(true, "allow", scope, subject, "");
        }
        if choice == "3" && subject.kind == "shell" {
            push_unique_opt(
                &mut self.session_shell_executables,
                subject.shell_executable.clone(),
            );
            return decision(true, "allow", "session_executable", subject, "");
        }
        if choice == "5" && subject.kind == "shell" {
            let mut next = grants.clone();
            push_unique_opt(
                &mut next.shell_executables,
                subject.shell_executable.clone(),
            );
            let _ = self.user_store.save(&next);
            return decision(true, "allow", "user_executable", subject, "");
        }
        if choice == "4" {
            let mut next = grants.clone();
            if subject.kind == "shell" {
                push_unique_opt(&mut next.shell_commands, subject.shell_command.clone());
            } else if subject.kind == "file_path" {
                push_unique_opt(&mut next.file_roots, subject.file_root.clone());
            } else {
                push_unique(&mut next.tool_names, subject.tool_name.clone());
            }
            let _ = self.user_store.save(&next);
            let scope = if subject.kind == "file_path" {
                "user_file_root"
            } else {
                "user"
            };
            return decision(true, "allow", scope, subject, "");
        }
        decision(false, "deny", "once", subject, "user_denied")
    }
}

fn permission_choice(reply: &str) -> String {
    let first = reply.split_whitespace().next().unwrap_or("0");
    if matches!(first, "0" | "1" | "2" | "3" | "4" | "5") {
        first.to_string()
    } else {
        "0".to_string()
    }
}

pub fn permission_subject_for(
    tool: &ToolInfo,
    arguments: &BTreeMap<String, String>,
    context: &ToolContext,
) -> PermissionRequest {
    let subject = permission_subject_for_internal(tool, arguments, context);
    PermissionRequest {
        tool_name: subject.tool_name,
        arguments: arguments.clone(),
        read_only: subject.read_only,
        subject_kind: subject.kind,
        shell_command: subject.shell_command,
        shell_executable: subject.shell_executable,
        file_path: subject.file_path,
        file_root: subject.file_root,
    }
}

fn permission_subject_for_internal(
    tool: &ToolInfo,
    arguments: &BTreeMap<String, String>,
    context: &ToolContext,
) -> PermissionSubject {
    if tool.name == "shell.run" {
        let command = arguments
            .get("command")
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        let argv = shell_words::split(&command).unwrap_or_default();
        let executable = crate::shell_policy::first_shell_executable(&command);
        if let Some(write_path) = shell_write_path(&command, &argv, context) {
            let root = grant_root_for(&write_path);
            return PermissionSubject {
                kind: "file_path".to_string(),
                tool_name: tool.name.clone(),
                shell_command: Some(command),
                shell_executable: executable,
                file_path: Some(write_path.display().to_string()),
                file_root: Some(root.display().to_string()),
                read_only: false,
            };
        }
        return PermissionSubject {
            kind: "shell".to_string(),
            tool_name: tool.name.clone(),
            shell_command: Some(command),
            shell_executable: executable,
            file_path: None,
            file_root: None,
            read_only: false,
        };
    }
    if matches!(
        tool.name.as_str(),
        "files.list" | "files.read" | "files.write" | "files.send" | "image.understand"
    ) {
        if let Some(path) = arguments.get("path") {
            let resolved = resolve_path(path, &context.cwd);
            let outside = !crate::tools::is_allowed(&resolved, context);
            if tool.name == "files.write" || tool.name == "files.send" || outside {
                let root = grant_root_for(&resolved);
                return PermissionSubject {
                    kind: "file_path".to_string(),
                    tool_name: tool.name.clone(),
                    shell_command: None,
                    shell_executable: None,
                    file_path: Some(resolved.display().to_string()),
                    file_root: Some(root.display().to_string()),
                    read_only: tool.read_only,
                };
            }
        }
    }
    PermissionSubject {
        kind: "tool".to_string(),
        tool_name: tool.name.clone(),
        shell_command: None,
        shell_executable: None,
        file_path: None,
        file_root: None,
        read_only: tool.read_only,
    }
}

pub fn decide_tool_permission(
    config: &AgentConfig,
    tool_name: &str,
    arguments: &BTreeMap<String, String>,
    context: &ToolContext,
) -> PermissionDecision {
    let tool = crate::tools::tool_info(tool_name);
    let mut policy = PermissionPolicy::from_config(config, context.cwd.clone(), None);
    policy.decide(&tool, arguments, context)
}

fn decision(
    allowed: bool,
    decision_text: &str,
    scope: &str,
    subject: &PermissionSubject,
    reason: &str,
) -> PermissionDecision {
    PermissionDecision {
        allowed,
        decision: decision_text.to_string(),
        scope: scope.to_string(),
        reason: reason.to_string(),
        subject_kind: subject.kind.clone(),
        file_path: subject.file_path.clone(),
        file_root: subject.file_root.clone(),
    }
}

fn shell_command_matches_user_executables(command: Option<&str>, grants: &UserGrants) -> bool {
    let Some(command) = command else {
        return false;
    };
    if grants.shell_executables.is_empty() {
        return false;
    }
    if crate::shell_policy::has_dangerous_shell_features(command) {
        return false;
    }
    let segments = crate::shell_policy::shell_command_segments(command);
    if segments.is_empty() {
        return false;
    }
    segments.iter().all(|segment| {
        grants.shell_commands.contains(segment)
            || grants
                .shell_executables
                .iter()
                .any(|executable| command_executable_matches(segment, executable))
    })
}

fn command_executable_matches(command: &str, executable: &str) -> bool {
    let command = command.trim();
    let executable = executable.trim();
    !executable.is_empty()
        && (command == executable || command.starts_with(&format!("{executable} ")))
}

fn string_list_at(value: &toml::Value, path: &[&str]) -> Vec<String> {
    let mut current = value;
    for key in path {
        let Some(next) = current.get(*key) else {
            return Vec::new();
        };
        current = next;
    }
    current
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn merged_string_lists_at(value: &toml::Value, paths: &[&[&str]]) -> Vec<String> {
    paths
        .iter()
        .flat_map(|path| string_list_at(value, path))
        .collect()
}

fn sorted_dedup(mut items: Vec<String>) -> Vec<String> {
    items.sort();
    items.dedup();
    items
}

fn toml_array(items: &mut Vec<String>) -> String {
    *items = sorted_dedup(std::mem::take(items));
    items
        .iter()
        .map(|item| format!("\"{}\"", item.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(", ")
}

fn resolve_path(path: &str, cwd: &Path) -> PathBuf {
    let path = expand_user_path(path);
    let joined = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    joined.canonicalize().unwrap_or(joined)
}

fn shell_write_path(command: &str, argv: &[String], context: &ToolContext) -> Option<PathBuf> {
    if command.is_empty() {
        return None;
    }
    redirection_target(argv).map(|target| resolve_path(&target, &context.cwd))
}

fn redirection_target(argv: &[String]) -> Option<String> {
    let redirect_ops = [">>", "1>>", "2>>", "&>>", ">", "1>", "2>", "&>"];
    for (index, token) in argv.iter().enumerate() {
        if redirect_ops.contains(&token.as_str()) {
            return argv.get(index + 1).cloned();
        }
        for op in redirect_ops {
            if token.starts_with(op) && token.len() > op.len() {
                return Some(token[op.len()..].to_string());
            }
        }
    }
    if argv.first().is_some_and(|item| item == "tee") {
        return argv
            .iter()
            .skip(1)
            .find(|token| !token.starts_with('-'))
            .cloned();
    }
    None
}

fn grant_root_for(path: &Path) -> PathBuf {
    if path.exists() && path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(path).to_path_buf()
    }
}

fn path_under_any_root(path: Option<&String>, roots: &[String]) -> bool {
    let Some(path) = path else {
        return false;
    };
    let path = PathBuf::from(path)
        .canonicalize()
        .unwrap_or(PathBuf::from(path));
    roots.iter().any(|root| {
        let root = PathBuf::from(root)
            .canonicalize()
            .unwrap_or(PathBuf::from(root));
        path.starts_with(root)
    })
}

fn contains_opt(items: &[String], value: Option<&String>) -> bool {
    value.is_some_and(|value| items.contains(value))
}

fn push_unique(items: &mut Vec<String>, value: String) {
    if !items.contains(&value) {
        items.push(value);
    }
}

fn push_unique_opt(items: &mut Vec<String>, value: Option<String>) {
    if let Some(value) = value {
        push_unique(items, value);
    }
}
