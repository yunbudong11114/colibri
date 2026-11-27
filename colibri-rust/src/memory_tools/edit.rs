//! Prepare under the Memory lock, then commit each file atomically.
use super::{io_error, optional_version, sha, version, Args, Version};
use crate::{
    messages::ToolResult,
    tools::{memory_target, ToolContext},
};
use std::{
    fs,
    io::{self, BufRead, BufReader, Read},
    path::Path,
};
fn validate_expected(a: &Args, k: &str) -> Result<(), ToolResult> {
    if a.get(k).is_some_and(|v| {
        v != "missing" && (v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
    }) {
        return Err(ToolResult::error(
            "invalid_arguments",
            format!("{k} must be a SHA-256 digest or missing"),
        ));
    }
    Ok(())
}
#[cfg(unix)]
fn stage_text(
    stage: &Path,
    target: &Path,
    content: &str,
    append: bool,
    before: &Option<Version>,
) -> io::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(stage)?;
    if append {
        if let Some(v) = before {
            let mut old = fs::File::open(target)?;
            let mut chunk = [0u8; 8192];
            loop {
                if crate::cancellation::cancelled() {
                    return Err(io::Error::other("cancelled during Memory preparation"));
                }
                let n = old.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                file.write_all(&chunk[..n])?;
            }
            if v.last.is_some_and(|b| b != b'\n') {
                file.write_all(b"\n")?;
            }
        }
    }
    file.write_all(content.as_bytes())?;
    if !content.ends_with('\n') {
        file.write_all(b"\n")?;
    }
    file.sync_all()
}
#[cfg(unix)]
fn stage_index(stage: &Path, target: &Path, label: &str, description: &str) -> io::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(stage)?;
    let name = label
        .strip_prefix("topics/")
        .unwrap_or(label)
        .trim_end_matches(".md");
    let needle = format!("]({label})");
    let entry = format!("- [{name}]({label}): {description}\n");
    let mut found = false;
    let mut last = None;
    match fs::File::open(target) {
        Ok(old) => {
            let mut reader = BufReader::new(old);
            let mut line = Vec::new();
            loop {
                if crate::cancellation::cancelled() {
                    return Err(io::Error::other("cancelled during index preparation"));
                }
                line.clear();
                let n = reader.by_ref().take(65537).read_until(b'\n', &mut line)?;
                if n == 0 {
                    break;
                }
                if n > 65536 {
                    return Err(io::Error::other("INDEX.md contains a line exceeding 64 KiB; edit it explicitly before updating its topic entry"));
                }
                let text = std::str::from_utf8(&line).map_err(io::Error::other)?;
                if text.trim_start().starts_with("- [") && text.contains(&needle) {
                    if !found {
                        file.write_all(entry.as_bytes())?;
                        last = Some(b'\n');
                        found = true;
                    }
                } else {
                    file.write_all(&line)?;
                    last = line.last().copied();
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    if !found {
        if last.is_some_and(|b| b != b'\n') {
            file.write_all(b"\n")?;
        }
        file.write_all(entry.as_bytes())?;
    }
    file.sync_all()
}

#[cfg(unix)]
pub(crate) fn write(a: &Args, c: &ToolContext) -> ToolResult {
    let Some((target, label)) = memory_target(a, c) else {
        return ToolResult::error("invalid_arguments", "Invalid memory file");
    };
    let Some(content) = a.get("content").map(|s| s.trim()).filter(|v| !v.is_empty()) else {
        return ToolResult::error("invalid_arguments", "Missing content");
    };
    let mode = a.get("mode").map(String::as_str).unwrap_or("append");
    if !matches!(mode, "append" | "replace") {
        return ToolResult::error("invalid_arguments", "Invalid write mode");
    }
    for key in ["expected_sha256", "expected_index_sha256"] {
        if let Err(r) = validate_expected(a, key) {
            return r;
        }
    }
    let description = a.get("index_description").map(|v| v.trim());
    if description.is_some_and(|d| {
        !label.starts_with("topics/")
            || d.is_empty()
            || d.contains(['\n', '\r'])
            || d.chars().count() > 400
    }) {
        return ToolResult::error("invalid_arguments","index_description requires a topic and a non-empty single line of at most 400 characters");
    }
    if description.is_none() && a.contains_key("expected_index_sha256") {
        return ToolResult::error(
            "invalid_arguments",
            "expected_index_sha256 requires index_description",
        );
    }
    let root = &c.config.memory.root;
    let index = root.join("INDEX.md");
    let unsafe_link = || -> bool {
        fs::symlink_metadata(&target).is_ok_and(|m| m.is_symlink())
            || (label.starts_with("topics/")
                && fs::symlink_metadata(root.join("topics")).is_ok_and(|m| m.is_symlink()))
            || (description.is_some() && fs::symlink_metadata(&index).is_ok_and(|m| m.is_symlink()))
    };
    if unsafe_link() {
        return ToolResult::error(
            "invalid_arguments",
            "Memory writes do not support symbolic links",
        );
    }
    if crate::cancellation::cancelled() {
        return ToolResult::error("cancelled", "Memory write cancelled; no changes applied");
    }
    if let Err(e) = fs::create_dir_all(root) {
        return io_error(e);
    }
    let _lock = match crate::config_io::lock_interruptible(&root.join(".memory.lock")) {
        Ok(l) => l,
        Err(e) => {
            return ToolResult::error(
                if crate::cancellation::cancelled() {
                    "cancelled"
                } else {
                    "io_error"
                },
                e.to_string(),
            )
        }
    };
    if unsafe_link() {
        return ToolResult::error("invalid_arguments", "Memory target became a symbolic link");
    }
    let before = match optional_version(&target) {
        Ok(v) => v,
        Err(e) => return io_error(e),
    };
    if a.get("expected_sha256")
        .is_some_and(|v| !v.eq_ignore_ascii_case(sha(&before)))
    {
        return ToolResult::error("stale_file","Memory changed since read; no changes applied. Read it again with include_version=true.");
    }
    let index_before = if description.is_some() {
        match optional_version(&index) {
            Ok(v) => v,
            Err(e) => return io_error(e),
        }
    } else {
        None
    };
    if a.get("expected_index_sha256")
        .is_some_and(|v| !v.eq_ignore_ascii_case(sha(&index_before)))
    {
        return ToolResult::error(
            "stale_index",
            "INDEX.md changed; no changes applied. Read the current index before updating.",
        );
    }
    let parent = target.parent().unwrap_or(root);
    if let Err(e) = fs::create_dir_all(parent) {
        return io_error(e);
    }
    let scratch = match crate::config_io::Scratch::new(parent) {
        Ok(s) => s,
        Err(e) => return io_error(e),
    };
    let staged = scratch.0.join("memory");
    let staged_index = scratch.0.join("index");
    if let Err(e) = stage_text(&staged, &target, content, mode == "append", &before) {
        return io_error(e);
    }
    if let Some(d) = description {
        if let Err(e) = stage_index(&staged_index, &index, &label, d) {
            return io_error(e);
        }
    }
    let after = match version(&staged) {
        Ok(v) => v,
        Err(e) => return io_error(e),
    };
    match optional_version(&target) {
        Ok(v) if v == before => {}
        Ok(_) => {
            return ToolResult::error(
                "stale_file",
                "Memory changed during preparation; no changes applied",
            )
        }
        Err(e) => return io_error(e),
    }
    if description.is_some() {
        match optional_version(&index) {
            Ok(v) if v == index_before => {}
            Ok(_) => {
                return ToolResult::error(
                    "stale_index",
                    "INDEX.md changed during preparation; no changes applied",
                )
            }
            Err(e) => return io_error(e),
        }
    }
    if crate::cancellation::cancelled() {
        return ToolResult::error("cancelled", "Memory write cancelled; no changes applied");
    }
    if unsafe_link() {
        return ToolResult::error(
            "stale_file",
            "Memory target changed during preparation; no changes applied",
        );
    }
    if let Err(e) = fs::rename(&staged, &target) {
        return io_error(e);
    }
    let index_result = description.map(|_| commit_index(&staged_index, &index, &index_before));
    write_result(&label, mode, &before, &after, index_result)
}

fn commit_index(staged: &Path, index: &Path, before: &Option<Version>) -> Result<String, String> {
    if crate::cancellation::cancelled() {
        return Err("cancelled after topic commit".into());
    }
    let prepared = version(staged).map_err(|e| e.to_string())?;
    match optional_version(index) {
        Ok(v) if &v == before => {}
        Ok(_) => return Err("INDEX.md changed after topic commit".into()),
        Err(e) => return Err(e.to_string()),
    }
    if crate::cancellation::cancelled() {
        return Err("cancelled after topic commit".into());
    }
    fs::rename(staged, index).map_err(|e| e.to_string())?;
    Ok(prepared.sha)
}

fn write_result(
    label: &str,
    mode: &str,
    before: &Option<Version>,
    after: &Version,
    index_result: Option<Result<String, String>>,
) -> ToolResult {
    let requested = index_result.is_some();
    let (index_updated, index_sha, index_error) = match index_result {
        Some(Ok(sha)) => (true, Some(sha), None),
        Some(Err(error)) => (false, None, Some(error)),
        None => (false, None, None),
    };
    let suggested = match label {
        "SOUL.md" | "USER.md" => Some(1000),
        "MEMORY.md" => Some(2000),
        _ => None,
    };
    let text=serde_json::json!({"file":label,"applied":true,"mode":mode,"previous_sha256":sha(before),"sha256":after.sha,"bytes":after.bytes,"characters":after.chars,
        "suggested_max_chars":suggested,"exceeds_suggested_length":suggested.is_some_and(|n|after.chars>n),"index_requested":requested,"index_updated":index_updated,"index_sha256":index_sha,"index_error":index_error,
        "note":if index_error.is_some(){"Topic is already applied. Update INDEX.md only; do not append the topic content again."}else if label.starts_with("topics/") && !requested{"Topic applied; supply index_description or update INDEX.md separately so memory.search can find it."}else{"Memory updated."}}).to_string();
    if index_error.is_some() {
        ToolResult::error(
            if crate::cancellation::cancelled() {
                "cancelled"
            } else {
                "index_update_failed"
            },
            text,
        )
    } else {
        ToolResult::ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_or_cancelled_index_commit_reports_topic_as_already_applied() {
        let scratch = crate::config_io::Scratch::new(&std::env::temp_dir()).unwrap();
        let topic = scratch.0.join("topic.md");
        fs::write(&topic, "already applied\n").unwrap();
        let after = version(&topic).unwrap();
        let index = scratch.0.join("INDEX.md");
        let missing_stage = scratch.0.join("missing-stage");
        let outcome = commit_index(&missing_stage, &index, &None);
        assert!(outcome.is_err());
        let result = write_result("topics/topic.md", "append", &None, &after, Some(outcome));
        assert!(!result.ok);
        assert_eq!(result.error_type.as_deref(), Some("index_update_failed"));
        let v: serde_json::Value = serde_json::from_str(&result.text).unwrap();
        assert_eq!(v["applied"], true);
        assert_eq!(v["index_updated"], false);
        assert!(v["note"].as_str().unwrap().contains("do not append"));
        assert_eq!(fs::read_to_string(&topic).unwrap(), "already applied\n");
        let token = crate::cancellation::Cancellation::default();
        let _scope = crate::cancellation::Scope::enter(token.clone());
        token.cancel();
        let result = write_result(
            "topics/topic.md",
            "append",
            &None,
            &after,
            Some(commit_index(&missing_stage, &index, &None)),
        );
        assert_eq!(result.error_type.as_deref(), Some("cancelled"));
        assert!(!index.exists());
    }
    #[test]
    fn waiting_for_memory_lock_can_be_cancelled_without_writing() {
        let scratch = crate::config_io::Scratch::new(&std::env::temp_dir()).unwrap();
        let mut config = crate::config::AgentConfig::default();
        config.memory.root = scratch.0.clone();
        let context = ToolContext::new(config, scratch.0.clone());
        let held = crate::config_io::lock(&scratch.0.join(".memory.lock")).unwrap();
        let token = crate::cancellation::Cancellation::default();
        let worker_token = token.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let _scope = crate::cancellation::Scope::enter(worker_token);
            let args = Args::from([
                ("file".into(), "MEMORY.md".into()),
                ("content".into(), "must not apply".into()),
            ]);
            tx.send(write(&args, &context)).unwrap();
        });
        token.cancel();
        let result = rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
        assert_eq!(result.error_type.as_deref(), Some("cancelled"));
        drop(held);
        handle.join().unwrap();
        assert!(!scratch.0.join("MEMORY.md").exists());
    }
}
