use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::config::{expand_user_path, AgentConfig};
use crate::tools::{ToolContext, ToolInfo};

const DEFAULT_USER_PERMISSIONS: &str = "~/.colibri/permissions.toml";
static NEXT_PERMISSION_TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserGrants {
    pub shell_commands: Vec<String>,
    pub shell_executables: Vec<String>,
    pub tool_names: Vec<String>,
    pub file_roots: Vec<String>,
}

pub struct UserPermissionStore {
    pub path: PathBuf,
    cache: Mutex<PermissionCache>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileFingerprint {
    device: u64,
    inode: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    size: u64,
}

#[derive(Default)]
struct PermissionCache {
    loaded: bool,
    fingerprint: Option<FileFingerprint>,
    grants: UserGrants,
}

impl UserPermissionStore {
    pub fn for_user() -> Self {
        Self {
            path: expand_user_path(DEFAULT_USER_PERMISSIONS),
            cache: Mutex::new(PermissionCache::default()),
        }
    }

    pub fn for_cwd(cwd: PathBuf) -> Self {
        let _ = cwd;
        Self::for_user()
    }

    pub fn load(&self) -> UserGrants {
        let fingerprint = self.fingerprint();
        {
            let cache = self.cache();
            if cache.loaded && cache.fingerprint == fingerprint {
                return cache.grants.clone();
            }
        }
        let (grants, fingerprint) = self.load_stable();
        self.set_cache(&grants, fingerprint);
        grants
    }

    fn load_uncached(&self) -> UserGrants {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return UserGrants::default();
        };
        let Ok(value) = text.parse::<toml::Value>() else {
            return UserGrants::default();
        };
        UserGrants {
            shell_commands: string_list_at(&value, &["shell", "commands"]),
            shell_executables: string_list_at(&value, &["shell", "executables"]),
            tool_names: string_list_at(&value, &["tools", "names"]),
            file_roots: string_list_at(&value, &["files", "roots"]),
        }
    }

    pub fn save(&self, grants: &UserGrants) -> Result<(), String> {
        self.create_parent()?;
        let _lock = PermissionFileLock::acquire(&self.lock_path())?;
        let normalized = normalize_grants(grants);
        self.write_atomic(&normalized)?;
        self.set_cache(&normalized, self.fingerprint());
        Ok(())
    }

    pub fn merge(&self, delta: &UserGrants) -> Result<UserGrants, String> {
        self.create_parent()?;
        let _lock = PermissionFileLock::acquire(&self.lock_path())?;
        let current = self.load_uncached();
        let merged = UserGrants {
            shell_commands: union_strings(&current.shell_commands, &delta.shell_commands),
            shell_executables: union_strings(&current.shell_executables, &delta.shell_executables),
            tool_names: union_strings(&current.tool_names, &delta.tool_names),
            file_roots: union_strings(&current.file_roots, &delta.file_roots),
        };
        self.write_atomic(&merged)?;
        self.set_cache(&merged, self.fingerprint());
        Ok(merged)
    }

    fn load_stable(&self) -> (UserGrants, Option<FileFingerprint>) {
        for _ in 0..2 {
            let before = self.fingerprint();
            let grants = self.load_uncached();
            let after = self.fingerprint();
            if before == after {
                return (grants, after);
            }
        }
        (self.load_uncached(), self.fingerprint())
    }

    fn fingerprint(&self) -> Option<FileFingerprint> {
        let metadata = fs::metadata(&self.path).ok()?;
        Some(FileFingerprint {
            device: metadata.dev(),
            inode: metadata.ino(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            size: metadata.size(),
        })
    }

    fn cache(&self) -> MutexGuard<'_, PermissionCache> {
        self.cache.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn set_cache(&self, grants: &UserGrants, fingerprint: Option<FileFingerprint>) {
        let mut cache = self.cache();
        cache.loaded = true;
        cache.fingerprint = fingerprint;
        cache.grants = grants.clone();
    }

    fn create_parent(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn lock_path(&self) -> PathBuf {
        let filename = self
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("permissions.toml");
        self.path.with_file_name(format!("{filename}.lock"))
    }

    fn write_atomic(&self, grants: &UserGrants) -> Result<(), String> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "Permission file has no parent directory".to_string())?;
        let text = format_grants(grants);
        let temp_id = NEXT_PERMISSION_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(
            ".permissions.{}.{}.tmp",
            std::process::id(),
            temp_id
        ));
        let result = (|| -> Result<(), String> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)
                .map_err(|error| error.to_string())?;
            file.write_all(text.as_bytes())
                .map_err(|error| error.to_string())?;
            file.flush().map_err(|error| error.to_string())?;
            drop(file);
            fs::rename(&temp_path, &self.path).map_err(|error| error.to_string())
        })();
        let _ = fs::remove_file(&temp_path);
        result
    }
}

struct PermissionFileLock {
    file: File,
}

impl PermissionFileLock {
    fn acquire(path: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)
            .map_err(|error| error.to_string())?;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if result != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(Self { file })
    }
}

impl Drop for PermissionFileLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
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

pub struct PermissionPolicy {
    default_permission: String,
    user_store: UserPermissionStore,
    session_tool_grants: Vec<String>,
    session_shell_commands: Vec<String>,
    session_shell_executables: Vec<String>,
    session_file_roots: Vec<String>,
}

