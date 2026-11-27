use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::{expand_user_path, AgentConfig};
use crate::hardware_tools::{
    hardware_controller_tool, hardware_devices, hardware_probe, hardware_serial_read,
    hardware_serial_write, is_hardware_tool,
};
use crate::memory::truncate;
use crate::messages::{MediaPart, ToolResult};
use crate::process_output::{run_tool_command, CommandRunError};
use crate::skills::run_skill_command;

static TOOL_SPECS_CACHE: Mutex<Option<(Vec<String>, Vec<serde_json::Value>)>> = Mutex::new(None);
#[derive(Clone)]
pub struct ToolContext {
    pub config: Arc<AgentConfig>,
    pub cwd: PathBuf,
    pub runtime_home: PathBuf,
    pub scope: String,
    pub execution_host: Option<PathBuf>,
    pub media_sender: Option<Arc<dyn Fn(MediaPart) -> Result<(), String> + Send + Sync>>,
    pub image_analyzer: Option<Arc<dyn Fn(&Path, &str) -> Result<String, String> + Send + Sync>>,
}

impl ToolContext {
    pub fn new(config: impl Into<Arc<AgentConfig>>, cwd: PathBuf) -> Self {
        Self {
            config: config.into(),
            cwd,
            runtime_home: crate::config::colibri_home(),
            scope: "local".into(),
            execution_host: None,
            media_sender: None,
            image_analyzer: None,
        }
    }

    pub fn with_execution_host(mut self, host: PathBuf) -> Self {
        self.execution_host = Some(host);
        self
    }
    pub fn with_runtime_home(mut self, home: PathBuf) -> Self {
        self.runtime_home = home;
        self
    }
    pub fn with_scope(mut self, scope: String) -> Self {
        self.scope = scope;
        self
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
    let mut enabled = config.tools.enabled.clone();
    if !config.hardware.enabled {
        enabled.retain(|category| category != "hardware");
    }
    tool_specs_for_enabled(&enabled)
}

fn tool_specs_for_enabled(enabled: &[String]) -> Vec<serde_json::Value> {
    if let Ok(cache) = TOOL_SPECS_CACHE.lock() {
        if let Some((key, specs)) = cache.as_ref() {
            if key.as_slice() == enabled {
                return specs.clone();
            }
        }
    }
    let specs = build_tool_specs(enabled);
    if let Ok(mut cache) = TOOL_SPECS_CACHE.lock() {
        *cache = Some((enabled.to_vec(), specs.clone()));
    }
    specs
}

fn build_tool_specs(enabled: &[String]) -> Vec<serde_json::Value> {
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
            "Run a non-interactive shell command as the current user. For builds/downloads/long commands set background=true, then shell.poll until terminal status. Starting a job is not completion. Set timeout_seconds to cover expected work; cwd optionally overrides the current directory. Combine already-known related checks in one command; do not repeat successful reads. Large foreground output is a bounded preview with redacted archive paths; use files.read ranges/offset_chars or files.search instead of rerunning the command.",
            serde_json::json!({"type":"object","properties":{"command":{"type":"string"},"background":{"type":"boolean"},"cwd":{"type":"string"},"timeout_seconds":{"type":"number","minimum":0.001,"maximum":86400}},"required":["command"]}),
        ));
    }
    if has("hardware") {
        specs.extend(hardware_tool_specs());
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
            "skill.read",
            "Read the full SKILL.md instructions for a skill listed in the catalog. Prefer this over guessing skill contents.",
            serde_json::json!({"type":"object","properties":{"name":{"type":"string","description":"Exact skill name from the catalog."}},"required":["name"]}),
        ));
        specs.push(openai_tool(
            "skill.run",
            "Run a command declared in a skill's SKILL.md YAML frontmatter. Prefer this tool whenever a configured command matches the requested action; do not invoke the underlying executable through shell.run.",
            serde_json::json!({"type":"object","properties":{"skill":{"type":"string"},"command":{"type":"string"},"args":{"type":"array","items":{"type":"string"}}},"required":["skill","command"]}),
        ));
    }
    #[cfg(unix)]
    {
        if has("shell") {
            specs.push(openai_tool("shell.poll", "Read incremental background output and status. Continue from next_offset while page_has_more. truncated means capture loss, not merely another page. Use tail=true if capture was truncated; output_path references only the indicated head/tail log, not a complete missing middle. Completed with exit_code=0 is success; running is not success.", serde_json::json!({"type":"object","properties":{"job_id":{"type":"string"},"offset":{"type":"integer","minimum":0},"wait_ms":{"type":"integer","minimum":0,"maximum":5000},"tail":{"type":"boolean"}},"required":["job_id"]})));
            specs.push(openai_tool("shell.cancel", "Request cancellation of a background job; poll afterwards to confirm settlement.", serde_json::json!({"type":"object","properties":{"job_id":{"type":"string"}},"required":["job_id"]})));
        }
        if has("shell") || has("files") {
            specs.push(openai_tool("task.start", "Record a multi-step execution task before substantial work. Keep simple questions direct. Existing tasks require replace=true only when the user changed the objective. Record concrete externally checkable acceptance criteria.", serde_json::json!({"type":"object","properties":{"objective":{"type":"string"},"steps":{"type":"array","items":{"type":"string"}},"acceptance":{"type":"array","items":{"type":"string"}},"replace":{"type":"boolean"}},"required":["objective","steps","acceptance"]})));
            specs.push(openai_tool("task.update", "Update durable progress. Step numbers are 1-based; completed steps need evidence_ids returned by actual successful tools, including relevant checks. Set a concrete blocker when work cannot continue; clear blocker with an empty string to resume. All completed steps settle the task.", serde_json::json!({"type":"object","properties":{"step":{"type":"integer","minimum":1},"status":{"type":"string","enum":["pending","running","completed"]},"evidence_ids":{"type":"array","items":{"type":"string"}},"next_action":{"type":"string"},"blocker":{"type":"string"}}})));
            specs.push(openai_tool("task.inspect", "Read the current task, acceptance checks, steps and actual evidence references after interruption or compaction.", serde_json::json!({"type":"object","properties":{}})));
        }
    }
    specs
}

