use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::AgentConfig;
use crate::http::request_json;
use crate::memory::truncate;
use crate::messages::{MediaPart, ToolResult};
use crate::skills::run_skill_command;

#[derive(Clone)]
pub struct ToolContext {
    pub config: Arc<AgentConfig>,
    pub cwd: PathBuf,
    pub allowed_file_roots: Vec<PathBuf>,
    pub media_sender: Option<Arc<dyn Fn(MediaPart) -> Result<(), String> + Send + Sync>>,
    pub image_analyzer: Option<Arc<dyn Fn(&Path, &str) -> Result<String, String> + Send + Sync>>,
}

#[derive(Clone, Debug)]
pub struct ToolInfo {
    pub name: String,
    pub read_only: bool,
}

impl ToolInfo {
    pub fn new(name: &str, read_only: bool) -> Self {
        Self {
            name: name.to_string(),
            read_only,
        }
    }
}

pub fn tool_info(name: &str) -> ToolInfo {
    ToolInfo::new(
        name,
        matches!(
            name,
            "files.list"
                | "files.read"
                | "memory.list"
                | "memory.read"
                | "memory.search"
                | "image.understand"
        ),
    )
}

impl ToolContext {
    pub fn new(config: impl Into<Arc<AgentConfig>>, cwd: PathBuf) -> Self {
        Self {
            config: config.into(),
            cwd,
            allowed_file_roots: Vec::new(),
            media_sender: None,
            image_analyzer: None,
        }
    }

    pub fn with_allowed_file_root(&self, root: PathBuf) -> Self {
        let mut next = self.clone();
        next.allowed_file_roots = vec![root];
        next
    }

    pub fn with_media_sender(
        mut self,
        sender: Arc<dyn Fn(MediaPart) -> Result<(), String> + Send + Sync>,
    ) -> Self {
        self.media_sender = Some(sender);
        self
    }

    pub fn with_image_analyzer(
        mut self,
        analyzer: Arc<dyn Fn(&Path, &str) -> Result<String, String> + Send + Sync>,
    ) -> Self {
        self.image_analyzer = Some(analyzer);
        self
    }
}

pub fn tool_specs() -> Vec<serde_json::Value> {
    tool_specs_for_enabled(&[
        "files".to_string(),
        "shell".to_string(),
        "memory".to_string(),
        "skills".to_string(),
        "web".to_string(),
        "image".to_string(),
    ])
}

pub fn tool_specs_for_config(config: &AgentConfig) -> Vec<serde_json::Value> {
    tool_specs_for_enabled(&config.tools.enabled)
}

fn tool_specs_for_enabled(enabled: &[String]) -> Vec<serde_json::Value> {
    let has = |name: &str| enabled.iter().any(|item| item == name);
    let mut specs = Vec::new();
    if has("files") {
        specs.extend(files_tool_specs());
    }
    if has("memory") {
        specs.extend(memory_tool_specs());
    }
    if has("shell") {
        specs.push(openai_tool(
            "shell.run",
            "Run a shell command after Colibri permission approval. Do not use this to create or edit files; use files.write for generated artifacts and text file changes.",
            serde_json::json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}),
        ));
    }
    if has("web") {
        specs.push(openai_tool(
            "web.search",
            "Search the web using the configured search engine.",
            serde_json::json!({
                "type":"object",
                "properties":{
                    "query":{"type":"string","description":"Search query."},
                    "count":{"type":"integer","description":"Maximum number of web results. Defaults to config.web_search.max_results."},
                    "freshness":{"type":"string","description":"Optional freshness filter: pd, pw, pm, py, or YYYY-MM-DDtoYYYY-MM-DD."}
                },
                "required":["query"],
                "additionalProperties":false
            }),
        ));
    }
    if has("image") {
        specs.push(openai_tool(
            "image.understand",
            "Understand a local image with the configured vision model. Use the optional prompt to ask what should be inspected or extracted.",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"prompt":{"type":"string"}},"required":["path"]}),
        ));
    }
    if has("skills") {
        specs.push(openai_tool(
            "skill.run",
            "Run a configured local skill command.",
            serde_json::json!({"type":"object","properties":{"skill":{"type":"string"},"command":{"type":"string"}},"required":["skill","command"]}),
        ));
    }
    specs
}