impl PermissionPolicy {
    pub fn from_config(config: &AgentConfig, cwd: PathBuf) -> Self {
        let _ = cwd;
        Self {
            default_permission: config.tools.default_permission.clone(),
            user_store: UserPermissionStore::for_user(),
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
        prompter: Option<&mut dyn PermissionPrompter>,
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
        let choice = prompter
            .map(|prompter| prompter.confirm(request))
            .unwrap_or_else(|| "0".to_string());
        self.apply_choice(&permission_choice(&choice), &subject)
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
            if shell_command_matches_executables(
                subject.shell_command.as_deref(),
                &self.session_shell_commands,
                &self.session_shell_executables,
            ) {
                return Some(decision(true, "allow", "session_executable", subject, ""));
            }
            if contains_opt(&grants.shell_commands, subject.shell_command.as_ref()) {
                return Some(decision(true, "allow", "user", subject, ""));
            }
            if shell_command_matches_executables(
                subject.shell_command.as_deref(),
                &grants.shell_commands,
                &grants.shell_executables,
            ) {
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

    fn apply_choice(&mut self, choice: &str, subject: &PermissionSubject) -> PermissionDecision {
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
            for executable in subject_shell_executables(subject) {
                push_unique(&mut self.session_shell_executables, executable);
            }
            return decision(true, "allow", "session_executable", subject, "");
        }
        if choice == "5" && subject.kind == "shell" {
            let mut delta = UserGrants::default();
            for executable in subject_shell_executables(subject) {
                push_unique(&mut delta.shell_executables, executable);
            }
            if self.user_store.merge(&delta).is_err() {
                return decision(false, "deny", "none", subject, "persist_error");
            }
            return decision(true, "allow", "user_executable", subject, "");
        }
        if choice == "4" {
            let mut delta = UserGrants::default();
            if subject.kind == "shell" {
                push_unique_opt(&mut delta.shell_commands, subject.shell_command.clone());
            } else if subject.kind == "file_path" {
                push_unique_opt(&mut delta.file_roots, subject.file_root.clone());
            } else {
                push_unique(&mut delta.tool_names, subject.tool_name.clone());
            }
            if self.user_store.merge(&delta).is_err() {
                return decision(false, "deny", "none", subject, "persist_error");
            }
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
    let mut policy = PermissionPolicy::from_config(config, context.cwd.clone());
    policy.decide(&tool, arguments, context, None)
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

fn subject_shell_executables(subject: &PermissionSubject) -> Vec<String> {
    let mut executables = subject
        .shell_command
        .as_deref()
        .map(crate::shell_policy::shell_executables)
        .unwrap_or_default();
    if executables.is_empty() {
        if let Some(executable) = subject.shell_executable.clone() {
            executables.push(executable);
        }
    }
    sorted_dedup(executables)
}

fn shell_command_matches_executables(
    command: Option<&str>,
    commands: &[String],
    executables: &[String],
) -> bool {
    let Some(command) = command else {
        return false;
    };
    if executables.is_empty() {
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
        commands.contains(segment)
            || executables
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

fn union_strings(left: &[String], right: &[String]) -> Vec<String> {
    sorted_dedup(left.iter().chain(right).cloned().collect())
}

fn format_grants(grants: &UserGrants) -> String {
    let mut shell_commands = grants.shell_commands.clone();
    let mut shell_executables = grants.shell_executables.clone();
    let mut tool_names = grants.tool_names.clone();
    let mut file_roots = grants.file_roots.clone();
    format!(
        "[shell]\ncommands = [{}]\nexecutables = [{}]\n\n[tools]\nnames = [{}]\n\n[files]\nroots = [{}]\n",
        toml_array(&mut shell_commands),
        toml_array(&mut shell_executables),
        toml_array(&mut tool_names),
        toml_array(&mut file_roots)
    )
}

fn normalize_grants(grants: &UserGrants) -> UserGrants {
    UserGrants {
        shell_commands: sorted_dedup(grants.shell_commands.clone()),
        shell_executables: sorted_dedup(grants.shell_executables.clone()),
        tool_names: sorted_dedup(grants.tool_names.clone()),
        file_roots: sorted_dedup(grants.file_roots.clone()),
    }
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
            if let Some(target) = argv.get(index + 1) {
                if !is_non_file_redirection_target(target) {
                    return Some(target.clone());
                }
            }
        }
        for op in redirect_ops {
            if token.starts_with(op) && token.len() > op.len() {
                let target = &token[op.len()..];
                if !is_non_file_redirection_target(target) {
                    return Some(target.to_string());
                }
            }
        }
    }
    if argv.first().is_some_and(|item| item == "tee") {
        return argv
            .iter()
            .skip(1)
            .find(|token| {
                !token.starts_with('-') && !is_non_file_redirection_target(token.as_str())
            })
            .cloned();
    }
    None
}

fn is_non_file_redirection_target(target: &str) -> bool {
    if target == "/dev/null" {
        return true;
    }
    let Some(descriptor) = target.strip_prefix('&') else {
        return false;
    };
    descriptor == "-"
        || (!descriptor.is_empty() && descriptor.chars().all(|ch| ch.is_ascii_digit()))
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
