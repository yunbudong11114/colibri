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
    let Some(raw) = a.get("path").filter(|v| !v.is_empty()) else {
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
        number(a, "max_chars", c.config.tools.max_result_chars.min(4096)),
    ) else {
        return ToolResult::error("invalid_arguments", "Invalid line range or max_chars");
    };
    if start > end {
        return ToolResult::error("invalid_arguments", "start_line must be <= end_line");
    }
    let offset = match a.get("offset_chars") {
        Some(v) => match v.parse::<usize>() {
            Ok(n) => Some(n),
            Err(_) => return ToolResult::error("invalid_arguments", "Invalid offset_chars"),
        },
        None => None,
    };
    if offset.is_some() && (a.contains_key("start_line") || a.contains_key("end_line")) {
        return ToolResult::error(
            "invalid_arguments",
            "offset_chars cannot be combined with line ranges",
        );
    }
    let result = (|| -> io::Result<(String, usize, bool)> {
        let config_file = p
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|n| n.starts_with("config.toml") || n.starts_with("settings.json"));
        let mut input: Box<dyn Read> = if config_file {
            let mut bytes = Vec::new();
            fs::File::open(&p)?.take(131073).read_to_end(&mut bytes)?;
            if bytes.len() > 131072 {
                return Err(io::Error::other(
                    "Config exceeds safe parsing bound; avoid dumping credentials",
                ));
            }
            let mut text = String::from_utf8(bytes).map_err(io::Error::other)?;
            crate::secrets::redact_config_content(&mut text);
            Box::new(io::Cursor::new(text.into_bytes()))
        } else {
            Box::new(BufReader::new(fs::File::open(&p)?))
        };
        let mut redactor = crate::secrets::StreamRedactor::new(crate::secrets::values(&c.config));
        let mut chunk = [0u8; 8192];
        let mut pending = Vec::new();
        let mut out = String::new();
        let mut position = 0usize;
        let mut line = 1usize;
        let mut count = 0usize;
        let cap = cap.min(c.config.tools.max_result_chars).max(1);
        loop {
            if crate::cancellation::cancelled() {
                return Err(io::Error::other("cancelled: file read interrupted"));
            }
            let n = input.read(&mut chunk)?;
            pending.extend(redactor.feed(&chunk[..n], n == 0));
            let valid = match std::str::from_utf8(&pending) {
                Ok(_) => pending.len(),
                Err(e) if e.error_len().is_none() && n > 0 => e.valid_up_to(),
                Err(e) => return Err(io::Error::other(e)),
            };
            let text = std::str::from_utf8(&pending[..valid]).map_err(io::Error::other)?;
            for ch in text.chars() {
                if offset.is_none() && line > end {
                    return Ok((out, position, false));
                }
                let selected = offset.map_or(line >= start, |o| position >= o);
                if selected {
                    if count >= cap {
                        return Ok((out, position, true));
                    }
                    out.push(ch);
                    count += 1;
                }
                position = position.saturating_add(1);
                if ch == '\n' {
                    line = line.saturating_add(1);
                }
            }
            pending.drain(..valid);
            if n == 0 {
                return Ok((out, position, false));
            }
        }
    })();
    match result {
        Ok((text, next, more)) => {
            let mut r = ToolResult::ok(if offset.is_some() || more {
                serde_json::json!({"path":p,"text":text,"next_offset_chars":next,"has_more":more,
                "continuation":more.then(||serde_json::json!({"path":p,"offset_chars":next,"max_chars":cap}))}).to_string()
            } else {
                text
            });
            r.truncated = more;
            r
        }
        Err(e) => ToolResult::error(
            if crate::cancellation::cancelled() {
                "cancelled"
            } else {
                "io_error"
            },
            e.to_string(),
        ),
    }
}