fn files_tool_specs() -> Vec<serde_json::Value> {
    vec![
        openai_tool(
            "files.list",
            "List direct children of an allowed directory.",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        ),
        openai_tool(
            "files.read",
            "Read a UTF-8 text file under an allowed root.",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        ),
        openai_tool(
            "files.write",
            "Write a UTF-8 text file under an allowed root. Use this for generated artifacts and file edits; do not use shell redirection or heredocs to create files.",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}),
        ),
        openai_tool(
            "files.send",
            "Send a local file to the current chat channel. This can expose host files outside Colibri, so use it only when the user asked to send a file.",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"caption":{"type":"string"}},"required":["path"]}),
        ),
    ]
}

fn memory_tool_specs() -> Vec<serde_json::Value> {
    vec![
        openai_tool("memory.list", "List available memory files.", serde_json::json!({"type":"object","properties":{}})),
        openai_tool(
            "memory.read",
            "Read MEMORY.md, USER.md, INDEX.md, or a topic memory file.",
            serde_json::json!({"type":"object","properties":{"file":{"type":"string"},"topic":{"type":"string"}}}),
        ),
        openai_tool(
            "memory.search",
            "Search INDEX.md memory manifest lines by keyword.",
            serde_json::json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}),
        ),
        openai_tool(
            "memory.write",
            "Append to or replace a memory file. Memory files must use frontmatter:\n---\ntype: user|feedback|project|reference|system\ndescription: one-line description\nupdated: YYYY-MM-DD\n---\nChoose USER.md for user profile, preferences, and collaboration style; keep it under 600 characters. Choose MEMORY.md for short stable general, project, or system facts; keep it under 1800 characters. Choose INDEX.md for the searchable topic manifest used by memory.search. Choose topics/<name>.md for detailed topic notes. When creating or materially changing a topic file, also update INDEX.md with a searchable one-line pointer. Consolidate or replace USER.md and MEMORY.md instead of appending forever.",
            serde_json::json!({"type":"object","properties":{"file":{"type":"string"},"topic":{"type":"string"},"content":{"type":"string"},"mode":{"type":"string","enum":["append","replace"]}},"required":["content"]}),
        ),
    ]
}

fn openai_tool(name: &str, description: &str, parameters: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters
        }
    })
}

pub fn run_tool(
    name: &str,
    arguments_json: &str,
    context: &ToolContext,
) -> Result<ToolResult, String> {
    let values = parse_json_object(arguments_json)?;
    let args = string_arguments(&values);
    run_tool_map(name, &args, context)
}

pub fn run_tool_map(
    name: &str,
    args: &BTreeMap<String, String>,
    context: &ToolContext,
) -> Result<ToolResult, String> {
    let result = match name {
        "files.list" => files_list(args, context),
        "files.read" => files_read(args, context),
        "files.write" => files_write(args, context),
        "files.send" => files_send(args, context),
        "shell.run" => shell_run(args, context),
        "memory.list" => memory_list(context),
        "memory.read" => memory_read(args, context),
        "memory.search" => memory_search(args, context),
        "memory.write" => memory_write(args, context),
        "skill.run" => run_skill_command(args, context),
        "web.search" => web_search(args, context),
        "image.understand" => image_understand(args, context),
        _ => ToolResult::error("unknown_tool", format!("Unknown tool: {}", name)),
    };
    Ok(result)
}

fn files_list(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let path = resolve_arg_path(args.get("path").map(String::as_str).unwrap_or("."), context);
    if !is_allowed(&path, context) {
        return ToolResult::error(
            "permission_denied",
            format!("Path is outside allowed roots: {}", path.display()),
        );
    }
    let entries = match fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(error) => return ToolResult::error("io_error", error.to_string()),
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let mut name = entry.file_name().to_string_lossy().to_string();
        if entry.path().is_dir() {
            name.push('/');
        }
        names.push(name);
    }
    names.sort();
    ToolResult::ok(names.join("\n"))
}