fn files_tool_specs() -> Vec<serde_json::Value> {
    let mut specs = vec![
        openai_tool("files.search", "Bounded literal text search with file paths and line numbers. Returns match-centered snippets and explicit stop_reason. Continue with continuation.after_path/start_line; when no continuation is available, narrow the scope or use streaming shell search.", serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"query":{"type":"string"},"after_path":{"type":"string"},"start_line":{"type":"integer","minimum":1},"max_results":{"type":"integer","minimum":1,"maximum":100}},"required":["query"]})),
        #[cfg(unix)]
        openai_tool("files.patch", "Replace exactly one unique old_text occurrence atomically, preserving permissions. Include enough unique context; optionally pass expected_sha256 to detect a stale read. Files above 1 MiB require streaming system tools.", serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"},"expected_sha256":{"type":"string"}},"required":["path","old_text","new_text"]})),
        openai_tool(
            "files.list",
            "List a sorted bounded page of direct children. Continue with after=next_after; limit defaults to 50, maximum 200. Directory changes can affect pagination.",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":200},"after":{"type":"string"}},"required":["path"]}),
        ),
        openai_tool(
            "files.read",
            "Read a UTF-8 text file. Prefer start_line/end_line ranges for large files. offset_chars (0-based, mutually exclusive with line ranges) pages any UTF-8 file, including long single lines; offsets refer to redacted text. Continue truncated reads using next_offset_chars. Optional max_chars caps this read result and is itself capped by tools.max_result_chars.",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"start_line":{"type":"integer","minimum":1},"end_line":{"type":"integer","minimum":1},"max_chars":{"type":"integer","minimum":1},"offset_chars":{"type":"integer","minimum":0}},"required":["path"]}),
        ),
        openai_tool(
            "files.write",
            "Write a UTF-8 text file.",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}),
        ),
        openai_tool(
            "files.send",
            "Send a local file to the current chat channel. This can expose host files outside Colibri, so use it only when the user asked to send a file.",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"caption":{"type":"string"}},"required":["path"]}),
        ),
    ];
    for spec in &mut specs {
        let f = &mut spec["function"];
        if matches!(f["name"].as_str(), Some("files.patch" | "files.write")) {
            let description = f["description"].as_str().unwrap_or_default().to_string();
            f["description"] = format!("{description} Optionally provide verify_command to edit then check in one call. Verification failure leaves the edit applied; inspect verification output rather than repeating the edit.").into();
            let properties = f["parameters"]["properties"].as_object_mut().unwrap();
            properties.insert("verify_command".into(), serde_json::json!({"type":"string","description":"Explicit foreground shell check after a successful edit; no automatic test guessing."}));
            properties.insert("verify_cwd".into(), serde_json::json!({"type":"string"}));
            properties.insert(
                "verify_timeout_seconds".into(),
                serde_json::json!({"type":"number","exclusiveMinimum":0,"maximum":86400}),
            );
        }
    }
    specs
}

