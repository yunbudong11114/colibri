//! Lazy, bounded command-output storage. Known secrets are removed before disk writes.
use crate::tools::ToolContext;
use std::{
    fs::{self, File},
    io::{self, Write},
    path::PathBuf,
};
const STREAM_LIMIT: u64 = 16 * 1024 * 1024;
const DIRECTORY_LIMIT: u64 = 64 * 1024 * 1024;

pub(crate) struct Archive {
    root: PathBuf,
    limit: usize,
    buffer: Vec<u8>,
    redactor: crate::secrets::StreamRedactor,
    file: Option<(File, PathBuf)>,
    written: u64,
    failed: bool,
}
impl Archive {
    pub(crate) fn new(context: &ToolContext, limit: usize) -> Self {
        Self {
            root: context.runtime_home.join("artifacts/tool-results"),
            limit,
            buffer: Vec::new(),
            redactor: crate::secrets::StreamRedactor::new(crate::secrets::values(&context.config)),
            file: None,
            written: 0,
            failed: false,
        }
    }
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        let safe = self.redactor.feed(bytes, false);
        self.store(&safe);
        safe
    }
    pub(crate) fn drain(&mut self, final_chunk: bool) -> Vec<u8> {
        let safe = self.redactor.feed(&[], final_chunk);
        self.store(&safe);
        safe
    }
    fn store(&mut self, bytes: &[u8]) {
        if self.failed {
            return;
        }
        if self.file.is_none() {
            if self.buffer.len().saturating_add(bytes.len()) <= self.limit {
                self.buffer.extend_from_slice(bytes);
                return;
            }
            match self.open() {
                Ok(file) => self.file = Some(file),
                Err(_) => {
                    self.failed = true;
                    self.buffer.clear();
                    return;
                }
            }
            let buffered = std::mem::take(&mut self.buffer);
            self.store(&buffered);
        }
        if self.failed {
            return;
        }
        let available = STREAM_LIMIT.saturating_sub(self.written) as usize;
        let part = &bytes[..bytes.len().min(available)];
        if let Some((f, _)) = &mut self.file {
            if f.write_all(part).is_err() {
                if let Some((_, path)) = self.file.take() {
                    let _ = fs::remove_file(path);
                }
                self.failed = true;
                return;
            }
        }
        self.written += part.len() as u64;
        if bytes.len() > available {
            self.failed = true;
        }
    }
    #[cfg(unix)]
    fn open(&self) -> io::Result<(File, PathBuf)> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.root)?;
        fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))?;
        let lock_path = self.root.join(".budget.lock");
        let lock = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(lock_path)?;
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let used: u64 = fs::read_dir(&self.root)?
            .filter_map(Result::ok)
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum();
        if used.saturating_add(STREAM_LIMIT) > DIRECTORY_LIMIT {
            return Err(io::Error::other("archive budget exhausted"));
        }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let p = self.root.join(format!(
            "{}-{stamp}-{}.txt",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let f = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&p)?;
        // Reserve quota with a sparse extent; shrink to actual bytes when the stream closes.
        if let Err(e) = f.set_len(STREAM_LIMIT) {
            let _ = fs::remove_file(&p);
            return Err(e);
        }
        Ok((f, p))
    }
    #[cfg(not(unix))]
    fn open(&self) -> io::Result<(File, PathBuf)> {
        Err(io::Error::other("archive unavailable"))
    }
    pub(crate) fn finish(self) -> String {
        self.finish_receipt().note()
    }
    pub(crate) fn finish_receipt(mut self) -> Receipt {
        self.drain(true);
        if let Some((f, p)) = self.file.take() {
            if f.set_len(self.written).and_then(|_| f.sync_all()).is_err() {
                let _ = fs::remove_file(p);
                return Receipt {
                    path: None,
                    bytes: 0,
                    complete: false,
                    error: Some("archive write failed".into()),
                };
            }
            Receipt {
                path: Some(p),
                bytes: self.written,
                complete: !self.failed,
                error: None,
            }
        } else {
            Receipt {
                path: None,
                bytes: 0,
                complete: !self.failed,
                error: self.failed.then(|| "storage budget or I/O failure".into()),
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn stream_boundaries_are_redacted_and_overflow_is_explicit() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("colibri-archive-{}-{stamp}", std::process::id()));
        let mut config = crate::config::AgentConfig::default();
        config.model.api_key = "boundary-secret".into();
        let context = ToolContext::new(config, root.clone()).with_runtime_home(root.clone());
        let mut archive = Archive::new(&context, 4);
        let mut visible = archive.feed(b"prefix boundary-");
        visible.extend(archive.feed(b"secret suffix"));
        visible.extend(archive.drain(true));
        let note = archive.finish();
        assert!(note.contains("complete=true"));
        assert_eq!(
            String::from_utf8(visible).unwrap(),
            "prefix <redacted> suffix"
        );
        let paths = fs::read_dir(root.join("artifacts/tool-results"))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|v| v == "txt"))
            .collect::<Vec<_>>();
        assert_eq!(
            fs::read_to_string(&paths[0]).unwrap(),
            "prefix <redacted> suffix"
        );
        let mut archive = Archive::new(&context, 4);
        let chunk = vec![b'x'; 32768];
        for _ in 0..513 {
            archive.feed(&chunk);
        }
        assert!(archive.finish().contains("complete=false"));
        assert!(fs::read_dir(root.join("artifacts/tool-results"))
            .unwrap()
            .flatten()
            .any(|e| e.metadata().unwrap().len() == STREAM_LIMIT));
        fs::remove_dir_all(root).unwrap();
    }
}