fn files_read(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some(raw_path) = args.get("path").filter(|value| !value.is_empty()) else {
        return ToolResult::error("invalid_arguments", "Missing path");
    };
    let path = resolve_arg_path(raw_path, context);
    if !is_allowed(&path, context) {
        return ToolResult::error(
            "permission_denied",
            format!("Path is outside allowed roots: {}", path.display()),
        );
    }
    if !path.exists() {
        return ToolResult::error("not_found", "Path does not exist");
    }
    if !path.is_file() {
        return ToolResult::error("not_file", "Path is not a file");
    }
    match fs::read_to_string(&path) {
        Ok(text) => {
            let (text, truncated) = truncate(text, context.config.tools.max_result_chars);
            let mut result = ToolResult::ok(text);
            result.truncated = truncated;
            result
        }
        Err(error) => ToolResult::error("io_error", error.to_string()),
    }
}

fn files_write(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some(raw_path) = args.get("path").filter(|value| !value.is_empty()) else {
        return ToolResult::error("invalid_arguments", "Missing path");
    };
    let path = resolve_arg_path(raw_path, context);
    if !is_allowed(&path, context) {
        return ToolResult::error(
            "permission_denied",
            format!("Path is outside allowed roots: {}", path.display()),
        );
    }
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            return ToolResult::error("io_error", error.to_string());
        }
    }
    let content = args.get("content").cloned().unwrap_or_default();
    match fs::write(&path, &content) {
        Ok(()) => ToolResult::ok(format!(
            "Wrote {} bytes to {}",
            content.as_bytes().len(),
            path.display()
        )),
        Err(error) => ToolResult::error("io_error", error.to_string()),
    }
}

fn files_send(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    if context.media_sender.is_none() {
        return ToolResult::error(
            "media_unavailable",
            "No active channel can send files in this session",
        );
    }
    let Some(raw_path) = args.get("path").filter(|value| !value.is_empty()) else {
        return ToolResult::error("invalid_arguments", "Missing path");
    };
    let path = resolve_arg_path(raw_path, context);
    if !is_allowed(&path, context) {
        return ToolResult::error("permission_denied", "Path is outside allowed roots");
    }
    if !path.exists() {
        return ToolResult::error("not_found", "Path does not exist");
    }
    if !path.is_file() {
        return ToolResult::error("not_file", "Path is not a file");
    }
    let content_type = content_type_for_path(&path);
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let media = MediaPart::new(
        media_type_for_content(&content_type),
        path,
        filename.clone(),
        content_type,
        args.get("caption").cloned().unwrap_or_default(),
    );
    ToolResult::ok(format!("Sent file to channel: {}", filename)).with_media(media)
}

fn shell_run(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let command = args.get("command").cloned().unwrap_or_default();
    if command.trim().is_empty() {
        return ToolResult::error("invalid_arguments", "Missing command");
    }
    let argv = match shell_words::split(&command) {
        Ok(argv) if !argv.is_empty() => argv,
        Ok(_) => return ToolResult::error("invalid_arguments", "Missing command"),
        Err(error) => return ToolResult::error("invalid_arguments", error.to_string()),
    };
    let executable = &argv[0];
    if context
        .config
        .shell
        .deny
        .iter()
        .any(|denied| denied == executable)
    {
        return ToolResult::error("permission_denied", "Command is denied");
    }
    let mut command = Command::new(executable);
    command
        .args(&argv[1..])
        .current_dir(&context.cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match run_command_with_timeout(command, context.config.tools.max_shell_seconds) {
        Err(CommandRunError::Timeout) => ToolResult::error("timeout", "Command timed out"),
        Err(CommandRunError::Io(error)) => ToolResult::error("execution_error", error),
        Ok(output) => {
            let mut text = String::new();
            text.push_str(&String::from_utf8_lossy(&output.stdout));
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            let (text, truncated) = truncate(text, context.config.tools.max_result_chars);
            let mut result = if output.status.success() {
                ToolResult::ok(text)
            } else {
                ToolResult::error("nonzero_exit", text)
            };
            result.truncated = truncated;
            result
        }
    }
}

enum CommandRunError {
    Timeout,
    Io(String),
}

fn run_command_with_timeout(
    mut command: Command,
    timeout_seconds: f64,
) -> Result<std::process::Output, CommandRunError> {
    let mut child = command
        .spawn()
        .map_err(|error| CommandRunError::Io(error.to_string()))?;
    let stdout_reader = child.stdout.take().map(|mut handle| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            handle.read_to_end(&mut bytes).map(|_| bytes)
        })
    });
    let stderr_reader = child.stderr.take().map(|mut handle| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            handle.read_to_end(&mut bytes).map(|_| bytes)
        })
    });
    let started = Instant::now();
    let timeout = Duration::from_secs_f64(timeout_seconds.max(0.0));
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                join_output_reader(stdout_reader)?;
                join_output_reader(stderr_reader)?;
                return Err(CommandRunError::Timeout);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(error) => return Err(CommandRunError::Io(error.to_string())),
        }
    }
    let status = child
        .wait()
        .map_err(|error| CommandRunError::Io(error.to_string()))?;
    let stdout = join_output_reader(stdout_reader)?;
    let stderr = join_output_reader(stderr_reader)?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