fn memory_tool_specs() -> Vec<serde_json::Value> {
    vec![
        openai_tool("memory.list", "List memory files in builtin order then sorted topics. Small results stay plain text. Continue paged results with after=next_after; default limit 50, maximum 200.", serde_json::json!({"type":"object","properties":{"limit":{"type":"integer","minimum":1,"maximum":200},"after":{"type":"string"}}})),
        openai_tool(
            "memory.read",
            "Read SOUL.md, USER.md, MEMORY.md, INDEX.md, or a topic memory file. Small files return once; larger results include continuation for memory.read. offset_chars refers to redacted text and cannot be combined with line ranges. Set include_version=true before an edit that needs expected_sha256.",
            serde_json::json!({"type":"object","properties":{"file":{"type":"string"},"topic":{"type":"string"},"start_line":{"type":"integer","minimum":1},"end_line":{"type":"integer","minimum":1},"offset_chars":{"type":"integer","minimum":0},"max_chars":{"type":"integer","minimum":1},"include_version":{"type":"boolean"}}}),
        ),
        openai_tool(
            "memory.search",
            "Search INDEX.md manifest only, not topic bodies. Missing/unreadable index is an error, not no matches. Returns match-centered snippets and continuation when more matches remain.",
            serde_json::json!({"type":"object","properties":{"query":{"type":"string"},"start_line":{"type":"integer","minimum":1},"max_results":{"type":"integer","minimum":1,"maximum":100}},"required":["query"]}),
        ),
        openai_tool(
            "memory.write",
            "Atomically append to or replace a memory file; concurrent writers are serialized. Optional expected_sha256 (or missing for a new file) rejects stale edits. For topics, index_description explicitly updates its INDEX.md entry in the same call; optional expected_index_sha256 checks the index version. If applied=true but index_updated=false with an error, update only INDEX.md; do not append the topic again. Targets: SOUL.md, USER.md, MEMORY.md, INDEX.md, or topics/<name>.md. Memory files must use frontmatter:\n---\ntype: soul|user|feedback|project|reference|system\ndescription: one-line description\nupdated: YYYY-MM-DD\n---",
            serde_json::json!({"type":"object","properties":{"file":{"type":"string"},"topic":{"type":"string"},"content":{"type":"string"},"mode":{"type":"string","enum":["append","replace"]},"expected_sha256":{"type":"string"},"index_description":{"type":"string","description":"Single-line topic index description, maximum 400 characters."},"expected_index_sha256":{"type":"string"}},"required":["content"]}),
        ),
    ]
}

