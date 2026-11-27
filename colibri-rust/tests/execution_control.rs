#![cfg(unix)]
use colibri_rust::{config::AgentConfig, model::FakeModel, session::AgentSession};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
struct Home(PathBuf);
impl Home {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "colibri-control-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn wait_file(path: &std::path::Path) {
    let start = Instant::now();
    while !path.exists() {
        assert!(start.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn core_stop_kills_foreground_command_and_saves_a_conclusion_without_later_effects() {
    let home = Home::new();
    let started = home.0.join("started");
    let late = home.0.join("late");
    let mut config = AgentConfig::default();
    config.memory.enabled = false;
    config.session.transcript = false;
    config.tools.max_shell_seconds = 30.;
    let mut session = AgentSession::new(config, Box::new(FakeModel::new()))
        .with_runtime_home(home.0.clone())
        .with_working_directory(home.0.clone());
    let control = session.steer_handle();
    let (tx, rx) = mpsc::channel();
    let command = format!(
        "printf start > {}; sleep 10; printf late > {}",
        started.display(),
        late.display()
    );
    let call = format!("tool:shell.run {}", json!({"command":command}));
    std::thread::spawn(move || {
        let response = session.submit(&call).unwrap();
        tx.send((session, response)).unwrap();
    });
    wait_file(&started);
    assert!(control.steer("补充消息"));
    assert!(control.stop());
    let (mut session, response) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(response.error_type.as_deref(), Some("cancelled"));
    assert!(response.text.contains("阶段性结论"));
    assert!(response.text.contains("执行命令"));
    assert!(!late.exists());
    assert!(session
        .messages
        .last()
        .unwrap()
        .content
        .contains("阶段性结论"));
    assert_eq!(
        session.submit("hello after stop").unwrap().text,
        "fake: hello after stop"
    );
}
struct Worker {
    child: Child,
    input: ChildStdin,
    rx: mpsc::Receiver<Value>,
}
impl Worker {
    fn new(home: &Home) -> Self {
        fs::write(home.0.join("config.toml"),"[model]\nprovider='fake'\n[memory]\nenabled=false\n[session]\nrestore_transcript=false\n").unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_colibri-worker"))
            .arg("--home")
            .arg(&home.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(value) = serde_json::from_str(&line) {
                    if tx.send(value).is_err() {
                        break;
                    }
                }
            }
        });
        Self { child, input, rx }
    }
    fn send(&mut self, v: Value) {
        writeln!(self.input, "{v}").unwrap();
        self.input.flush().unwrap();
    }
    fn until(&self, kind: &str) -> Value {
        let start = Instant::now();
        loop {
            let r = self.rx.recv_timeout(Duration::from_secs(3)).unwrap();
            if r["type"] == kind {
                return r;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "waiting for {kind}, got {r}"
            );
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[test]
fn app_accepts_menu_pinyin_and_steering_while_running_then_stop_and_new_turn() {
    let home = Home::new();
    let mut worker = Worker::new(&home);
    let started = home.0.join("started");
    let late = home.0.join("late");
    worker.send(json!({"type":"submit","text":format!("tool:shell.run {}",json!({"command":format!("printf start > {}; sleep 10; printf late > {}",started.display(),late.display())}))}));
    wait_file(&started);
    worker.send(json!({"type":"settings"}));
    assert!(worker.until("settings")["fields"].is_array());
    worker.send(json!({"type":"pinyin","spelling":"nihao"}));
    assert_eq!(worker.until("pinyin")["spelling"], "nihao");
    worker.send(json!({"type":"submit","text":"只检查，不继续修改"}));
    assert!(worker.until("steering_received")["text"]
        .as_str()
        .unwrap()
        .contains("收到"));
    worker.send(json!({"type":"submit","text":"/stop"}));
    let reply = worker.until("reply");
    assert!(reply["text"].as_str().unwrap().contains("阶段性结论"));
    assert!(!late.exists());
    worker.send(json!({"type":"submit","text":"hello after stop"}));
    assert_eq!(worker.until("reply")["text"], "fake: hello after stop");
    worker.send(json!({"type":"shutdown"}));
    worker.until("shutdown");
    assert!(worker.child.wait().unwrap().success());
    let data = fs::read_dir(home.0.join("transcripts"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .map(|e| fs::read_to_string(e.path()).unwrap())
        .collect::<String>();
    assert!(data.contains("阶段性结论"));
}
#[test]
fn app_shutdown_uses_the_same_stop_path_and_does_not_leave_foreground_commands() {
    let home = Home::new();
    let mut worker = Worker::new(&home);
    let started = home.0.join("started");
    let late = home.0.join("late");
    worker.send(json!({"type":"submit","text":format!("tool:shell.run {}",json!({"command":format!("printf start > {}; sleep 10; printf late > {}",started.display(),late.display())}))}));
    wait_file(&started);
    worker.send(json!({"type":"shutdown"}));
    assert!(worker.until("reply")["text"]
        .as_str()
        .unwrap()
        .contains("阶段性结论"));
    worker.until("shutdown");
    assert!(worker.child.wait().unwrap().success());
    assert!(!late.exists());
}

#[test]
fn model_http_wait_can_be_stopped_without_waiting_for_remote_response() {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    let server = std::thread::spawn(move || loop {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
        }
        if first.starts_with("GET ") {
            let body = r#"{"data":[{"id":"fixture-model","context_length":64000,"max_output_tokens":4096}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            continue;
        }
        tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(1200));
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndone"
        );
        break;
    });
    let home = Home::new();
    let mut config = AgentConfig::default();
    config.memory.enabled = false;
    config.session.transcript = false;
    config.model.provider = "openai_compatible".into();
    config.model.base_url = endpoint;
    config.model.model = "fixture-model".into();
    config.model.api_key = "fixture-key".into();
    config.model.max_retries = 5;
    config.model.retry_backoff_ms = 10000;
    let model = colibri_rust::model::OpenAiCompatibleModel::from_config(&config.model).unwrap();
    let mut session = AgentSession::new(config, Box::new(model)).with_runtime_home(home.0.clone());
    let control = session.steer_handle();
    let task = std::thread::spawn(move || session.submit("wait for model").unwrap());
    rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let start = Instant::now();
    assert!(control.stop());
    let reply = task.join().unwrap();
    assert!(start.elapsed() < Duration::from_millis(800));
    assert_eq!(reply.error_type.as_deref(), Some("cancelled"));
    assert!(reply.text.contains("远端请求结果需核验"));
    server.join().unwrap();
}

#[test]
fn stop_skips_remaining_tools_and_keeps_tool_call_history_well_formed() {
    use colibri_rust::{
        messages::{Message, ModelLimits, ModelResponse, ToolCall},
        model::ModelClient,
    };
    struct Batch(PathBuf);
    impl ModelClient for Batch {
        fn complete(
            &mut self,
            _: &[Message],
            _: &[Value],
            _: &str,
            _: &ModelLimits,
        ) -> Result<ModelResponse, String> {
            Ok(ModelResponse{tool_calls:vec![
    ToolCall{id:"first".into(),name:"shell.run".into(),arguments:json!({"command":format!("printf ready > {}; sleep 10",self.0.join("started").display())}).as_object().unwrap().clone()},
    ToolCall{id:"second".into(),name:"shell.run".into(),arguments:json!({"command":format!("touch {}",self.0.join("late").display())}).as_object().unwrap().clone()},
   ],..Default::default()})
        }
    }
    let home = Home::new();
    let mut config = AgentConfig::default();
    config.memory.enabled = false;
    config.session.transcript = false;
    config.tools.max_shell_seconds = 30.;
    let mut session = AgentSession::new(config, Box::new(Batch(home.0.clone())))
        .with_runtime_home(home.0.clone());
    let control = session.steer_handle();
    let work = std::thread::spawn(move || {
        let reply = session.submit("do both").unwrap();
        (session, reply)
    });
    wait_file(&home.0.join("started"));
    assert!(control.steer("change this"));
    assert!(control.steer("/stop"));
    let (session, reply) = work.join().unwrap();
    assert_eq!(reply.error_type.as_deref(), Some("cancelled"));
    assert!(!home.0.join("late").exists());
    for id in ["first", "second"] {
        assert!(session
            .messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some(id)));
    }
}

#[test]
fn stop_marks_active_task_and_running_step_cancelled_but_keeps_pending_work() {
    let home = Home::new();
    let mut config = AgentConfig::default();
    config.memory.enabled = false;
    config.session.transcript = false;
    let mut session = AgentSession::new(config, Box::new(FakeModel::new()))
        .with_runtime_home(home.0.clone())
        .with_working_directory(home.0.clone());
    session.submit("tool:task.start {\"objective\":\"verify cancellation\",\"steps\":[\"perform work\",\"verify output\"],\"acceptance\":[\"output verified\"]}").unwrap();
    session.submit("tool:task.update {\"step\":1,\"status\":\"running\",\"next_action\":\"finish first step\"}").unwrap();
    let report = session.submit("/stop").unwrap();
    assert!(report.text.contains("verify cancellation"));
    assert!(report.text.contains("未完成：perform work"));
    let path = fs::read_dir(home.0.join("tasks"))
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| e.path().extension().is_some_and(|x| x == "json"))
        .unwrap()
        .path();
    let state: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(state["status"], "cancelled");
    assert_eq!(state["steps"][0]["status"], "cancelled");
    assert_eq!(state["steps"][1]["status"], "pending");
    assert_eq!(state["next_action"], "finish first step");
}