fn join_output_reader(
    reader: Option<std::thread::JoinHandle<std::io::Result<Vec<u8>>>>,
) -> Result<Vec<u8>, CommandRunError> {
    let Some(reader) = reader else {
        return Ok(Vec::new());
    };
    reader
        .join()
        .map_err(|_| CommandRunError::Io("command output reader panicked".to_string()))?
        .map_err(|error| CommandRunError::Io(error.to_string()))
}

fn memory_list(context: &ToolContext) -> ToolResult {
    let mut names = Vec::new();
    for name in ["MEMORY.md", "USER.md", "INDEX.md"] {
        if context.config.memory.root.join(name).is_file() {
            names.push(name.to_string());
        }
    }
    let topics = context.config.memory.root.join("topics");
    if let Ok(entries) = fs::read_dir(topics) {
        for entry in entries.flatten() {
            if entry.path().is_file()
                && entry.path().extension().and_then(|value| value.to_str()) == Some("md")
            {
                names.push(format!("topics/{}", entry.file_name().to_string_lossy()));
            }
        }
    }
    names.sort();
    let (text, truncated) = truncate(names.join("\n"), context.config.tools.max_result_chars);
    let mut result = ToolResult::ok(text);
    result.truncated = truncated;
    result
}

fn memory_read(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some((path, _)) = memory_target(args, context) else {
        return ToolResult::error("invalid_arguments", "Invalid memory file");
    };
    if !path.exists() {
        return ToolResult::error("not_found", "Memory file does not exist");
    }
    if !path.is_file() {
        return ToolResult::error("not_file", "Memory path is not a file");
    }
    match fs::read_to_string(path) {
        Ok(text) => {
            let (text, truncated) = truncate(text, context.config.tools.max_result_chars);
            let mut result = ToolResult::ok(text);
            result.truncated = truncated;
            result
        }
        Err(error) => ToolResult::error("io_error", error.to_string()),
    }
}

fn memory_search(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some(query) = args
        .get("query")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return ToolResult::error("invalid_arguments", "Missing query");
    };
    let query = query.to_lowercase();
    let index = context.config.memory.root.join("INDEX.md");
    let text = fs::read_to_string(index).unwrap_or_default();
    let mut matches = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.to_lowercase().contains(&query) {
            matches.push(format!("INDEX.md:{}: {}", index + 1, line));
        }
        if matches.len() >= context.config.memory.max_search_results {
            break;
        }
    }
    let (text, truncated) = truncate(matches.join("\n"), context.config.tools.max_result_chars);
    let mut result = ToolResult::ok(text);
    result.truncated = truncated;
    result
}