fn hardware_tool_specs() -> Vec<serde_json::Value> {
    vec![
        openai_tool(
            "hardware.probe",
            "List host hardware capabilities and standard device nodes without opening the devices.",
            serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
        ),
        openai_tool(
            "hardware.devices",
            "List configured hardware device aliases, transports and capabilities.",
            serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
        ),
        openai_tool(
            "serial.read",
            "Consume available serial bytes once. encoding=auto returns readable text or hex; text/hex can be requested. Large results have a saved JSON archive. Recover consumed data from that archive, not by rereading the device.",
            serde_json::json!({"type":"object","properties":{"device":{"type":"string"},"max_bytes":{"type":"integer","minimum":1},"encoding":{"type":"string","enum":["auto","text","hex"]}},"required":["device"],"additionalProperties":false}),
        ),
        openai_tool(
            "serial.write",
            "Write text or hex bytes. Success confirms only bytes accepted by the local transport, not remote execution. Verify unknown/partial outcomes before retrying.",
            serde_json::json!({"type":"object","properties":{"device":{"type":"string"},"data":{"type":"string"},"encoding":{"type":"string","enum":["text","hex"]}},"required":["device","data"],"additionalProperties":false}),
        ),
        openai_tool(
            "gpio.read",
            "Read a GPIO pin through a configured serial JSON controller.",
            serde_json::json!({"type":"object","properties":{"device":{"type":"string"},"pin":{"type":"integer","minimum":0}},"required":["device","pin"],"additionalProperties":false}),
        ),
        openai_tool(
            "gpio.write",
            "Write a GPIO pin through a configured serial JSON controller.",
            serde_json::json!({"type":"object","properties":{"device":{"type":"string"},"pin":{"type":"integer","minimum":0},"value":{"type":"integer","enum":[0,1]}},"required":["device","pin","value"],"additionalProperties":false}),
        ),
        openai_tool(
            "i2c.scan",
            "Scan I2C addresses through a configured serial JSON controller.",
            serde_json::json!({"type":"object","properties":{"device":{"type":"string"}},"required":["device"],"additionalProperties":false}),
        ),
        openai_tool(
            "i2c.read",
            "Read bytes from an I2C address through a configured serial JSON controller.",
            serde_json::json!({"type":"object","properties":{"device":{"type":"string"},"address":{"type":"integer","minimum":0,"maximum":127},"register":{"type":"integer","minimum":0,"maximum":255},"length":{"type":"integer","minimum":1}},"required":["device","address","length"],"additionalProperties":false}),
        ),
        openai_tool(
            "i2c.write",
            "Write hexadecimal bytes to an I2C address through a configured serial JSON controller.",
            serde_json::json!({"type":"object","properties":{"device":{"type":"string"},"address":{"type":"integer","minimum":0,"maximum":127},"register":{"type":"integer","minimum":0,"maximum":255},"data":{"type":"string","description":"Non-empty even-length hexadecimal bytes."}},"required":["device","address","data"],"additionalProperties":false}),
        ),
        openai_tool(
            "spi.transfer",
            "Transfer hexadecimal bytes through a configured serial JSON SPI controller.",
            serde_json::json!({"type":"object","properties":{"device":{"type":"string"},"data":{"type":"string","description":"Non-empty even-length hexadecimal bytes."}},"required":["device","data"],"additionalProperties":false}),
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
    if is_hardware_tool(name)
        && (!context.config.hardware.enabled
            || !context
                .config
                .tools
                .enabled
                .iter()
                .any(|category| category == "hardware"))
    {
        return Ok(ToolResult::error(
            "unknown_tool",
            format!("Unknown tool: {}", name),
        ));
    }
    if matches!(name, "files.patch" | "files.write") {
        if let Err(error) = crate::tool_verification::validate(args, context) {
            return Ok(error);
        }
    }
    let result = match name {
        "files.list" => crate::file_ops::list(args, context),
        "files.read" => crate::file_ops::read(args, context),
        "files.search" => crate::file_ops::search(args, context),
        #[cfg(unix)]
        "files.patch" => crate::file_ops::patch(args, context),
        #[cfg(unix)]
        "shell.poll" => crate::jobs::poll(args, context),
        #[cfg(unix)]
        "shell.cancel" => crate::jobs::cancel(args, context),
        #[cfg(unix)]
        "task.start" | "task.update" | "task.inspect" => {
            let value = serde_json::Value::Object(
                args.iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            if matches!(
                                k.as_str(),
                                "steps" | "acceptance" | "evidence_ids" | "step" | "replace"
                            ) {
                                serde_json::from_str(v)
                                    .unwrap_or_else(|_| serde_json::Value::String(v.clone()))
                            } else {
                                serde_json::Value::String(v.clone())
                            },
                        )
                    })
                    .collect(),
            );
            crate::tasks::run(name, &value, context)
        }
        "files.write" => files_write(args, context),
        "files.send" => files_send(args, context),
        "shell.run" => shell_run(args, context),
        "memory.list" => crate::memory_tools::list(args, context),
        "memory.read" => crate::memory_tools::read(args, context),
        "memory.search" => crate::memory_tools::search(args, context),
        "memory.write" => crate::memory_tools::write(args, context),
        "skill.read" => crate::skills::read_skill(args, context),
        "skill.run" => run_skill_command(args, context),
        "image.understand" => image_understand(args, context),
        "hardware.probe" => hardware_probe(context),
        "hardware.devices" => hardware_devices(context),
        "serial.read" => hardware_serial_read(args, context),
        "serial.write" => hardware_serial_write(args, context),
        "gpio.read" => hardware_controller_tool(args, context, "gpio", "gpio_read"),
        "gpio.write" => hardware_controller_tool(args, context, "gpio", "gpio_write"),
        "i2c.scan" => hardware_controller_tool(args, context, "i2c", "i2c_scan"),
        "i2c.read" => hardware_controller_tool(args, context, "i2c", "i2c_read"),
        "i2c.write" => hardware_controller_tool(args, context, "i2c", "i2c_write"),
        "spi.transfer" => hardware_controller_tool(args, context, "spi", "spi_transfer"),
        _ => ToolResult::error("unknown_tool", format!("Unknown tool: {}", name)),
    };
    Ok(if matches!(name, "files.patch" | "files.write") {
        crate::tool_verification::after_edit(result, args, context)
    } else {
        result
    })
}