impl Drop for Archive {
    fn drop(&mut self) {
        // Reader failure or unwinding must not leave an unreferenced quota reservation.
        if let Some((_, path)) = self.file.take() {
            let _ = fs::remove_file(path);
        }
    }
}

#[derive(serde::Serialize)]
pub(crate) struct Receipt {
    pub path: Option<PathBuf>,
    pub bytes: u64,
    pub complete: bool,
    pub error: Option<String>,
}
impl Receipt {
    fn note(&self) -> String {
        if let Some(p) = &self.path {
            format!("archive={} bytes={} complete={}. Read with files.read start_line/end_line or offset_chars/max_chars; search with files.search.",p.display(),self.bytes,self.complete)
        } else if self.error.is_some() {
            "archive unavailable: storage budget or I/O failure; only preview retained".into()
        } else {
            String::new()
        }
    }
}
/// Archive before projection. The envelope stays valid JSON even for structured results.
pub(crate) fn project(
    value: &serde_json::Value,
    kind: &str,
    context: &ToolContext,
) -> (String, bool) {
    let mut raw = value.to_string();
    crate::secrets::redact_tool_output(&context.config, &mut raw);
    let cap = context.config.tools.max_result_chars.min(4096);
    if raw.chars().count() <= cap {
        return (raw, false);
    }
    let mut a = Archive::new(context, 0);
    for chunk in raw.as_bytes().chunks(8192) {
        a.feed(chunk);
    }
    let receipt = a.finish_receipt();
    let preview = raw.chars().take(cap.min(1024)).collect::<String>();
    (serde_json::json!({"kind":kind,"preview":preview,"truncated":true,"archive":receipt,
        "read_hint":"Use files.read offset_chars/max_chars to inspect the saved JSON; do not rerun the original operation."}).to_string(),true)
}

/// Preserve protocol metadata and non-text blocks instead of pretending they were understood.
pub(crate) fn project_mcp(result: &serde_json::Value, context: &ToolContext) -> (String, bool) {
    use serde_json::{json, Value};
    let parts = result["content"].as_array();
    let non_text = parts.is_some_and(|p| p.iter().any(|v| v["type"] != "text"));
    let value = if !result["structuredContent"].is_null() {
        result["structuredContent"].clone()
    } else {
        json!(parts
            .into_iter()
            .flatten()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>())
    };
    let raw = value.to_string();
    if !non_text && raw.chars().count() <= context.config.tools.max_result_chars.min(4096) {
        let mut raw = raw;
        crate::secrets::redact_tool_output(&context.config, &mut raw);
        return (raw, false);
    }
    let (text, large) = project(result, "mcp_result", context);
    let mut envelope: Value =
        serde_json::from_str(&text).unwrap_or_else(|_| json!({"incomplete":true}));
    if !envelope.is_object() {
        envelope = json!({"result":envelope});
    }
    envelope["isError"] = json!(result["isError"] == true);
    envelope["has_structured_content"] = json!(!result["structuredContent"].is_null());
    envelope["content_types"] = json!(parts
        .into_iter()
        .flatten()
        .take(32)
        .filter_map(|p| p["type"].as_str())
        .collect::<Vec<_>>());
    if non_text {
        envelope["non_text_note"]=json!("Typed blocks are preserved in the result/archive; their presence does not mean image/audio/resource content was interpreted.");
    }
    (envelope.to_string(), large)
}

#[cfg(all(test, unix))]
mod projection_tests {
    use super::*;
    #[test]
    fn mcp_projection_preserves_error_types_and_json_without_secrets() {
        let scratch = crate::config_io::Scratch::new(&std::env::temp_dir()).unwrap();
        let mut config = crate::config::AgentConfig::default();
        config.model.api_key = "projection-test-secret".into();
        let context =
            ToolContext::new(config, scratch.0.clone()).with_runtime_home(scratch.0.clone());
        let result = serde_json::json!({"isError":true,"structuredContent":{"rows":"row ".repeat(5000),"token":"projection-test-secret"},"content":[{"type":"text","text":"failed"},{"type":"image","mimeType":"image/png","data":"abc"}]});
        let (text, truncated) = project_mcp(&result, &context);
        assert!(truncated);
        assert!(text.len() < 6000);
        assert!(!text.contains("projection-test-secret"));
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["isError"], true);
        assert_eq!(v["has_structured_content"], true);
        assert_eq!(v["content_types"], serde_json::json!(["text", "image"]));
        let full = fs::read_to_string(v["archive"]["path"].as_str().unwrap()).unwrap();
        let original: serde_json::Value = serde_json::from_str(&full).unwrap();
        assert_eq!(original["structuredContent"]["token"], "<redacted>");
        assert_eq!(original["content"][1]["type"], "image");
        fs::remove_dir_all(scratch.0.join("artifacts")).unwrap();
        fs::write(scratch.0.join("artifacts"), "blocked").unwrap();
        let (text, truncated) = project_mcp(&result, &context);
        assert!(truncated);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(v["archive"]["path"].is_null());
        assert_eq!(v["archive"]["complete"], false);
        assert_eq!(v["isError"], true);
        let (text, truncated) = project_mcp(
            &serde_json::json!({"content":[{"type":"resource_link","uri":"test:resource","name":"example"}]}),
            &context,
        );
        assert!(!truncated);
        assert!(text.contains("resource_link"));
    }
}