fn memory_write(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some((path, label)) = memory_target(args, context) else {
        return ToolResult::error("invalid_arguments", "Invalid memory file");
    };
    let Some(content) = args
        .get("content")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return ToolResult::error("invalid_arguments", "Missing content");
    };
    let mode = args.get("mode").map(String::as_str).unwrap_or("append");
    if !matches!(mode, "append" | "replace") {
        return ToolResult::error("invalid_arguments", "Invalid write mode");
    }
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            return ToolResult::error("io_error", error.to_string());
        }
    }
    let write_result = if mode == "replace" {
        fs::write(
            &path,
            format!(
                "{}{}",
                content,
                if content.ends_with('\n') { "" } else { "\n" }
            ),
        )
    } else {
        use std::io::Write;
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                file.write_all(content.as_bytes())?;
                if !content.ends_with('\n') {
                    file.write_all(b"\n")?;
                }
                Ok(())
            })
    };
    match write_result {
        Ok(()) => {
            let mut message = format!("Updated memory file: {}", label);
            if label.starts_with("topics/") {
                message.push_str(
                    "\nRemember to update INDEX.md so this topic can be found by memory.search.",
                );
            }
            let limit = match label.as_str() {
                "MEMORY.md" => Some(1800usize),
                "USER.md" => Some(600usize),
                _ => None,
            };
            if let Some(limit) = limit {
                let size = fs::read_to_string(&path)
                    .map(|text| text.chars().count())
                    .unwrap_or(0);
                if size > limit {
                    message.push_str(&format!(
                        "\n{} exceeds {} characters. Summarize or consolidate it, then call memory.write with file=\"{}\", mode=\"replace\" to keep it within the limit.",
                        label, limit, label
                    ));
                }
            }
            let (text, truncated) = truncate(message, context.config.tools.max_result_chars);
            let mut result = ToolResult::ok(text);
            result.truncated = truncated;
            result
        }
        Err(error) => ToolResult::error("io_error", error.to_string()),
    }
}

fn memory_target(
    args: &BTreeMap<String, String>,
    context: &ToolContext,
) -> Option<(PathBuf, String)> {
    if let Some(topic) = args.get("topic").map(|value| value.trim()) {
        if valid_topic_name(topic) {
            let label = format!("topics/{}.md", topic);
            return Some((context.config.memory.root.join(&label), label));
        }
    }
    let file = args.get("file")?.trim();
    if matches!(file, "MEMORY.md" | "USER.md" | "INDEX.md") {
        return Some((context.config.memory.root.join(file), file.to_string()));
    }
    let topic = file.strip_prefix("topics/")?.strip_suffix(".md")?;
    valid_topic_name(topic).then(|| {
        let label = format!("topics/{}.md", topic);
        (context.config.memory.root.join(&label), label)
    })
}

fn valid_topic_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn web_search(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    if context.config.web_search.engine != "baidu" {
        return ToolResult::error(
            "invalid_config",
            format!(
                "Unsupported web search engine: {}",
                context.config.web_search.engine
            ),
        );
    }
    let query = args.get("query").cloned().unwrap_or_default();
    if query.trim().is_empty() {
        return ToolResult::error(
            "invalid_arguments",
            "web.search requires a non-empty string query",
        );
    }
    let count = match args.get("count") {
        None => context.config.web_search.max_results,
        Some(value) => match value.parse::<isize>() {
            Ok(value) => value.clamp(1, 50) as usize,
            Err(_) => {
                return ToolResult::error(
                    "invalid_arguments",
                    "web.search count must be an integer",
                )
            }
        },
    };
    let search_filter = match web_search_filter(args.get("freshness").map(String::as_str)) {
        Ok(value) => value,
        Err(error) => return ToolResult::error("invalid_arguments", error),
    };
    let body = serde_json::to_string(&serde_json::json!({
        "messages":[{"content":query.trim(),"role":"user"}],
        "search_source":"baidu_search_v2",
        "resource_type_filter":[{"type":"web","top_k":count}],
        "search_filter":search_filter
    }))
    .unwrap_or_else(|_| "{}".to_string());
    let (url, headers) = match baidu_url_and_headers(context) {
        Ok(value) => value,
        Err(error) => return ToolResult::error("invalid_config", error),
    };
    let response = match request_json(
        "POST",
        &url,
        &headers
            .iter()
            .map(|(key, value)| (key.as_str(), value.clone()))
            .collect::<Vec<_>>(),
        Some(&body),
        context.config.web_search.timeout_seconds,
    ) {
        Ok(response) => response,
        Err(error) if error.contains("(28)") || error.to_lowercase().contains("timed out") => {
            return ToolResult::error("timeout", "web.search request timed out")
        }
        Err(error) => {
            return ToolResult::error(
                "network_error",
                format!("web.search request failed: {}", error),
            )
        }
    };
    if let Some(status) = response.status {
        if status >= 400 {
            return ToolResult::error(
                "http_error",
                format!("web.search failed with HTTP {}: {}", status, response.body),
            );
        }
    }
    let text = match format_baidu_references(&response.body) {
        Ok(text) => text,
        Err((kind, text)) => return ToolResult::error(kind, text),
    };
    let (text, truncated) = truncate(text, context.config.tools.max_result_chars);
    let mut result = ToolResult::ok(text);
    result.truncated = truncated;
    result
}

