//! Memory queries and atomic edits, shared by all clients. Ordinary small results stay compact.
use crate::{
    messages::ToolResult,
    tools::{memory_target, ToolContext},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BinaryHeap},
    fs,
    io::{self, BufRead, BufReader, Read},
    path::Path,
};
type Args = BTreeMap<String, String>;
fn positive(a: &Args, k: &str, default: usize) -> Result<usize, ToolResult> {
    a.get(k).map_or(Ok(default.max(1)), |v| {
        v.parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| ToolResult::error("invalid_arguments", format!("Invalid {k}")))
    })
}
fn io_error(e: io::Error) -> ToolResult {
    if crate::cancellation::cancelled() {
        return ToolResult::error("cancelled", e.to_string());
    }
    ToolResult::error(
        if e.kind() == io::ErrorKind::NotFound {
            "not_found"
        } else {
            "io_error"
        },
        e.to_string(),
    )
}
#[derive(Clone, PartialEq, Eq)]
struct Version {
    sha: String,
    bytes: u64,
    chars: usize,
    last: Option<u8>,
}
fn version(p: &Path) -> io::Result<Version> {
    let mut f = fs::File::open(p)?;
    let mut hash = Sha256::new();
    let mut bytes = 0;
    let mut chars = 0;
    let mut last = None;
    let mut chunk = [0u8; 8192];
    loop {
        if crate::cancellation::cancelled() {
            return Err(io::Error::other("cancelled while reading Memory"));
        }
        let n = f.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        hash.update(&chunk[..n]);
        bytes += n as u64;
        chars += chunk[..n].iter().filter(|b| (**b & 0xc0) != 0x80).count();
        last = Some(chunk[n - 1]);
    }
    Ok(Version {
        sha: format!("{:x}", hash.finalize()),
        bytes,
        chars,
        last,
    })
}
fn optional_version(p: &Path) -> io::Result<Option<Version>> {
    match version(p) {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn sha(v: &Option<Version>) -> &str {
    v.as_ref().map_or("missing", |v| v.sha.as_str())
}

pub(crate) fn read(a: &Args, c: &ToolContext) -> ToolResult {
    let Some((path, label)) = memory_target(a, c) else {
        return ToolResult::error("invalid_arguments", "Invalid memory file");
    };
    if a.get("include_version")
        .is_some_and(|s| !matches!(s.as_str(), "true" | "false"))
    {
        return ToolResult::error("invalid_arguments", "Invalid include_version");
    }
    let with_version = a.get("include_version").is_some_and(|s| s == "true");
    let before = if with_version {
        match version(&path) {
            Ok(v) => Some(v),
            Err(e) => return io_error(e),
        }
    } else {
        None
    };
    let mut args = a.clone();
    args.insert("path".into(), path.to_string_lossy().into());
    let mut r = crate::file_ops::read(&args, c);
    if !r.ok {
        return r;
    }
    let mut page = if a.contains_key("offset_chars") || r.truncated {
        serde_json::from_str::<serde_json::Value>(&r.text).ok()
    } else {
        None
    };
    if let Some(v) = &before {
        match version(&path) {
            Ok(after) if &after == v => {}
            Ok(_) => {
                return ToolResult::error(
                    "stale_file",
                    "Memory changed during read; read again before editing",
                )
            }
            Err(e) => return io_error(e),
        }
        if page.is_none() {
            page = Some(serde_json::json!({"text":r.text,"has_more":false}));
        }
        page.as_mut().unwrap()["sha256"] = serde_json::json!(v.sha);
        page.as_mut().unwrap()["bytes"] = serde_json::json!(v.bytes);
    }
    if let Some(mut page) = page {
        page.as_object_mut().unwrap().remove("path");
        page["file"] = serde_json::json!(label);
        if page["has_more"] == true {
            page["continuation"] = serde_json::json!({"file":label,"offset_chars":page["next_offset_chars"],"max_chars":a.get("max_chars").and_then(|v|v.parse::<usize>().ok()).unwrap_or(c.config.tools.max_result_chars.min(4096))});
        }
        r.text = page.to_string();
    }
    r
}

pub(crate) fn list(a: &Args, c: &ToolContext) -> ToolResult {
    let limit = match positive(a, "limit", 50) {
        Ok(n) => n.min(200),
        Err(r) => return r,
    };
    let after = a.get("after").map(String::as_str);
    let key = |s: &str| -> (u8, String) {
        match s {
            "SOUL.md" => (0, String::new()),
            "USER.md" => (1, String::new()),
            "MEMORY.md" => (2, String::new()),
            "INDEX.md" => (3, String::new()),
            _ => (4, s.to_string()),
        }
    };
    let after = after.map(key);
    let mut heap = BinaryHeap::new();
    let mut add = |s: String| {
        let k = key(&s);
        if after.as_ref().is_none_or(|v| k > *v) {
            heap.push((k, s));
            if heap.len() > limit + 1 {
                heap.pop();
            }
        }
    };
    for name in ["SOUL.md", "USER.md", "MEMORY.md", "INDEX.md"] {
        match fs::metadata(c.config.memory.root.join(name)) {
            Ok(m) if m.is_file() => add(name.into()),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return io_error(e),
        }
    }
    let topics = c.config.memory.root.join("topics");
    match fs::read_dir(topics) {
        Ok(entries) => {
            for e in entries {
                if crate::cancellation::cancelled() {
                    return ToolResult::error("cancelled", "Memory listing interrupted");
                }
                let e = match e {
                    Ok(e) => e,
                    Err(e) => return io_error(e),
                };
                let t = match e.file_type() {
                    Ok(t) => t,
                    Err(e) => return io_error(e),
                };
                if t.is_file() && e.path().extension().is_some_and(|v| v == "md") {
                    let Ok(name) = e.file_name().into_string() else {
                        return ToolResult::error(
                            "invalid_filename",
                            "Memory topic name is not UTF-8",
                        );
                    };
                    add(format!("topics/{name}"));
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return io_error(e),
    }
    let mut names = heap
        .into_sorted_vec()
        .into_iter()
        .map(|(_, s)| s)
        .collect::<Vec<_>>();
    let mut more = names.len() > limit;
    names.truncate(limit);
    let mut used = 0;
    let mut kept = 0;
    for name in &names {
        if kept > 0 && used + name.len() > c.config.tools.max_result_chars.min(4096) {
            more = true;
            break;
        }
        used += name.len() + 1;
        kept += 1;
    }
    names.truncate(kept);
    let mut r = ToolResult::ok(
        if more || a.contains_key("after") || a.contains_key("limit") {
            serde_json::json!({"files":names,"has_more":more,"next_after":if more{names.last()}else{None}}).to_string()
        } else {
            names.join("\n")
        },
    );
    r.truncated = more;
    r
}

pub(crate) fn search(a: &Args, c: &ToolContext) -> ToolResult {
    let Some(q) = a.get("query").map(|v| v.trim()).filter(|v| !v.is_empty()) else {
        return ToolResult::error("invalid_arguments", "Missing query");
    };
    let query = q.to_lowercase();
    let start = match positive(a, "start_line", 1) {
        Ok(n) => n,
        Err(r) => return r,
    };
    let limit = match positive(a, "max_results", c.config.memory.max_search_results) {
        Ok(n) => n.min(100),
        Err(r) => return r,
    };
    let file = match fs::File::open(c.config.memory.root.join("INDEX.md")) {
        Ok(f) => f,
        Err(e) => return io_error(e),
    };
    let mut reader = BufReader::new(file);
    let mut row = Vec::new();
    let mut n = 0;
    let mut results = Vec::new();
    let mut used = 0;
    let mut next = None;
    let mut reason = "complete";
    loop {
        if crate::cancellation::cancelled() {
            return ToolResult::error("cancelled", "Memory search interrupted");
        }
        row.clear();
        let count = match reader.by_ref().take(65537).read_until(b'\n', &mut row) {
            Ok(n) => n,
            Err(e) => return io_error(e),
        };
        if count == 0 {
            break;
        }
        n += 1;
        if count > 65536 {
            reason = "line_too_long";
            next = None;
            break;
        }
        if n < start {
            continue;
        }
        let text = match std::str::from_utf8(&row) {
            Ok(v) => v.trim_end(),
            Err(e) => return ToolResult::error("invalid_utf8", e.to_string()),
        };
        if text.to_lowercase().contains(&query) {
            let folded = text.to_lowercase();
            let match_byte = folded.find(&query).unwrap_or(0);
            let mut folded_bytes = 0;
            let at = text
                .chars()
                .take_while(|ch| {
                    let before = folded_bytes;
                    folded_bytes += ch.to_lowercase().map(char::len_utf8).sum::<usize>();
                    before < match_byte
                })
                .count();
            let chars = text.chars().collect::<Vec<_>>();
            if results.len() >= limit {
                next = Some(n);
                reason = "match_limit";
                break;
            }
            let mut snippet = chars
                .iter()
                .skip(at.saturating_sub(120))
                .take(500)
                .collect::<String>();
            crate::secrets::redact_tool_output(&c.config, &mut snippet);
            if !results.is_empty()
                && used + snippet.len() > c.config.tools.max_result_chars.min(4096)
            {
                next = Some(n);
                reason = "result_budget";
                break;
            }
            used += snippet.len();
            results.push(serde_json::json!({"line":n,"text":snippet}));
        }
    }
    let truncated = reason != "complete";
    let mut r=ToolResult::ok(serde_json::json!({"file":"INDEX.md","matches":results,"truncated":truncated,"stop_reason":reason,"next_start_line":next,
        "continuation":next.map(|n|serde_json::json!({"query":q,"start_line":n,"max_results":limit})),"note":"Searches INDEX.md only; use memory.read for topic contents."}).to_string());
    r.truncated = truncated;
    r
}

#[cfg(unix)]
mod edit;
#[cfg(unix)]
pub(crate) use edit::write;
#[cfg(not(unix))]
pub(crate) fn write(_a: &Args, _c: &ToolContext) -> ToolResult {
    ToolResult::error(
        "unsupported_platform",
        "Atomic Memory writes are currently supported on Linux and macOS",
    )
}
