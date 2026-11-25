use crate::{messages::ToolResult, tools::ToolContext};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    path::PathBuf,
};
fn path(raw: &str, c: &ToolContext) -> PathBuf {
    let p = crate::config::expand_user_path(raw);
    if p.is_absolute() {
        p
    } else {
        c.cwd.join(p)
    }
}
fn number(a: &BTreeMap<String, String>, key: &str, default: usize) -> Result<usize, ()> {
    match a.get(key) {
        None => Ok(default),
        Some(s) => s.parse().ok().filter(|n| *n > 0).ok_or(()),
    }
}

pub(crate) fn read(a: &BTreeMap<String, String>, c: &ToolContext) -> ToolResult {
    let Some(raw) = a.get("path").filter(|s| !s.is_empty()) else {
        return ToolResult::error("invalid_arguments", "Missing path");
    };
    let p = path(raw, c);
    if !p.exists() {
        return ToolResult::error("not_found", "Path does not exist");
    }
    if !p.is_file() {
        return ToolResult::error("not_file", "Path is not a file");
    }
    let (Ok(start), Ok(end), Ok(cap)) = (
        number(a, "start_line", 1),
        number(a, "end_line", usize::MAX),
        number(a, "max_chars", c.config.tools.max_result_chars),
    ) else {
        return ToolResult::error("invalid_arguments", "Invalid line range or max_chars");
    };
    if start > end {
        return ToolResult::error("invalid_arguments", "start_line must be <= end_line");
    }
    let cap = cap.min(c.config.tools.max_result_chars);
    let config_file = p
        .file_name()
        .and_then(|v| v.to_str())
        .is_some_and(|n| n.starts_with("config.toml") || n.starts_with("settings.json"));
    let result = (|| -> io::Result<(String, bool)> {
        if config_file {
            // Parse complete bounded config documents before slicing so multiline secrets stay hidden.
            let mut bytes = Vec::new();
            fs::File::open(&p)?.take(131073).read_to_end(&mut bytes)?;
            if bytes.len() > 131072 {
                return Err(io::Error::other(
                    "Config exceeds safe parsing bound; avoid dumping credentials",
                ));
            }
            let mut text = String::from_utf8(bytes).map_err(io::Error::other)?;
            crate::secrets::redact_config_content(&mut text);
            let text = text
                .split_inclusive('\n')
                .enumerate()
                .filter(|(i, _)| *i >= start - 1 && *i < end)
                .map(|(_, v)| v)
                .collect::<String>();
            let (text, truncated) = crate::memory::truncate(text, cap);
            return Ok((text, truncated));
        }
        let guard = crate::secrets::values(&c.config)
            .iter()
            .map(String::len)
            .max()
            .unwrap_or(0);
        let bound = cap
            .saturating_mul(4)
            .saturating_add(guard)
            .saturating_add(8);
        let mut reader = BufReader::new(fs::File::open(&p)?);
        let mut output = Vec::new();
        let mut line = 1usize;
        let mut clipped = false;
        'scan: loop {
            let buf = reader.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            let mut consumed = 0;
            for &byte in buf {
                if line > end {
                    break 'scan;
                }
                if line >= start {
                    if output.len() >= bound {
                        clipped = true;
                        break 'scan;
                    }
                    output.push(byte);
                }
                consumed += 1;
                if byte == b'\n' {
                    line = line.saturating_add(1)
                }
            }
            reader.consume(consumed);
        }
        let mut text = match String::from_utf8(output) {
            Ok(s) => s,
            Err(e) if clipped => {
                let good = e.utf8_error().valid_up_to();
                String::from_utf8(e.into_bytes()[..good].to_vec()).map_err(io::Error::other)?
            }
            Err(e) => return Err(io::Error::other(e)),
        };
        crate::secrets::redact_tool_output(&c.config, &mut text);
        let (text, truncated) = crate::memory::truncate(text, cap);
        Ok((text, truncated || clipped))
    })();
    match result {
        Ok((mut text, truncated)) => {
            crate::secrets::redact_tool_output(&c.config, &mut text);
            let mut r = ToolResult::ok(text);
            r.truncated = truncated;
            r
        }
        Err(e) => ToolResult::error("io_error", e.to_string()),
    }
}