fn image_understand(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some(raw_path) = args.get("path").filter(|value| !value.is_empty()) else {
        return ToolResult::error("invalid_arguments", "Missing path");
    };
    let Some(analyzer) = &context.image_analyzer else {
        return ToolResult::error("vision_unavailable", "Image understanding is unavailable");
    };
    let path = resolve_arg_path(raw_path, context);
    if !is_allowed(&path, context) {
        return ToolResult::error("permission_denied", "Path is outside allowed roots");
    }
    if !path.exists() {
        return ToolResult::error("not_found", "Path does not exist");
    }
    if !path.is_file() {
        return ToolResult::error("not_file", "Path is not a file");
    }
    let content_type = content_type_for_path(&path);
    if !content_type.starts_with("image/") {
        return ToolResult::error("invalid_media", "Path is not an image");
    }
    let prompt = args.get("prompt").map(String::as_str).unwrap_or("");
    match analyzer(&path, prompt) {
        Ok(text) => {
            let (text, truncated) = truncate(text, context.config.tools.max_result_chars);
            let mut result = ToolResult::ok(text);
            result.truncated = truncated;
            result
        }
        Err(error) if error.starts_with("image_too_large:") => ToolResult::error(
            "image_too_large",
            error.trim_start_matches("image_too_large:"),
        ),
        Err(error) => ToolResult::error("model_error", error),
    }
}

fn web_search_filter(freshness: Option<&str>) -> Result<serde_json::Value, String> {
    match freshness {
        None | Some("") => Ok(serde_json::json!({})),
        Some(value) => {
            let today = local_date();
            let end = add_date_days(&today, 1).unwrap_or_else(|| today.clone());
            let start = match value {
                "pd" => add_date_days(&today, -1),
                "pw" => add_date_days(&today, -6),
                "pm" => add_date_days(&today, -30),
                "py" => add_date_days(&today, -364),
                _ if valid_date_range(value) => {
                    let (start, end) = value.split_once("to").unwrap();
                    return Ok(serde_json::json!({"range":{"page_time":{"gte":start,"lt":end}}}));
                }
                _ => None,
            }
            .ok_or_else(|| {
                "web.search freshness must be pd, pw, pm, py, or YYYY-MM-DDtoYYYY-MM-DD".to_string()
            })?;
            Ok(serde_json::json!({"range":{"page_time":{"gte":start,"lt":end}}}))
        }
    }
}

fn format_baidu_references(body: &str) -> Result<String, (&'static str, String)> {
    let data: serde_json::Value = serde_json::from_str(body).map_err(|_| {
        (
            "invalid_response",
            "web.search response was not valid JSON".to_string(),
        )
    })?;
    if data.get("code").is_some() {
        let message = data
            .get("message")
            .or_else(|| data.get("msg"))
            .and_then(|value| value.as_str())
            .unwrap_or("Baidu web search API error");
        return Err(("api_error", message.to_string()));
    }
    let references = data
        .get("references")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            (
                "invalid_response",
                "Baidu web search response missing references".to_string(),
            )
        })?;
    let cleaned = references
        .iter()
        .map(|item| {
            let mut item = item.clone();
            if let Some(object) = item.as_object_mut() {
                object.remove("snippet");
            }
            item
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&cleaned).map_err(|error| ("invalid_response", error.to_string()))
}

