#![cfg(unix)]
use colibri_rust::{
    config::AgentConfig,
    tools::{run_tool, ToolContext},
};
use serde_json::{json, Value};
use std::{fs, path::PathBuf};
struct Home(PathBuf);
impl Home {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "colibri-efficiency-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn context(&self) -> ToolContext {
        let mut c = AgentConfig::default();
        c.model.api_key = "test-secret-cross-boundary".into();
        ToolContext::new(c, self.0.clone()).with_runtime_home(self.0.clone())
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn run(name: &str, args: Value, c: &ToolContext) -> colibri_rust::messages::ToolResult {
    run_tool(name, &args.to_string(), c).unwrap()
}
#[test]
fn long_output_is_small_recoverable_and_redacted_before_persistence() {
    let h = Home::new();
    let c = h.context();
    // More than one pipe chunk; secret deliberately starts close to a read boundary.
    let r = run(
        "shell.run",
        json!({"command":"printf '%08190d' 0; printf test-secret-cross-boundary; printf '中间证据'; printf '%010000d' 0; exit 7"}),
        &c,
    );
    assert!(!r.ok);
    assert!(r.truncated);
    assert!(r.text.chars().count() < 5500);
    assert!(r.text.contains("exit_code=7"));
    assert!(!r.text.contains("test-secret-cross-boundary"));
    let paths = fs::read_dir(h.0.join("artifacts/tool-results"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "txt"))
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 1);
    let full = fs::read_to_string(&paths[0]).unwrap();
    assert!(full.contains("<redacted>中间证据"));
    assert!(!full.contains("test-secret-cross-boundary"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(&paths[0]).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let page = run(
        "files.read",
        json!({"path":paths[0],"offset_chars":8180,"max_chars":70}),
        &c,
    );
    let page: Value = serde_json::from_str(&page.text).unwrap();
    assert!(page["text"].as_str().unwrap().contains("中间证据"));
    assert_eq!(page["next_offset_chars"], 8250);
}
#[test]
fn small_output_does_not_write_and_archive_failure_keeps_command_status() {
    let h = Home::new();
    let c = h.context();
    let r = run("shell.run", json!({"command":"printf small"}), &c);
    assert!(r.ok);
    assert!(!h.0.join("artifacts").exists());
    fs::write(h.0.join("artifacts"), "blocked").unwrap();
    let r = run("shell.run", json!({"command":"printf '%012000d' 0"}), &c);
    assert!(r.ok);
    assert!(r.truncated);
    assert!(r.text.contains("archive unavailable"));
    assert!(!r.text.contains("archive="));
}
#[test]
fn validation_is_checked_before_edit_and_failure_preserves_applied_state() {
    let h = Home::new();
    let c = h.context();
    let p = h.0.join("file.txt");
    fs::write(&p, "before").unwrap();
    let bad = run(
        "files.patch",
        json!({"path":p,"old_text":"before","new_text":"after","verify_command":"true","verify_timeout_seconds":-1}),
        &c,
    );
    assert!(!bad.ok);
    assert_eq!(fs::read_to_string(&p).unwrap(), "before");
    let r = run(
        "files.patch",
        json!({"path":p,"old_text":"before","new_text":"after","verify_command":"printf check-failed; exit 9"}),
        &c,
    );
    assert!(!r.ok);
    assert_eq!(fs::read_to_string(&p).unwrap(), "after");
    let v: Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["edit"], "applied");
    assert_eq!(v["verification"]["status"], "failed");
    assert!(v["verification"]["output"]
        .as_str()
        .unwrap()
        .contains("exit_code=9"));
    let r = run(
        "files.write",
        json!({"path":p,"content":"last","verify_command":"test \"$(cat file.txt)\" = last"}),
        &c,
    );
    assert!(r.ok);
    let v: Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["verification"]["status"], "passed");
}
#[test]
fn timeout_retains_output_and_edit_result() {
    let h = Home::new();
    let c = h.context();
    let r = run(
        "files.write",
        json!({"path":"timeout.txt","content":"applied","verify_command":"printf started; sleep 5","verify_timeout_seconds":0.03}),
        &c,
    );
    assert!(!r.ok);
    assert_eq!(r.error_type.as_deref(), Some("timeout"));
    let v: Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["edit"], "applied");
    assert_eq!(v["verification"]["status"], "timeout");
    assert!(v["verification"]["output"]
        .as_str()
        .unwrap()
        .contains("started"));
}

#[test]
fn full_archive_budget_degrades_without_deleting_existing_evidence() {
    let h = Home::new();
    let c = h.context();
    let dir = h.0.join("artifacts/tool-results");
    fs::create_dir_all(&dir).unwrap();
    let old = dir.join("protected.txt");
    let f = fs::File::create(&old).unwrap();
    f.set_len(64 * 1024 * 1024).unwrap();
    let r = run("shell.run", json!({"command":"printf '%012000d' 0"}), &c);
    assert!(r.ok);
    assert!(r.text.contains("archive unavailable"));
    assert_eq!(fs::metadata(old).unwrap().len(), 64 * 1024 * 1024);
}