pub(crate) fn search(a: &BTreeMap<String, String>, c: &ToolContext) -> ToolResult {
    let Some(query) = a.get("query").filter(|s| !s.is_empty()) else {
        return ToolResult::error("invalid_arguments", "Missing literal query");
    };
    let root = path(a.get("path").map(String::as_str).unwrap_or("."), c);
    let limit = number(a, "max_results", 30).unwrap_or(30).min(100);
    let mut stack = vec![root];
    let mut matches = Vec::new();
    let mut scanned = 0;
    let mut budget = 16 * 1024 * 1024usize;
    let mut truncated = false;
    let mut result_bytes = 0;
    let mut visited = 0;
    while let Some(p) = stack.pop() {
        visited += 1;
        if visited > 4096 || budget == 0 || matches.len() >= limit {
            truncated = true;
            break;
        }
        if p.is_dir() {
            if let Ok(entries) = fs::read_dir(&p) {
                let mut paths = entries
                    .flatten()
                    .take(4097usize.saturating_sub(visited + stack.len()))
                    .filter(|e| !e.file_type().is_ok_and(|t| t.is_symlink()))
                    .map(|e| e.path())
                    .collect::<Vec<_>>();
                paths.sort();
                paths.reverse();
                if paths.len() + visited + stack.len() >= 4097 {
                    truncated = true;
                }
                stack.extend(paths)
            }
            continue;
        }
        if !p.is_file() {
            continue;
        }
        scanned += 1;
        let Ok(file) = fs::File::open(&p) else {
            continue;
        };
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        let mut n = 0;
        loop {
            line.clear();
            let max = budget.min(65536);
            if max == 0 {
                truncated = true;
                break;
            }
            let Ok(count) = reader
                .by_ref()
                .take(max as u64)
                .read_until(b'\n', &mut line)
            else {
                break;
            };
            if count == 0 {
                break;
            }
            budget -= count;
            n += 1;
            if line.contains(&0) {
                break;
            }
            if !line.ends_with(b"\n") && count == max {
                truncated = true;
                break;
            }
            let text = String::from_utf8_lossy(&line);
            if text.contains(query) {
                let mut text = text.trim_end().to_string();
                if p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("config.toml") || n.starts_with("settings.json"))
                {
                    text = "[configuration match; use files.read for redacted content]".into();
                }
                crate::secrets::redact_tool_output(&c.config, &mut text);
                let text = crate::memory::truncate(text, 500).0;
                result_bytes += text.len() + p.as_os_str().len() + 64;
                if result_bytes > c.config.tools.max_result_chars {
                    truncated = true;
                    break;
                }
                matches.push(serde_json::json!({"path":p,"line":n,"text":text}));
                if matches.len() >= limit {
                    truncated = true;
                    break;
                }
            }
        }
        if result_bytes > c.config.tools.max_result_chars {
            break;
        }
    }
    let mut result = ToolResult::ok(
        serde_json::json!({"matches":matches,"scanned_files":scanned,"truncated":truncated})
            .to_string(),
    );
    result.truncated = truncated;
    result
}

#[cfg(unix)]
pub(crate) fn patch(a: &BTreeMap<String, String>, c: &ToolContext) -> ToolResult {
    let (Some(raw), Some(old), Some(new)) = (a.get("path"), a.get("old_text"), a.get("new_text"))
    else {
        return ToolResult::error("invalid_arguments", "Need path, old_text and new_text");
    };
    if old.is_empty() {
        return ToolResult::error("invalid_arguments", "old_text must not be empty");
    }
    let result = (|| -> Result<String, String> {
        let p = path(raw, c).canonicalize().map_err(|e| e.to_string())?;
        if fs::metadata(&p).map_err(|e| e.to_string())?.len() > 1024 * 1024 {
            return Err("File exceeds 1 MiB patch memory bound; use streaming system tools".into());
        }
        let parent = p.parent().ok_or("Missing parent")?;
        let _lock = crate::config_io::lock(&parent.join(".colibri-patch.lock"))
            .map_err(|e| e.to_string())?;
        let mut before = String::new();
        fs::File::open(&p)
            .map_err(|e| e.to_string())?
            .take(1048577)
            .read_to_string(&mut before)
            .map_err(|e| e.to_string())?;
        if before.len() > 1048576 {
            return Err("File exceeds patch memory bound".into());
        }
        let hash = format!("{:x}", Sha256::digest(before.as_bytes()));
        if a.get("expected_sha256").is_some_and(|v| v != &hash) {
            return Err("stale_file: file changed since it was read".into());
        }
        if before.match_indices(old).count() != 1 {
            return Err(
                "old_text must match exactly once; reread and include unique context".into(),
            );
        }
        let after = before.replacen(old, new, 1);
        if after.len() > 2 * 1024 * 1024 {
            return Err("Replacement exceeds patch memory bound".into());
        }
        let scratch = crate::config_io::Scratch::new(parent).map_err(|e| e.to_string())?;
        let temp = scratch.0.join("value");
        let mut f = fs::File::create(&temp).map_err(|e| e.to_string())?;
        f.set_permissions(fs::metadata(&p).map_err(|e| e.to_string())?.permissions())
            .map_err(|e| e.to_string())?;
        f.write_all(after.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        if fs::read(&p).map_err(|e| e.to_string())? != before.as_bytes() {
            return Err("stale_file: file changed during patch preparation".into());
        }
        fs::rename(temp, &p).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({"path":p,"replacements":1,"sha256":format!("{:x}",Sha256::digest(after.as_bytes()))}).to_string())
    })();
    match result {
        Ok(text) => ToolResult::ok(text),
        Err(error) => ToolResult::error("patch_failed", error),
    }
}