pub(crate) fn list(a: &BTreeMap<String, String>, c: &ToolContext) -> ToolResult {
    use std::collections::BinaryHeap;
    let p = path(a.get("path").map(String::as_str).unwrap_or("."), c);
    let Ok(limit) = number(a, "limit", 50) else {
        return ToolResult::error("invalid_arguments", "Invalid limit");
    };
    let limit = limit.min(200);
    let after = a.get("after").map(String::as_str).unwrap_or("");
    let result = (|| -> io::Result<ToolResult> {
        let mut candidates = BinaryHeap::new();
        for entry in fs::read_dir(&p)? {
            if crate::cancellation::cancelled() {
                return Ok(ToolResult::error(
                    "cancelled",
                    "Directory listing interrupted",
                ));
            }
            let e = entry?;
            let mut name = e.file_name().into_string().map_err(|_| {
                io::Error::other("Non-UTF-8 filename; use shell for byte-safe listing")
            })?;
            if e.file_type()?.is_dir() {
                name.push('/');
            }
            if name.as_str() <= after {
                continue;
            }
            candidates.push(name);
            if candidates.len() > limit + 1 {
                candidates.pop();
            }
        }
        let mut names = candidates.into_sorted_vec();
        let mut more = names.len() > limit;
        names.truncate(limit);
        let budget = c.config.tools.max_result_chars.min(4096);
        let mut used = 0usize;
        let mut keep = 0;
        for name in &names {
            if keep > 0 && used + name.len() > budget {
                more = true;
                break;
            }
            used += name.len();
            keep += 1;
        }
        names.truncate(keep);
        let next = if more { names.last().cloned() } else { None };
        let mut r = ToolResult::ok(
            serde_json::json!({"path":p,"entries":names,"has_more":more,"next_after":next})
                .to_string(),
        );
        r.truncated = more;
        Ok(r)
    })();
    result.unwrap_or_else(|e| ToolResult::error("io_error", e.to_string()))
}