#[test]
fn restored_model_history_preserves_exact_compact_tool_view() {
    use colibri_rust::{
        model::FakeModel, session::AgentSession, session_history::TranscriptHistoryLoader,
    };
    let h = Home::new();
    let mut config = AgentConfig::default();
    config.memory.enabled = false;
    use std::sync::{Arc, Mutex};
    let writer = colibri_rust::transcript::TranscriptWriter::for_session(&h.0, &config.session)
        .unwrap()
        .map(|w| Arc::new(Mutex::new(w)));
    let mut session = AgentSession::from_shared(
        Arc::new(config),
        Arc::new(Mutex::new(Box::new(FakeModel::new()))),
        writer,
        Default::default(),
    )
    .with_runtime_home(h.0.clone())
    .with_working_directory(h.0.clone());
    session
        .submit(&format!(
            "tool:shell.run {}",
            json!({"command":"printf '%012000d' 0"})
        ))
        .unwrap();
    let before = session
        .messages
        .iter()
        .find(|m| m.role == "tool")
        .unwrap()
        .content
        .clone();
    drop(session);
    let restored = TranscriptHistoryLoader::for_home(h.0.clone()).load_model("local");
    let after = restored.messages.iter().find(|m| m.role == "tool").unwrap();
    assert_eq!(before, after.content);
    assert!(after.content.contains("archive="));
    assert!(after.content.chars().count() < 5500);
}

#[test]
fn sorted_directory_pages_cover_all_entries_once() {
    let h = Home::new();
    let c = h.context();
    for i in 0..123 {
        fs::write(h.0.join(format!("文件{i:03}.txt")), "").unwrap();
    }
    let mut after = String::new();
    let mut all = Vec::new();
    loop {
        let r = run(
            "files.list",
            json!({"path":".","limit":17,"after":after}),
            &c,
        );
        assert!(r.ok, "{}", r.text);
        let v: Value = serde_json::from_str(&r.text).unwrap();
        let entries = v["entries"].as_array().unwrap();
        assert!(entries.len() <= 17);
        all.extend(entries.iter().map(|v| v.as_str().unwrap().to_string()));
        if v["has_more"] == false {
            break;
        }
        after = v["next_after"].as_str().unwrap().to_string();
        assert!(all.len() <= 123);
    }
    assert_eq!(all.len(), 123);
    let mut sorted = all.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(all, sorted);
}

#[test]
fn normal_file_character_pages_preserve_chinese_and_redact_before_slicing() {
    let h = Home::new();
    let c = h.context();
    let original = "第一行\n前缀test-secret-cross-boundary中文单行内容结尾";
    fs::write(h.0.join("text"), original).unwrap();
    let mut offset = 0;
    let mut combined = String::new();
    loop {
        let r = run(
            "files.read",
            json!({"path":"text","offset_chars":offset,"max_chars":5}),
            &c,
        );
        assert!(r.ok, "{}", r.text);
        let page: Value = serde_json::from_str(&r.text).unwrap();
        combined += page["text"].as_str().unwrap();
        let next = page["next_offset_chars"].as_u64().unwrap();
        if page["has_more"] == false {
            break;
        }
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(
        combined,
        original.replace("test-secret-cross-boundary", "<redacted>")
    );
    let r = run(
        "files.read",
        json!({"path":"text","start_line":2,"max_chars":4}),
        &c,
    );
    let page: Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(page["next_offset_chars"], 8);
}

#[test]
fn search_returns_match_neighborhood_and_resumes_same_file_without_duplicates() {
    let h = Home::new();
    let c = h.context();
    fs::write(
        h.0.join("text"),
        format!("{}needle结尾\nneedle二\nneedle三\n", "前".repeat(1000)),
    )
    .unwrap();
    let first = run(
        "files.search",
        json!({"path":"text","query":"needle","max_results":2}),
        &c,
    );
    let v: Value = serde_json::from_str(&first.text).unwrap();
    assert_eq!(v["stop_reason"], "match_limit");
    assert!(v["matches"][0]["text"].as_str().unwrap().contains("needle"));
    let more = run(
        "files.search",
        json!({"path":"text","query":"needle","after_path":v["continuation"]["after_path"],"start_line":v["continuation"]["start_line"]}),
        &c,
    );
    let v: Value = serde_json::from_str(&more.text).unwrap();
    assert_eq!(v["matches"].as_array().unwrap().len(), 1);
    assert_eq!(v["matches"][0]["line"], 3);
    assert_eq!(v["stop_reason"], "complete");
}

#[test]
fn background_poll_distinguishes_page_limit_from_capture_truncation() {
    let h = Home::new();
    let c = h.context();
    let id = "0123456789abcdef01234567";
    let dir = h.0.join("jobs").join(id);
    fs::create_dir_all(&dir).unwrap();
    let output = "中".repeat(5000);
    fs::write(dir.join("output.log"), &output).unwrap();
    fs::write(dir.join("state.json"),json!({"job_id":id,"status":"completed","pid":0,"identity":"","exit_code":0,"truncated":false,"bytes":output.len(),"created":0,"label":"fixture","error":null}).to_string()).unwrap();
    let mut offset = 0;
    let mut combined = String::new();
    loop {
        let r = run("shell.poll", json!({"job_id":id,"offset":offset}), &c);
        assert!(r.ok, "{}", r.text);
        let v: Value = serde_json::from_str(&r.text).unwrap();
        assert_eq!(v["truncated"], false);
        assert_eq!(v["exit_code"], 0);
        combined += v["output"].as_str().unwrap();
        if v["page_has_more"] == false {
            break;
        }
        let next = v["next_offset"].as_u64().unwrap();
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(combined, output);
}

#[test]
fn polling_terminal_invalid_utf8_reports_error_instead_of_nonadvancing_page() {
    let h = Home::new();
    let c = h.context();
    let id = "0123456789abcdef01234567";
    let dir = h.0.join("jobs").join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("output.log"), [0xe4u8]).unwrap();
    fs::write(dir.join("state.json"),json!({"job_id":id,"status":"completed","pid":0,"identity":"","exit_code":0,"truncated":false,"bytes":1,"created":0,"label":"fixture","error":null}).to_string()).unwrap();
    let r = run("shell.poll", json!({"job_id":id}), &c);
    assert!(!r.ok);
    assert!(r.text.contains("incomplete UTF-8"));
}