fn baidu_url_and_headers(context: &ToolContext) -> Result<(String, Vec<(String, String)>), String> {
    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    if let (Ok(session_id), Ok(scheduler_url)) = (
        std::env::var("DUMATE_SESSION_ID"),
        std::env::var("DUMATE_SCHEDULER_URL"),
    ) {
        let endpoint = &context.config.web_search.endpoint;
        let without_scheme = endpoint
            .split_once("://")
            .map(|(_, value)| value)
            .unwrap_or(endpoint);
        let (host, path) = without_scheme
            .split_once('/')
            .unwrap_or((without_scheme, ""));
        headers.push(("Host".to_string(), host.to_string()));
        headers.push(("X-Dumate-Session-Id".to_string(), session_id));
        headers.push(("X-Appbuilder-From".to_string(), "desktop".to_string()));
        return Ok((
            format!(
                "{}/api/qianfanproxy/{}",
                scheduler_url.trim_end_matches('/'),
                path
            ),
            headers,
        ));
    }
    if context.config.web_search.api_key.is_empty() {
        return Err("Missing Baidu web search API key: set web_search.api_key".to_string());
    }
    headers.push((
        "Authorization".to_string(),
        format!("Bearer {}", context.config.web_search.api_key),
    ));
    headers.push(("X-Appbuilder-From".to_string(), "openclaw".to_string()));
    Ok((context.config.web_search.endpoint.clone(), headers))
}

fn valid_date_range(value: &str) -> bool {
    let Some((start, end)) = value.split_once("to") else {
        return false;
    };
    valid_date(start) && valid_date(end) && value.matches("to").count() == 1
}

fn valid_date(value: &str) -> bool {
    value.len() == 10
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value
            .chars()
            .enumerate()
            .all(|(index, ch)| matches!(index, 4 | 7) || ch.is_ascii_digit())
}

fn local_date() -> String {
    Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| valid_date(value))
        .unwrap_or_else(|| "1970-01-01".to_string())
}

fn add_date_days(value: &str, offset: i64) -> Option<String> {
    let year = value.get(0..4)?.parse::<i64>().ok()?;
    let month = value.get(5..7)?.parse::<i64>().ok()?;
    let day = value.get(8..10)?.parse::<i64>().ok()?;
    let days = days_from_civil(year, month, day) + offset;
    let (year, month, day) = civil_from_days(days);
    Some(format!("{:04}-{:02}-{:02}", year, month, day))
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * shifted_month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719468;
    let era = days.div_euclid(146097);
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn resolve_arg_path(value: &str, context: &ToolContext) -> PathBuf {
    let path = expand_user_path(value);
    let joined = if path.is_absolute() {
        path
    } else {
        context.cwd.join(path)
    };
    canonicalize_existing_prefix(&joined)
}

fn expand_user_path(value: &str) -> PathBuf {
    if value == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(value)
}

pub fn is_allowed(path: &Path, context: &ToolContext) -> bool {
    let canonical = canonicalize_existing_prefix(path);
    let mut roots = context.config.files.roots.clone();
    roots.push(context.cwd.clone());
    roots.extend(context.allowed_file_roots.clone());
    roots.into_iter().any(|root| {
        let root = canonicalize_existing_prefix(&root);
        canonical.starts_with(root)
    })
}

pub fn content_type_for_path(path: &Path) -> String {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "txt" | "md" | "csv" | "log" => "text/plain".to_string(),
        "html" | "htm" => "text/html".to_string(),
        "json" => "application/json".to_string(),
        "png" => "image/png".to_string(),
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "gif" => "image/gif".to_string(),
        "webp" => "image/webp".to_string(),
        "mp4" => "video/mp4".to_string(),
        "mov" => "video/quicktime".to_string(),
        "mp3" => "audio/mpeg".to_string(),
        "wav" => "audio/wav".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

fn media_type_for_content(content_type: &str) -> String {
    if content_type.starts_with("image/") {
        "image".to_string()
    } else if content_type.starts_with("video/") {
        "video".to_string()
    } else if content_type.starts_with("audio/") {
        "audio".to_string()
    } else {
        "file".to_string()
    }
}

fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    let mut missing = Vec::new();
    let mut current = path;
    while !current.exists() {
        if let Some(name) = current.file_name() {
            missing.push(name.to_os_string());
        }
        let Some(parent) = current.parent() else {
            return path.to_path_buf();
        };
        current = parent;
    }
    let mut rebuilt = current
        .canonicalize()
        .unwrap_or_else(|_| current.to_path_buf());
    for item in missing.iter().rev() {
        rebuilt.push(item);
    }
    rebuilt
}

pub fn parse_json_object(text: &str) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("expected JSON object: {}", error))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "expected JSON object".to_string())
}

pub fn string_arguments(
    values: &serde_json::Map<String, serde_json::Value>,
) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| value.to_string()),
            )
        })
        .collect()
}