pub(crate) fn search(a: &BTreeMap<String, String>, c: &ToolContext) -> ToolResult {
    let Some(query) = a.get("query").filter(|s| !s.is_empty()) else {
        return ToolResult::error("invalid_arguments", "Missing literal query");
    };
    let root = path(a.get("path").map(String::as_str).unwrap_or("."), c);
    if !root.exists() {
        return ToolResult::error("not_found", "Search path does not exist");
    }
    let Ok(limit) = number(a, "max_results", 30) else {
        return ToolResult::error("invalid_arguments", "Invalid max_results");
    };
    let limit = limit.min(100);
    let Ok(resume_line) = number(a, "start_line", 1) else {
        return ToolResult::error("invalid_arguments", "Invalid start_line");
    };
    let after = a.get("after_path").map(|v| path(v, c));
    let mut continuation = None;
    let mut stop_reason = "complete";
    let mut errors = Vec::new();
    let mut scan_incomplete = false;
    let mut limitations = std::collections::BTreeSet::new();
    let mut stack = vec![root];
    let mut matches = Vec::new();
    let mut scanned = 0;
    let mut budget = 16 * 1024 * 1024usize;
    let mut truncated = false;
    let mut result_bytes = 0;
    let mut visited = 0;
    while let Some(p) = stack.pop() {
        visited += 1;
        if crate::cancellation::cancelled() {
            return ToolResult::error("cancelled", "Search interrupted");
        }
        if visited > 4096 || budget == 0 {
            truncated = true;
            stop_reason = "scan_budget";
            continuation = None;
            break;
        }
        if matches.len() >= limit {
            break;
        }
        if p.is_dir() {
            if let Ok(entries) = fs::read_dir(&p) {
                let room = 4097usize.saturating_sub(visited + stack.len());
                let mut paths = Vec::new();
                for entry in entries.take(room + 1) {
                    if paths.len() >= room {
                        truncated = true;
                        scan_incomplete = true;
                        break;
                    }
                    match entry {
                        Ok(e) => match e.file_type() {
                            Ok(t) if !t.is_symlink() => paths.push(e.path()),
                            Ok(_) => {}
                            Err(error) => {
                                truncated = true;
                                scan_incomplete = true;
                                if errors.len() < 8 {
                                    errors.push(serde_json::json!({"path":e.path(),"error":error.to_string()}));
                                }
                            }
                        },
                        Err(error) => {
                            truncated = true;
                            scan_incomplete = true;
                            if errors.len() < 8 {
                                errors
                                    .push(serde_json::json!({"path":p,"error":error.to_string()}));
                            }
                        }
                    }
                }
                paths.sort();
                paths.reverse();
                if paths.len() + visited + stack.len() >= 4097 {
                    truncated = true;
                    stop_reason = "directory_budget";
                    limitations.insert("directory_budget");
                    scan_incomplete = true;
                }
                stack.extend(paths)
            } else {
                truncated = true;
                scan_incomplete = true;
                if errors.len() < 8 {
                    errors.push(serde_json::json!({"path":p,"error":"directory unreadable"}));
                }
            }
            continue;
        }
        if !p.is_file() {
            continue;
        }
        if after.as_ref().is_some_and(|v| p < *v) {
            continue;
        }
        scanned += 1;
        let file = match fs::File::open(&p) {
            Ok(f) => f,
            Err(e) => {
                truncated = true;
                scan_incomplete = true;
                if errors.len() < 8 {
                    errors.push(serde_json::json!({"path":p,"error":e.to_string()}));
                }
                continue;
            }
        };
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        let mut n = 0;
        loop {
            line.clear();
            let max = budget.min(65536);
            if max == 0 {
                truncated = true;
                stop_reason = "scan_budget";
                continuation = None;
                break;
            }
            let count = match reader
                .by_ref()
                .take(max as u64)
                .read_until(b'\n', &mut line)
            {
                Ok(n) => n,
                Err(e) => {
                    truncated = true;
                    scan_incomplete = true;
                    if errors.len() < 8 {
                        errors.push(serde_json::json!({"path":p,"error":e.to_string()}));
                    }
                    break;
                }
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
                stop_reason = "long_line_or_scan_budget";
                limitations.insert("long_line_or_scan_budget");
                scan_incomplete = true;
                continuation = None;
                break;
            }
            if after.as_ref().is_some_and(|v| p == *v) && n < resume_line {
                continue;
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
                let at = text.find(query).unwrap_or(0);
                let chars_before = text[..at].chars().count();
                let text = text
                    .chars()
                    .skip(chars_before.saturating_sub(160))
                    .take(500)
                    .collect::<String>();
                let next_bytes = text.len() + p.as_os_str().len() + 64;
                if !matches.is_empty()
                    && result_bytes + next_bytes > c.config.tools.max_result_chars.min(4096)
                {
                    truncated = true;
                    stop_reason = "result_budget";
                    continuation = Some(serde_json::json!({"after_path":p,"start_line":n}));
                    break;
                }
                result_bytes += next_bytes;
                matches.push(serde_json::json!({"path":p,"line":n,"text":text}));
                if matches.len() >= limit {
                    truncated = true;
                    stop_reason = "match_limit";
                    continuation = Some(serde_json::json!({"after_path":p,"start_line":n+1}));
                    break;
                }
            }
        }
        if continuation.is_some() {
            break;
        }
    }
    if scan_incomplete {
        continuation = None;
        if !errors.is_empty() {
            limitations.insert("io_errors");
        }
        if limitations.is_empty() {
            limitations.insert("directory_budget");
        }
        stop_reason = "incomplete_scan";
    }
    let mut result = ToolResult::ok(
        serde_json::json!({"matches":matches,"scanned_files":scanned,"truncated":truncated,"stop_reason":stop_reason,"limitations":limitations,"continuation":continuation,"errors":errors,"hint":if truncated && continuation.is_none(){Some("Scan incomplete. Narrow path/query or use a streaming shell search.")}else{None}})
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
