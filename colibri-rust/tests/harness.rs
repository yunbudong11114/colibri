#![cfg(unix)]
use colibri_rust::{
    config::AgentConfig,
    messages::{Message, ModelLimits, ModelResponse, ToolCall, ToolResult},
    model::ModelClient,
    session::AgentSession,
    tools::{run_tool, ToolContext},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p =
            std::env::temp_dir().join(format!("colibri-harness-{}-{sequence}", std::process::id()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn context(&self) -> ToolContext {
        let mut cfg = AgentConfig::default();
        cfg.model.api_key = "test-secret-token".into();
        ToolContext::new(cfg, self.0.clone())
            .with_runtime_home(self.0.join("runtime"))
            .with_execution_host(env!("CARGO_BIN_EXE_colibri").into())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn call(c: &ToolContext, name: &str, args: Value) -> ToolResult {
    run_tool(name, &args.to_string(), c).unwrap()
}
fn job(c: &ToolContext, command: &str, timeout: f64) -> String {
    let r = call(
        c,
        "shell.run",
        json!({"command":command,"background":true,"timeout_seconds":timeout}),
    );
    assert!(r.ok, "{}", r.text);
    serde_json::from_str::<Value>(&r.text).unwrap()["job_id"]
        .as_str()
        .unwrap()
        .into()
}
fn until_done(c: &ToolContext, id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let r = call(c, "shell.poll", json!({"job_id":id,"wait_ms":200}));
        assert!(r.ok, "{}", r.text);
        let v: Value = serde_json::from_str(&r.text).unwrap();
        if v["status"] != "running" {
            if v["status"] == "interrupted" {
                eprintln!("Unexpected interrupted job: {}", v);
            }
            return v;
        }
        assert!(Instant::now() < deadline, "job stuck");
    }
}
#[test]
fn job_survives_new_context_and_redacts_split_output() {
    let f = Fixture::new();
    let c = f.context();
    let start = Instant::now();
    let id = job(
        &c,
        "printf 'test-secret-'; sleep 0.3; printf 'token 中文'; sleep 0.5; printf ' done'",
        5.,
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    let v = until_done(&f.context(), &id);
    assert_eq!(v["status"], "completed");
    assert_eq!(v["exit_code"], 0);
    assert_eq!(v["output"], "<redacted> 中文 done");
    for e in fs::read_dir(c.runtime_home.join("jobs").join(id))
        .unwrap()
        .flatten()
    {
        if e.path().is_file() {
            assert!(!fs::read_to_string(e.path())
                .unwrap()
                .contains("test-secret-token"));
        }
    }
}
#[test]
fn job_cancel_timeout_and_active_limit() {
    let f = Fixture::new();
    let c = f.context();
    let id = job(&c, "sleep 30 & wait", 40.);
    let denied = call(&c, "shell.run", json!({"command":"true","background":true}));
    assert!(!denied.ok);
    assert!(call(&c, "shell.cancel", json!({"job_id":id})).ok);
    assert_eq!(until_done(&c, &id)["status"], "killed");
    let id = job(&c, "sleep 30", 0.2);
    assert_eq!(until_done(&c, &id)["status"], "timed_out");
}
#[test]
fn job_output_is_bounded_with_tail() {
    let f = Fixture::new();
    let c = f.context();
    let id = job(
        &c,
        "head -c 1200000 /dev/zero | tr '\\000' x; printf FINAL",
        10.,
    );
    let v = until_done(&c, &id);
    assert_eq!(v["status"], "completed");
    assert_eq!(v["truncated"], true);
    let tail = call(&c, "shell.poll", json!({"job_id":id,"tail":true}));
    let v: Value = serde_json::from_str(&tail.text).unwrap();
    assert!(v["output"].as_str().unwrap().ends_with("FINAL"));
    assert!(
        fs::metadata(c.runtime_home.join("jobs").join(id).join("output.log"))
            .unwrap()
            .len()
            <= 1048576
    );
}
#[test]
fn streaming_range_search_and_stale_patch() {
    let f = Fixture::new();
    let c = f.context();
    fs::write(
        f.0.join("log"),
        format!("{}\nneedle\n", "a".repeat(8 * 1024 * 1024)),
    )
    .unwrap();
    let r = call(
        &c,
        "files.read",
        json!({"path":"log","start_line":2,"end_line":2,"max_chars":20}),
    );
    assert!(r.ok);
    assert_eq!(r.text, "needle\n");
    let r = call(&c, "files.read", json!({"path":"log","max_chars":20}));
    assert!(r.truncated);
    assert_eq!(r.text.chars().count(), 20);
    fs::write(f.0.join("data"), "one\nneedle\nneedle\n").unwrap();
    let r = call(
        &c,
        "files.search",
        json!({"path":"data","query":"needle","max_results":1}),
    );
    let v: Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["matches"][0]["line"], 2);
    assert_eq!(v["truncated"], true);
    assert!(
        !call(
            &c,
            "files.patch",
            json!({"path":"data","old_text":"needle","new_text":"other"})
        )
        .ok
    );
    assert!(
        !call(
            &c,
            "files.patch",
            json!({"path":"data","old_text":"one","new_text":"two","expected_sha256":"stale"})
        )
        .ok
    );
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(f.0.join("data"), fs::Permissions::from_mode(0o640)).unwrap();
    assert!(
        call(
            &c,
            "files.patch",
            json!({"path":"data","old_text":"one","new_text":"two"})
        )
        .ok
    );
    assert_eq!(
        fs::read_to_string(f.0.join("data")).unwrap(),
        "two\nneedle\nneedle\n"
    );
    assert_eq!(
        fs::metadata(f.0.join("data")).unwrap().permissions().mode() & 0o777,
        0o640
    );
}
#[test]
fn tasks_restore_isolate_and_reject_fake_completion() {
    let f = Fixture::new();
    let c = f.context();
    assert!(
        call(
            &c,
            "task.start",
            json!({"objective":"verify","steps":["check"],"acceptance":["file exists"]})
        )
        .ok
    );
    assert!(
        !call(
            &c,
            "task.update",
            json!({"step":1,"status":"completed","evidence_ids":["invented"]})
        )
        .ok
    );
    assert!(
        !call(
            &c,
            "task.start",
            json!({"objective":"other","steps":["check"],"acceptance":["ok"]})
        )
        .ok
    );
    let restored = call(&f.context(), "task.inspect", json!({}));
    assert!(restored.text.contains("verify"));
    let isolated = call(
        &f.context().with_scope("other".into()),
        "task.inspect",
        json!({}),
    );
    assert_eq!(isolated.text, "null");
    assert!(call(&c, "task.update", json!({"blocker":"network unavailable"})).ok);
    assert!(call(&c, "task.inspect", json!({})).text.contains("blocked"));
}
struct PlannedModel(usize);
impl ModelClient for PlannedModel {
    fn complete(
        &mut self,
        _: &[Message],
        _: &[Value],
        _: &str,
        _: &ModelLimits,
    ) -> Result<ModelResponse, String> {
        self.0 += 1;
        let (name, args) = match self.0 {
            1 => (
                "task.start",
                json!({"objective":"verify file","steps":["read file"],"acceptance":["read succeeds"]}),
            ),
            2 => ("files.read", json!({"path":"data"})),
            3 => (
                "task.update",
                json!({"step":1,"status":"completed","evidence_ids":["ev-1"]}),
            ),
            _ => {
                return Ok(ModelResponse {
                    text: "verified".into(),
                    ..Default::default()
                })
            }
        };
        Ok(ModelResponse {
            tool_calls: vec![ToolCall {
                id: self.0.to_string(),
                name: name.into(),
                arguments: args
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            }],
            ..Default::default()
        })
    }
}
#[test]
fn session_records_real_evidence_and_completes() {
    let f = Fixture::new();
    fs::write(f.0.join("data"), "ok").unwrap();
    let mut cfg = AgentConfig::default();
    cfg.memory.enabled = false;
    cfg.skills.dir = f.0.join("skills");
    cfg.session.transcript = false;
    let mut session = AgentSession::new(cfg, Box::new(PlannedModel(0)))
        .with_runtime_home(f.0.join("runtime"))
        .with_working_directory(f.0.clone());
    let response = session.submit("check the file").unwrap();
    assert_eq!(response.text, "verified");
    let state = call(&f.context(), "task.inspect", json!({}));
    let v: Value = serde_json::from_str(&state.text).unwrap();
    assert_eq!(v["status"], "completed");
    assert_eq!(v["steps"][0]["evidence"][0], "ev-1");
}

struct SilentModel(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl ModelClient for SilentModel {
    fn complete(
        &mut self,
        _: &[Message],
        _: &[Value],
        _: &str,
        _: &ModelLimits,
    ) -> Result<ModelResponse, String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(ModelResponse {
            text: "done".into(),
            ..Default::default()
        })
    }
}
#[test]
fn unfinished_task_gets_one_reminder_then_explicit_failure() {
    let f = Fixture::new();
    let c = f.context();
    assert!(
        call(
            &c,
            "task.start",
            json!({"objective":"still pending","steps":["verify"],"acceptance":["actual check"]})
        )
        .ok
    );
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut cfg = AgentConfig::default();
    cfg.memory.enabled = false;
    cfg.session.transcript = false;
    cfg.skills.dir = f.0.join("skills");
    let mut session = AgentSession::new(cfg, Box::new(SilentModel(calls.clone())))
        .with_runtime_home(c.runtime_home);
    let r = session.submit("continue").unwrap();
    assert_eq!(r.error_type.as_deref(), Some("task_incomplete"));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}
#[test]
fn polling_preserves_utf8_offsets() {
    let f = Fixture::new();
    let c = f.context();
    let id = job(&c, "printf '你好世界abcd'", 3.);
    assert_eq!(until_done(&c, &id)["status"], "completed");
    let mut small = c.config.as_ref().clone();
    small.tools.max_result_chars = 4;
    let c = ToolContext::new(small, f.0.clone()).with_runtime_home(c.runtime_home);
    let mut offset = 0;
    let mut text = String::new();
    loop {
        let v: Value = serde_json::from_str(
            &call(&c, "shell.poll", json!({"job_id":id,"offset":offset})).text,
        )
        .unwrap();
        let next = v["next_offset"].as_u64().unwrap();
        if next == offset {
            break;
        }
        text.push_str(v["output"].as_str().unwrap());
        offset = next;
    }
    assert_eq!(text, "你好世界abcd");
}
#[test]
fn failed_cwd_and_lost_supervisor_are_not_success() {
    let f = Fixture::new();
    let c = f.context();
    let result = call(
        &c,
        "shell.run",
        json!({"command":"true","cwd":"missing","background":true}),
    );
    assert!(result.ok);
    let v: Value = serde_json::from_str(&result.text).unwrap();
    let id = v["job_id"].as_str().unwrap();
    assert_eq!(until_done(&c, id)["status"], "failed");
    let p = c.runtime_home.join("jobs").join(id).join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
    state["status"] = json!("running");
    state["pid"] = json!(std::process::id());
    state["identity"] = json!("not-the-process-identity");
    fs::write(&p, serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(until_done(&c, id)["status"], "interrupted");
    assert!(call(&c, "shell.cancel", json!({"job_id":id})).ok);
    assert!(p.parent().unwrap().join("cancel").exists());
}

#[test]
fn command_label_redacts_before_truncation() {
    let f = Fixture::new();
    let c = f.context();
    let id = job(
        &c,
        &format!("printf '{}test-secret-token'", "x".repeat(145)),
        3.,
    );
    assert_eq!(until_done(&c, &id)["status"], "completed");
    let metadata =
        fs::read_to_string(c.runtime_home.join("jobs").join(id).join("state.json")).unwrap();
    assert!(!metadata.contains("test-"));
    let state: Value = serde_json::from_str(&metadata).unwrap();
    assert!(state["label"].as_str().unwrap().chars().count() <= 160);
}

#[test]
fn restarted_session_can_discover_a_finished_background_job() {
    let f = Fixture::new();
    let c = f.context();
    let id = job(&c, "true", 3.);
    assert_eq!(until_done(&c, &id)["status"], "completed");
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<Message>>>);
    impl ModelClient for Capture {
        fn complete(
            &mut self,
            messages: &[Message],
            _: &[Value],
            _: &str,
            _: &ModelLimits,
        ) -> Result<ModelResponse, String> {
            *self.0.lock().unwrap() = messages.to_vec();
            Ok(ModelResponse {
                text: "okay".into(),
                ..Default::default()
            })
        }
    }
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut config = AgentConfig::default();
    config.memory.enabled = false;
    config.session.transcript = false;
    config.skills.max_catalog = 0;
    config.tools.enabled.clear();
    let mut session = AgentSession::new(config, Box::new(Capture(seen.clone())))
        .with_runtime_home(c.runtime_home);
    session.submit("check previous work").unwrap();
    assert!(seen.lock().unwrap().iter().any(|m| m.content.contains(&id)
        && m.content.contains("completed")
        && m.content.contains("shell.poll")));
}

#[test]
fn silent_jobs_do_not_rewrite_metadata_but_still_cancel() {
    let f = Fixture::new();
    let c = f.context();
    let id = job(&c, "sleep 10", 12.);
    let dir = c.runtime_home.join("jobs").join(&id);
    let deadline = Instant::now() + Duration::from_secs(2);
    while !dir.join("output.log").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let before = fs::metadata(dir.join("state.json"))
        .unwrap()
        .modified()
        .unwrap();
    std::thread::sleep(Duration::from_millis(1300));
    assert_eq!(
        fs::metadata(dir.join("state.json"))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
    assert!(call(&c, "shell.cancel", json!({"job_id":id})).ok);
    assert_eq!(until_done(&c, &id)["status"], "killed");
    assert_ne!(
        fs::metadata(dir.join("state.json"))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
}