fn files_write(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some(raw_path) = args.get("path").filter(|value| !value.is_empty()) else {
        return ToolResult::error("invalid_arguments", "Missing path");
    };
    let path = resolve_arg_path(raw_path, context);
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

pub(crate) fn shell_run(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let command = args.get("command").cloned().unwrap_or_default();
    if command.trim().is_empty() {
        return ToolResult::error("invalid_arguments", "Missing command");
    }
    match shell_words::split(&command) {
        Ok(argv) if !argv.is_empty() => argv,
        Ok(_) => return ToolResult::error("invalid_arguments", "Missing command"),
        Err(error) => return ToolResult::error("invalid_arguments", error.to_string()),
    };
    #[cfg(unix)]
    if args.get("background").is_some_and(|v| v == "true") {
        return crate::jobs::start(args, context);
    }
    let mut process = platform_shell_command(&command);
    let cwd = args
        .get("cwd")
        .map(|v| resolve_arg_path(v, context))
        .unwrap_or_else(|| context.cwd.clone());
    process.current_dir(cwd);
    let seconds = match args.get("timeout_seconds") {
        Some(v) => match v.parse::<f64>() {
            Ok(n) if n.is_finite() && n > 0.0 && n <= 86400.0 => n,
            _ => return ToolResult::error("invalid_arguments", "Invalid timeout_seconds"),
        },
        None => context.config.tools.max_shell_seconds.max(0.0),
    };
    let timeout = Duration::from_secs_f64(seconds);
    match run_tool_command(process, timeout, context) {
        Err(CommandRunError::Cancelled) => ToolResult::error(
            "cancelled",
            "Operation cancelled; previous side effects are not rolled back",
        ),
        Err(CommandRunError::Timeout) => ToolResult::error("timeout", "Command timed out"),
        Err(CommandRunError::Io(error)) => ToolResult::error("execution_error", error),
        Ok(output) => {
            let mut result = if output.success {
                ToolResult::ok(output.text)
            } else {
                ToolResult::error(output.termination.unwrap_or("nonzero_exit"), output.text)
            };
            result.truncated = output.truncated;
            result
        }
    }
}

fn platform_shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut process = Command::new("cmd");
        process.arg("/C").arg(command);
        process
    }
    #[cfg(not(windows))]
    {
        let mut process = Command::new("sh");
        process.arg("-c").arg(command);
        process
    }
}

pub(crate) fn memory_target(
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
    if matches!(file, "SOUL.md" | "USER.md" | "MEMORY.md" | "INDEX.md") {
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

fn image_understand(args: &BTreeMap<String, String>, context: &ToolContext) -> ToolResult {
    let Some(raw_path) = args.get("path").filter(|value| !value.is_empty()) else {
        return ToolResult::error("invalid_arguments", "Missing path");
    };
    let Some(analyzer) = &context.image_analyzer else {
        return ToolResult::error("vision_unavailable", "Image understanding is unavailable");
    };
    let path = resolve_arg_path(raw_path, context);
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

fn resolve_arg_path(value: &str, context: &ToolContext) -> PathBuf {
    let path = expand_user_path(value);
    let joined = if path.is_absolute() {
        path
    } else {
        context.cwd.join(path)
    };
    canonicalize_existing_prefix(&joined)
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
