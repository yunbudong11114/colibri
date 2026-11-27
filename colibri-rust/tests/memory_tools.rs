#![cfg(unix)]
use colibri_rust::{
    config::AgentConfig,
    tools::{run_tool, ToolContext},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Barrier},
};
struct Home(PathBuf);
impl Home {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "colibri-memory-tools-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn context(&self) -> ToolContext {
        let mut config = AgentConfig::default();
        config.memory.root = self.0.join("memory");
        config.model.api_key = "memory-test-secret".into();
        ToolContext::new(config, self.0.clone()).with_runtime_home(self.0.clone())
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn call(c: &ToolContext, name: &str, args: Value) -> colibri_rust::messages::ToolResult {
    run_tool(name, &args.to_string(), c).unwrap()
}
#[test]
fn read_keeps_small_results_and_pages_large_redacted_memory() {
    let h = Home::new();
    let c = h.context();
    fs::create_dir(&c.config.memory.root).unwrap();
    let path = c.config.memory.root.join("MEMORY.md");
    fs::write(&path, "短记忆").unwrap();
    assert_eq!(
        call(&c, "memory.read", json!({"file":"MEMORY.md"})).text,
        "短记忆"
    );
    let original = "中文memory-test-secret长句".repeat(30);
    fs::write(&path, &original).unwrap();
    let mut offset = 0;
    let mut text = String::new();
    loop {
        let r = call(
            &c,
            "memory.read",
            json!({"file":"MEMORY.md","offset_chars":offset,"max_chars":11}),
        );
        assert!(r.ok, "{}", r.text);
        let v: Value = serde_json::from_str(&r.text).unwrap();
        text += v["text"].as_str().unwrap();
        if v["has_more"] == false {
            break;
        }
        assert_eq!(v["continuation"]["file"], "MEMORY.md");
        assert!(v["continuation"].get("path").is_none());
        let next = v["next_offset_chars"].as_u64().unwrap();
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(text, original.replace("memory-test-secret", "<redacted>"));
    fs::write(&path, r#"{"text":"plain memory","next_offset_chars":999}"#).unwrap();
    let r = call(
        &c,
        "memory.read",
        json!({"file":"MEMORY.md","include_version":true}),
    );
    let v: Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["text"], fs::read_to_string(path).unwrap());
}
#[test]
fn versions_reject_stale_writes_without_changing_memory() {
    let h = Home::new();
    let c = h.context();
    assert!(
        call(
            &c,
            "memory.write",
            json!({"file":"MEMORY.md","content":"first","expected_sha256":"missing"})
        )
        .ok
    );
    let read = call(
        &c,
        "memory.read",
        json!({"file":"MEMORY.md","include_version":true}),
    );
    let v: Value = serde_json::from_str(&read.text).unwrap();
    assert!(call(&c,"memory.write",json!({"file":"MEMORY.md","content":"second","mode":"replace","expected_sha256":v["sha256"]})).ok);
    let stale = call(
        &c,
        "memory.write",
        json!({"file":"MEMORY.md","content":"wrong","expected_sha256":v["sha256"]}),
    );
    assert!(!stale.ok);
    assert_eq!(stale.error_type.as_deref(), Some("stale_file"));
    assert_eq!(
        fs::read_to_string(c.config.memory.root.join("MEMORY.md")).unwrap(),
        "second\n"
    );
}
#[test]
fn concurrent_appends_do_not_lose_or_interleave_records() {
    let h = Home::new();
    let c = h.context();
    let start = Arc::new(Barrier::new(8));
    let mut handles = Vec::new();
    for thread in 0..8 {
        let c = c.clone();
        let start = start.clone();
        handles.push(std::thread::spawn(move || {
            start.wait();
            for i in 0..4 {
                let r = call(
                    &c,
                    "memory.write",
                    json!({"file":"MEMORY.md","content":format!("record-{thread}-{i}")}),
                );
                assert!(r.ok, "{}", r.text);
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    let text = fs::read_to_string(c.config.memory.root.join("MEMORY.md")).unwrap();
    let mut lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 32);
    lines.sort();
    lines.dedup();
    assert_eq!(lines.len(), 32);
}
#[test]
fn index_errors_are_distinct_and_search_uses_lookahead() {
    let h = Home::new();
    let c = h.context();
    let missing = call(&c, "memory.search", json!({"query":"needle"}));
    assert!(!missing.ok);
    assert_eq!(missing.error_type.as_deref(), Some("not_found"));
    fs::create_dir(&c.config.memory.root).unwrap();
    fs::write(
        c.config.memory.root.join("INDEX.md"),
        format!("{}needle一\nneedle二\nneedle三\n", "前".repeat(1000)),
    )
    .unwrap();
    let r = call(
        &c,
        "memory.search",
        json!({"query":"needle","max_results":2}),
    );
    let v: Value = serde_json::from_str(&r.text).unwrap();
    assert!(r.truncated);
    assert!(v["matches"][0]["text"].as_str().unwrap().contains("needle"));
    assert_eq!(v["next_start_line"], 3);
    let r = call(&c, "memory.search", v["continuation"].clone());
    let v: Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["matches"].as_array().unwrap().len(), 1);
    assert!(!r.truncated);
    let r = call(
        &c,
        "memory.search",
        json!({"query":"needle","max_results":3}),
    );
    assert!(!r.truncated);
    let r = call(&c, "memory.search", json!({"query":"absent"}));
    assert!(r.ok);
    assert_eq!(
        serde_json::from_str::<Value>(&r.text).unwrap()["matches"],
        json!([])
    );
    fs::remove_file(c.config.memory.root.join("INDEX.md")).unwrap();
    fs::create_dir(c.config.memory.root.join("INDEX.md")).unwrap();
    assert!(!call(&c, "memory.search", json!({"query":"needle"})).ok);
}
#[test]
fn topic_and_index_update_together_and_preparation_failure_applies_neither() {
    let h = Home::new();
    let c = h.context();
    let r = call(
        &c,
        "memory.write",
        json!({"topic":"nas","content":"details","index_description":"NAS 同步 备份","expected_sha256":"missing","expected_index_sha256":"missing"}),
    );
    assert!(r.ok, "{}", r.text);
    let v: Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["index_updated"], true);
    let r = call(
        &c,
        "memory.write",
        json!({"topic":"nas","content":"updated","mode":"replace","index_description":"NAS 更新"}),
    );
    assert!(r.ok);
    let index = fs::read_to_string(c.config.memory.root.join("INDEX.md")).unwrap();
    assert_eq!(index.matches("topics/nas.md").count(), 1);
    assert!(index.contains("NAS 更新"));
    let path = c.config.memory.root.join("topics/nas.md");
    let before = fs::read(&path).unwrap();
    fs::write(c.config.memory.root.join("INDEX.md"), "x".repeat(65537)).unwrap();
    let failed = call(
        &c,
        "memory.write",
        json!({"topic":"nas","content":"must-not-apply","index_description":"valid"}),
    );
    assert!(!failed.ok);
    assert_eq!(fs::read(path).unwrap(), before);
}
#[test]
fn topic_list_pages_preserve_builtin_order_and_cover_all_topics() {
    let h = Home::new();
    let c = h.context();
    fs::create_dir_all(c.config.memory.root.join("topics")).unwrap();
    for name in ["SOUL.md", "USER.md", "MEMORY.md", "INDEX.md"] {
        fs::write(c.config.memory.root.join(name), "").unwrap();
    }
    for i in 0..80 {
        fs::write(
            c.config.memory.root.join(format!("topics/topic{i:03}.md")),
            "",
        )
        .unwrap();
    }
    let mut after = None;
    let mut names = Vec::new();
    loop {
        let mut args = json!({"limit":9});
        if let Some(v) = &after {
            args["after"] = json!(v);
        }
        let r = call(&c, "memory.list", args);
        assert!(r.ok);
        let v: Value = serde_json::from_str(&r.text).unwrap();
        names.extend(
            v["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string()),
        );
        if v["has_more"] == false {
            break;
        }
        after = Some(v["next_after"].as_str().unwrap().to_string());
        assert!(names.len() <= 84);
    }
    assert_eq!(
        &names[..4],
        &["SOUL.md", "USER.md", "MEMORY.md", "INDEX.md"]
    );
    assert_eq!(names.len(), 84);
    names.sort();
    names.dedup();
    assert_eq!(names.len(), 84);
}
