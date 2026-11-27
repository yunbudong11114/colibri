#![cfg(unix)]

use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "colibri-cardputer-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn worker(home: &Path, args: &[&str], lines: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_colibri-worker"))
        .arg("--home")
        .arg(home)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(lines.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn pipe(home: &Path, requests: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_colibri-worker"))
        .arg("--home")
        .arg(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut rows = Vec::new();
    for request in requests {
        writeln!(input, "{request}").unwrap();
        input.flush().unwrap();
        if request["type"] == "quit" {
            break;
        }
        loop {
            let mut line = String::new();
            assert!(output.read_line(&mut line).unwrap() > 0);
            let row: Value = serde_json::from_str(&line).unwrap();
            let startup = row["startup"] == true;
            let transient = startup
                || matches!(
                    row["type"].as_str(),
                    Some("activity" | "status" | "steering_received" | "steering_applied")
                );
            rows.push(row);
            if !transient {
                break;
            }
        }
    }
    drop(input);
    assert!(child.wait().unwrap().success());
    rows
}

#[test]
fn pinyin_returns_ranked_phrases_and_preserves_uncommitted_input() {
    let fixture = Fixture::new();
    let examples = [
        ("nihao", "你好", 5),
        ("jintiantianqizenmeyang", "今天天气怎么样", 22),
        ("bajiyitongbudao", "把记忆同步到", 15),
        ("xi'an", "西安", 5),
        ("nihaoz", "你好", 5),
    ];
    let requests = examples.map(|(spelling, _, _)| json!({"type":"pinyin","spelling":spelling}));
    let replies = pipe(&fixture.0, &requests);
    for (reply, (spelling, text, consumed)) in replies.iter().zip(examples) {
        assert_eq!(reply["spelling"], spelling);
        assert_eq!(
            reply["candidates"][0],
            json!({"text":text,"consumed":consumed})
        );
    }
    assert!(replies[0]["candidates"]
        .as_array()
        .unwrap()
        .contains(&json!({"text":"你","consumed":2})));
}

#[test]
fn protocol_preserves_settings_memory_and_native_tools() {
    let fixture = Fixture::new();
    let home = fixture.0.join("data");
    let memories = fixture.0.join("memory");
    fs::create_dir(&home).unwrap();
    fs::write(
        home.join("config.toml"),
        "[vision]\nmodel = 'fixture-vl'\napi_key = 'secret-fixture'\n",
    )
    .unwrap();
    let chat_file = fixture.0.join("fixture.txt");
    fs::write(&chat_file, "content outside the data directory").unwrap();
    let rows = pipe(
        &home,
        &[
            json!({"type":"settings"}),
            json!({"type":"save","key":"model.provider","value":"fake"}),
            json!({"type":"save","key":"memory.root","value":memories}),
            json!({"type":"save","key":"vision.model","value":"updated-vl"}),
            json!({"type":"memory_read","file":"USER.md"}),
            json!({"type":"memory_save","file":"USER.md","text":"测试偏好"}),
            json!({"type":"memory_read","file":"USER.md"}),
            json!({"type":"pinyin","spelling":"zhong"}),
            json!({"type":"submit","text":format!("tool:files.read {}",json!({"path":chat_file}))}),
            json!({"type":"new"}),
            json!({"type":"submit","text":"tool:shell.run {\"command\":\"pwd\"}"}),
            json!({"type":"quit"}),
        ],
    );
    assert!(!format!("{rows:?}").contains("secret-fixture"));
    let fields = rows[0]["fields"].as_array().unwrap();
    assert!(fields
        .iter()
        .any(|entry| entry["key"] == "vision.model" && entry["value"] == "fixture-vl"));
    assert_eq!(rows[4]["type"], "memory_file");
    assert_eq!(rows[5]["type"], "error");
    assert_eq!(rows[6]["type"], "memory_file");
    assert_eq!(rows[4]["text"], rows[6]["text"]);
    assert!(!rows[6]["text"].as_str().unwrap().contains("测试偏好"));
    assert!(rows[7]["text"].as_str().unwrap().contains('中'));
    let rows = rows
        .into_iter()
        .filter(|row| row["type"] != "activity")
        .collect::<Vec<_>>();
    assert!(rows[8]["text"]
        .as_str()
        .unwrap()
        .contains("content outside the data directory"));
    assert!(rows[10]["text"]
        .as_str()
        .unwrap()
        .contains(&*home.canonicalize().unwrap().to_string_lossy()));
    let transcripts: String = fs::read_dir(home.join("transcripts"))
        .unwrap()
        .flatten()
        .map(|entry| fs::read_to_string(entry.path()).unwrap())
        .collect();
    assert!(transcripts.contains("\"type\":\"tool_result\""));
    assert!(!transcripts.contains("\"type\":\"permission_decision\""));
    assert_eq!(
        fs::read_to_string(memories.join("USER.md")).unwrap(),
        rows[4]["text"].as_str().unwrap()
    );
}

#[test]
fn startup_returns_visible_history_without_model_connection() {
    let fixture = Fixture::new();
    let home = &fixture.0;
    let dir = home.join("transcripts");
    fs::create_dir(&dir).unwrap();
    let history = dir.join("2026-11-22-history.jsonl");
    fs::write(
        &history,
        format!(
            "{}\n{}\n",
            json!({"type":"user_message","payload":{"text":"之前的问题"}}),
            json!({"type":"assistant_message","payload":{"text":"之前的回答","tool_call_count":0}}),
        ),
    )
    .unwrap();
    let rows = pipe(
        home,
        &[
            json!({"type":"model_status"}),
            json!({"type":"save","key":"session.restore_transcript","value":"false"}),
            json!({"type":"model_status"}),
        ],
    );
    assert_eq!(
        display_texts(&rows[0]["history"]),
        json!([
            {"role":"user","text":"之前的问题"},{"role":"assistant","text":"之前的回答"}
        ])
    );
    assert_eq!(rows[2]["history"], json!([]));
    assert_eq!(fs::read_dir(dir).unwrap().count(), 1);
}

#[test]
fn stdio_discovery_call_and_resource() {
    let fixture = Fixture::new();
    let path = fixture.0.join("server.sh");
    fs::write(&path,r#"#!/bin/sh
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
    [ -n "$id" ] || continue
    case "$line" in
        *'"initialize"'*) result='{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}' ;;
        *'"tools/list"'*'"next"'*) result='{"tools":[{"name":"echo","inputSchema":{"type":"object"}}]}' ;;
        *'"tools/list"'*) result='{"tools":[],"nextCursor":"next"}' ;;
        *'"tools/call"'*) result='{"content":[{"type":"text","text":"MCP fixture"}]}' ;;
        *'"resources/read"'*) result='{"contents":[{"uri":"fixture://cardputer","text":"资源"}]}' ;;
        *) exit 2 ;;
    esac
    printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$result"
done
"#).unwrap();
    let settings = json!({"model.provider":"fake","memory.enabled":false,"mcp.servers":[{"name":"fixture","command":"sh","args":[path]}]});
    fs::write(fixture.0.join("settings.json"), settings.to_string()).unwrap();
    let rows = pipe(
        &fixture.0,
        &[
            json!({"type":"submit","text":"tool:mcp.fixture.echo {\"text\":\"Hello\"}"}),
            json!({"type":"quit"}),
        ],
    );
    assert!(rows.iter().any(|row| row["text"]
        .as_str()
        .is_some_and(|text| text.contains("MCP fixture"))));
    let output = worker(
        &fixture.0,
        &["--repl"],
        "/mcp fixture read_resource fixture://cardputer\n/quit\n",
    );
    assert!(output.contains("资源"));
}

#[test]
fn streamable_http_negotiates_session_and_reads_sse() {
    let fixture = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for step in 0..4 {
            let (mut connection, _) = listener.accept().unwrap();
            connection
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut input = BufReader::new(&connection);
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                assert_ne!(input.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                headers.push_str(&line.to_ascii_lowercase());
            }
            assert!(headers.contains("authorization: bearer fixture-key"));
            if step > 0 {
                assert!(headers.contains("mcp-session-id: test-session"));
                assert!(headers.contains("mcp-protocol-version: 2025-06-18"));
            }
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            let mut body = vec![0; length];
            input.read_exact(&mut body).unwrap();
            let body = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
            let (status, content_type, extra, output) = match step {
                0 => {
                    assert_eq!(body["method"], "initialize");
                    ("200 OK","application/json","Mcp-Session-Id: test-session\r\n",
                    json!({"jsonrpc":"2.0","id":body["id"],"result":{"protocolVersion":"2025-06-18","capabilities":{}}}).to_string())
                }
                1 => {
                    assert_eq!(body["method"], "notifications/initialized");
                    ("202 Accepted", "application/json", "", String::new())
                }
                2 => {
                    assert_eq!(body["method"], "tools/call");
                    (
                        "200 OK",
                        "text/event-stream",
                        "",
                        format!(
                            "event: message\ndata: {}\n\n",
                            json!({"jsonrpc":"2.0","id":body["id"],"result":{"content":[{"type":"text","text":"SSE verified"}]}})
                        ),
                    )
                }
                _ => {
                    assert!(headers.starts_with("delete "));
                    ("204 No Content", "application/json", "", String::new())
                }
            };
            drop(input);
            write!(connection,"HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{output}",output.len()).unwrap();
        }
    });
    fs::write(fixture.0.join("settings.json"),json!({"model.provider":"fake","mcp.servers":[{"name":"fixture","url":format!("http://{address}/mcp"),"token":"fixture-key"}]}).to_string()).unwrap();
    let output = worker(
        &fixture.0,
        &["--repl"],
        "/mcp fixture call_tool example {}\n/quit\n",
    );
    assert!(output.contains("SSE verified"), "{output}");
    server.join().unwrap();
}

#[test]
fn clear_chat_hides_old_turns_across_restarts_without_editing_the_transcript() {
    let fixture = Fixture::new();
    let dir = fixture.0.join("transcripts");
    fs::create_dir(&dir).unwrap();
    let log = dir.join("2026-11-22-history.jsonl");
    let turn = |question, answer| {
        format!(
            "{}\n{}\n",
            json!({"type":"user_message","payload":{"text":question}}),
            json!({"type":"assistant_message","payload":{"text":answer,"tool_call_count":0}}),
        )
    };
    let old = turn("旧问题", "旧回答");
    fs::write(&log, &old).unwrap();
    let rows = pipe(&fixture.0, &[json!({"type":"clear_chat"})]);
    assert_eq!(rows[0]["type"], "chat_cleared");
    assert_eq!(fs::read_to_string(&log).unwrap(), old);
    let all = old + &turn("新问题", "新回答");
    fs::write(&log, &all).unwrap();
    let rows = pipe(&fixture.0, &[json!({"type":"model_status"})]);
    assert_eq!(
        display_texts(&rows[0]["history"]),
        json!([{"role":"user","text":"新问题"},{"role":"assistant","text":"新回答"}])
    );
    assert_eq!(fs::read_to_string(&log).unwrap(), all);
}

#[test]
fn menu_migrates_to_the_native_config_and_retains_comments_and_gateway_settings() {
    let fixture = Fixture::new();
    let path = fixture.0.join("config.toml");
    let original = "# existing comment\n[model]\nprovider = 'fake'\nmodel = 'previous-cli-model' # keep this\ntimeout_seconds = 22\n[channels.weixin]\nenabled = true\nallow_from = ['owner']\n";
    fs::write(&path, original).unwrap();
    let legacy = json!({
        "model.provider":"fake","model.model":"current-menu-model",
        "speech.backend":"offline","session.transcript_retention_days":14
    });
    fs::write(fixture.0.join("settings.json"), legacy.to_string()).unwrap();
    let rows = pipe(&fixture.0, &[json!({"type":"settings"})]);
    assert_eq!(rows[0]["type"], "settings");
    let shared = colibri_rust::config::AgentConfig::load(Some(&path)).unwrap();
    assert_eq!(shared.model.model, "current-menu-model");
    assert_eq!(shared.model.timeout_seconds, 22);
    assert!(!rows[0]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field["key"]
            .as_str()
            .unwrap_or_default()
            .starts_with("speech.")));
    assert_eq!(shared.session.transcript_retention_days, 14);
    assert!(shared.channels_weixin.enabled);
    assert_eq!(shared.channels_weixin.allow_from, ["owner"]);
    let updated = fs::read_to_string(&path).unwrap();
    assert!(updated.contains("# existing comment") && updated.contains("# keep this"));
    assert!(!fixture.0.join("settings.json").exists());
    assert_eq!(
        fs::read_to_string(fixture.0.join("config.toml.pre-cardputer-unification")).unwrap(),
        original
    );
    let rows = pipe(
        &fixture.0,
        &[
            json!({"type":"save","key":"model.model","value":"next-model"}),
            json!({"type":"settings"}),
        ],
    );
    assert_eq!(rows[0]["type"], "saved");
    assert_eq!(
        colibri_rust::config::AgentConfig::load(Some(&path))
            .unwrap()
            .model
            .model,
        "next-model"
    );
    assert_eq!(
        fs::read_to_string(fixture.0.join("settings.json.legacy")).unwrap(),
        legacy.to_string()
    );
}

#[test]
fn config_secrets_are_redacted_before_limits_and_model_context() {
    let fixture = Fixture::new();
    let path = fixture.0.join("config.toml");
    let secret = "fixture-api-token-93-sample";
    let content = format!(
        "[model]\nprovider = \"fake\"\napi_key = \"{secret}\"\n[channels.weixin]\ntoken = \"fixture-weixin-token-94\"\n"
    );
    fs::write(&path, content).unwrap();
    let rows = pipe(
        &fixture.0,
        &[
            json!({"type":"submit","text":format!("tool:files.read {}",json!({"path":path,"start_line":3,"end_line":3,"max_chars":24}))}),
        ],
    );
    let replies = serde_json::to_string(&rows).unwrap();
    assert!(replies.contains("<redacted>"));
    assert!(!replies.contains(secret));
    let logs = fs::read_dir(fixture.0.join("transcripts")).unwrap();
    for item in logs {
        let text = fs::read_to_string(item.unwrap().path()).unwrap();
        assert!(!text.contains(secret));
        assert!(!text.contains("fixture-weixin-token-94"));
    }
}

#[test]
fn changing_model_clears_capacity_override_but_key_refresh_preserves_it() {
    let fixture = Fixture::new();
    let path = fixture.0.join("config.toml");
    fs::write(&path, "[model]\nprovider='openai_compatible'\nbase_url='http://127.0.0.1:9999/v1'\nmodel='old'\ninput_context_tokens=8192\n").unwrap();
    let same = pipe(
        &fixture.0,
        &[json!({
            "type":"save_model","base_url":"http://127.0.0.1:9999/v1","model":"old","api_key":"fixture"
        })],
    );
    assert_eq!(same[0]["type"], "model_saved");
    let value: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        value["model"]["input_context_tokens"].as_integer(),
        Some(8192)
    );
    let changed = pipe(
        &fixture.0,
        &[json!({
            "type":"save_model","base_url":"http://127.0.0.1:9999/v1","model":"new","api_key":"fixture"
        })],
    );
    assert_eq!(changed[0]["type"], "model_saved");
    let value: toml::Value = toml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert!(value["model"].get("input_context_tokens").is_none());
}

#[test]
fn menu_can_set_and_clear_optional_context_budget() {
    let fixture = Fixture::new();
    let response = pipe(
        &fixture.0,
        &[json!({"type":"save","key":"session.max_context_memory_mib","value":"10"})],
    );
    assert_eq!(response[0]["type"], "saved");
    let text = fs::read_to_string(fixture.0.join("config.toml")).unwrap();
    let config: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(
        config["session"]["max_context_memory_mib"].as_integer(),
        Some(10)
    );
    let response = pipe(
        &fixture.0,
        &[json!({"type":"save","key":"session.max_context_memory_mib","value":"0"})],
    );
    assert_eq!(response[0]["type"], "saved");
    let text = fs::read_to_string(fixture.0.join("config.toml")).unwrap();
    let config: toml::Value = toml::from_str(&text).unwrap();
    assert!(config["session"].get("max_context_memory_mib").is_none());
}

#[test]
fn context_memory_budget_can_be_set_and_cleared_from_settings() {
    let fixture = Fixture::new();
    let saved = pipe(
        &fixture.0,
        &[json!({
            "type":"save","key":"session.max_context_memory_mib","value":"10"
        })],
    );
    assert_eq!(saved[0]["type"], "saved");
    let path = fixture.0.join("config.toml");
    let value: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        value["session"]["max_context_memory_mib"].as_integer(),
        Some(10)
    );
    let cleared = pipe(
        &fixture.0,
        &[json!({
            "type":"save","key":"session.max_context_memory_mib","value":"0"
        })],
    );
    assert_eq!(cleared[0]["type"], "saved");
    let value: toml::Value = toml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert!(value["session"].get("max_context_memory_mib").is_none());
}

#[test]
fn unreleased_speech_settings_are_hidden_and_cannot_be_saved() {
    let fixture = Fixture::new();
    let original = "[model]\nprovider='fake'\n[speech]\nbackend='online'\nbase_url='https://example.invalid/v1'\napi_key='private-test-token'\n";
    fs::write(fixture.0.join("config.toml"), original).unwrap();
    let mut requests = vec![json!({"type":"settings"})];
    for key in [
        "speech.backend",
        "speech.base_url",
        "speech.api_key",
        "speech.model",
    ] {
        requests.push(json!({"type":"save","key":key,"value":"offline"}));
    }
    let rows = pipe(&fixture.0, &requests);
    let fields = rows[0]["fields"].as_array().unwrap();
    assert!(!fields
        .iter()
        .any(|field| field["key"].as_str().unwrap().starts_with("speech.")));
    for response in &rows[1..] {
        assert_eq!(response["type"], "error");
    }
    let stored = fs::read_to_string(fixture.0.join("config.toml")).unwrap();
    assert!(stored.contains("backend='online'") || stored.contains("backend = 'online'"));
    assert!(stored.contains("private-test-token"));
}

#[test]
fn compact_menu_protocol_uses_shared_session_without_clearing_display_history() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("config.toml"),
        "[model]\nprovider='fake'\n[session]\ntranscript=false\nrestore_transcript=false\n",
    )
    .unwrap();
    let rows = pipe(
        &fixture.0,
        &[
            json!({"type":"submit","text":"hello"}),
            json!({"type":"submit","text":"/compact"}),
            json!({"type":"submit","text":"continue"}),
        ],
    );
    let rows = rows
        .into_iter()
        .filter(|row| row["type"] == "reply")
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert_eq!(row["type"], "reply");
    }
    assert_eq!(rows[0]["text"], "fake: hello");
    assert_eq!(rows[2]["text"], "fake: continue");
}

#[test]
fn manual_compaction_emits_transient_activity_before_reply() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("config.toml"), "[model]\nprovider='fake'\n[memory]\nenabled=false\n[session]\ntranscript=false\nrestore_transcript=false\n").unwrap();
    let rows = pipe(
        &fixture.0,
        &[
            json!({"type":"submit","text":"first"}),
            json!({"type":"submit","text":"second"}),
            json!({"type":"submit","text":"/compact"}),
        ],
    );
    let start = rows
        .iter()
        .position(|row| row["type"] == "activity" && row["text"] == "压缩上下文")
        .unwrap();
    assert_eq!(rows[start]["text"], "压缩上下文");
    assert_eq!(rows.last().unwrap()["type"], "reply");
    assert!(rows.last().unwrap()["text"]
        .as_str()
        .unwrap()
        .contains("已压缩"));
    assert!(rows.iter().all(|row| row["type"] != "status"));
}

#[test]
fn first_launch_initializes_memory_before_model_configuration() {
    let fixture = Fixture::new();
    pipe(&fixture.0, &[json!({"type":"model_status"})]);
    for name in [
        "SOUL.md",
        "USER.md",
        "MEMORY.md",
        "INDEX.md",
        "topics/sample.md",
    ] {
        assert!(fixture.0.join("memory").join(name).is_file(), "{name}");
    }
}

#[test]
fn startup_reports_memory_errors_and_keeps_settings_available() {
    let fixture = Fixture::new();
    let blocked = fixture.0.join("blocked");
    fs::write(&blocked, "keep this file").unwrap();
    fs::write(
        fixture.0.join("config.toml"),
        format!("[memory]\nroot='{}'\n", blocked.display()),
    )
    .unwrap();
    let recovered = fixture.0.join("recovered");
    let rows = pipe(
        &fixture.0,
        &[
            json!({"type":"settings"}),
            json!({"type":"save","key":"memory.root","value":recovered}),
            json!({"type":"memory_read","file":"USER.md"}),
        ],
    );
    assert_eq!(rows[0]["type"], "error");
    assert!(rows[0]["text"]
        .as_str()
        .unwrap()
        .contains("Memory initialization failed"));
    assert_eq!(rows[1]["type"], "settings");
    assert_eq!(rows[3]["type"], "memory_file");
    assert!(!rows[3]["text"].as_str().unwrap().is_empty());
    assert!(recovered.join("USER.md").is_file());
    assert_eq!(fs::read_to_string(blocked).unwrap(), "keep this file");
}

#[test]
fn disabled_memory_on_first_launch_creates_no_memory_directory() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("config.toml"), "[memory]\nenabled=false\n").unwrap();
    pipe(&fixture.0, &[json!({"type":"model_status"})]);
    assert!(!fixture.0.join("memory").exists());
}

#[test]
fn task_progress_is_available_without_model_and_restores_local_scope() {
    let fixture = Fixture::new();
    let rows = pipe(&fixture.0, &[json!({"type":"task_status"})]);
    assert_eq!(rows[0]["type"], "task_status");
    assert!(rows[0]["text"]
        .as_str()
        .unwrap()
        .contains("当前没有记录的任务"));
    let context = colibri_rust::tools::ToolContext::new(
        colibri_rust::config::AgentConfig::default(),
        fixture.0.clone(),
    )
    .with_runtime_home(fixture.0.clone());
    assert!(colibri_rust::tools::run_tool("task.start",&json!({"objective":"测试小屏进度","steps":["读取文件","验证结果"],"acceptance":["结果可复现"]}).to_string(),&context).unwrap().ok);
    let rows = pipe(&fixture.0, &[json!({"type":"task_status"})]);
    let text = rows[0]["text"].as_str().unwrap();
    assert!(
        text.contains("测试小屏进度") && text.contains("读取文件") && text.contains("结果可复现")
    );
}

#[test]
fn missing_model_hint_uses_app_settings_path_and_keeps_configuration_available() {
    let fixture = Fixture::new();
    let rows = pipe(
        &fixture.0,
        &[json!({"type":"model_status"}), json!({"type":"settings"})],
    );
    assert!(rows[0]["text"]
        .as_str()
        .unwrap()
        .starts_with("未配置 AI 模型"));
    assert!(rows[0]["text"]
        .as_str()
        .unwrap()
        .contains("设置 → 配置 → 模型"));
    assert!(rows[0]["text"].as_str().unwrap().contains("Enter"));
    assert_eq!(rows[1]["type"], "settings");
    let fields = rows[1]["fields"].as_array().unwrap();
    assert!(fields.iter().any(|field| field["key"] == "model.base_url"));
    assert!(!fields
        .iter()
        .any(|field| field["key"].as_str().unwrap().starts_with("display.")));
}

#[test]
fn memory_menu_reads_agent_updates_and_rejects_save_requests() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("config.toml"),
        "[model]\nprovider='fake'\n[session]\ntranscript=false\nrestore_transcript=false\n",
    )
    .unwrap();
    let rows = pipe(
        &fixture.0,
        &[
            json!({"type":"submit","text":"tool:memory.write {\"file\":\"USER.md\",\"mode\":\"replace\",\"content\":\"Agent 更新的记忆\"}"}),
            json!({"type":"memory_read","file":"USER.md"}),
            json!({"type":"memory_save","file":"USER.md","text":"不应保存","revision":null}),
            json!({"type":"memory_read","file":"USER.md"}),
            json!({"type":"save","key":"memory.enabled","value":"false"}),
            json!({"type":"memory_read","file":"USER.md"}),
        ],
    );
    let rows = rows
        .iter()
        .filter(|row| row["type"] != "activity" && row["type"] != "status")
        .collect::<Vec<_>>();
    assert_eq!(rows[0]["type"], "reply");
    assert!(rows[1]["text"]
        .as_str()
        .unwrap()
        .contains("Agent 更新的记忆"));
    assert_eq!(rows[2]["type"], "error");
    assert_eq!(rows[1]["text"], rows[3]["text"]);
    assert_eq!(rows[1]["text"], rows[5]["text"]);
    assert!(!fs::read_to_string(fixture.0.join("memory/USER.md"))
        .unwrap()
        .contains("不应保存"));
}

#[test]
fn app_restart_restores_local_turns_by_time_and_excludes_weixin_after_clear() {
    use std::io::Write;
    let fixture = Fixture::new();
    fs::write(fixture.0.join("config.toml"), "[model]\nprovider='fake'\n").unwrap();
    let dir = fixture.0.join("transcripts");
    fs::create_dir(&dir).unwrap();
    let turn = |hour: &str, source: &str, text: &str| {
        format!(
            "{}\n{}\n",
            json!({"ts":format!("2026-11-26T{hour}:00+08:00"),"type":"user_message","payload":{"session_key":source,"text":text}}),
            json!({"ts":format!("2026-11-26T{hour}:01+08:00"),"type":"assistant_message","payload":{"session_key":source,"text":format!("回复{text}"),"tool_call_count":0}})
        )
    };
    let newest = dir.join("2026-11-26-100.jsonl");
    fs::write(&newest, turn("21:00", "local", "本地最新")).unwrap();
    fs::write(
        dir.join("2026-11-26-200.jsonl"),
        turn("20:00", "local", "本地较早"),
    )
    .unwrap();
    let gateway = dir.join("2026-11-26.jsonl");
    fs::write(&gateway, turn("22:00", "weixin:fixture", "微信消息")).unwrap();
    let history =
        || display_texts(&pipe(&fixture.0, &[json!({"type":"model_status"})])[0]["history"]);
    let expected = json!([
        {"role":"user","text":"本地较早"},{"role":"assistant","text":"回复本地较早"},
        {"role":"user","text":"本地最新"},{"role":"assistant","text":"回复本地最新"}
    ]);
    assert_eq!(history(), expected);
    assert_eq!(history(), expected); // another process restart, not cached UI state
    pipe(&fixture.0, &[json!({"type":"clear_chat"})]);
    assert_eq!(history(), json!([]));
    fs::OpenOptions::new()
        .append(true)
        .open(&newest)
        .unwrap()
        .write_all(turn("23:00", "local", "清屏后本地").as_bytes())
        .unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&gateway)
        .unwrap()
        .write_all(turn("23:30", "weixin:fixture", "清屏后微信").as_bytes())
        .unwrap();
    assert_eq!(
        history(),
        json!([
            {"role":"user","text":"清屏后本地"},{"role":"assistant","text":"回复清屏后本地"}
        ])
    );
    assert!(fs::read_to_string(gateway).unwrap().contains("微信消息"));
}

#[test]
fn app_processes_share_daily_log_and_clear_hides_only_the_earlier_records() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("config.toml"),
        "[model]\nprovider='fake'\n[memory]\nenabled=false\n",
    )
    .unwrap();
    pipe(
        &fixture.0,
        &[json!({"type":"submit","text":"first local question"})],
    );
    let files = || {
        fs::read_dir(fixture.0.join("transcripts"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .collect::<Vec<_>>()
    };
    let original = files();
    assert_eq!(original.len(), 1);
    assert_eq!(original[0].file_name().unwrap().to_str().unwrap().len(), 16); // YYYY-MM-DD.jsonl
    let mut remote = colibri_rust::transcript::TranscriptWriter::new(
        original[0].clone(),
        std::collections::BTreeMap::from([("session_key".into(), "weixin:fixture".into())]),
        0,
        0,
    )
    .unwrap();
    remote
        .write("user_message", json!({"text":"remote question"}))
        .unwrap();
    remote
        .write(
            "assistant_message",
            json!({"text":"remote answer","tool_call_count":0}),
        )
        .unwrap();
    pipe(&fixture.0, &[json!({"type":"clear_chat"})]);
    let rows = pipe(
        &fixture.0,
        &[json!({"type":"submit","text":"second local question"})],
    );
    let answer = rows.iter().find(|r| r["type"] == "reply").unwrap()["text"].clone();
    let rows = pipe(&fixture.0, &[json!({"type":"model_status"})]);
    assert_eq!(
        display_texts(&rows[0]["history"]),
        json!([
            {"role":"user","text":"second local question"}, {"role":"assistant","text":answer}
        ])
    );
    assert_eq!(files(), original);
    let data = fs::read_to_string(&original[0]).unwrap();
    assert!(data.contains("first local question"));
    assert!(data.contains("remote answer"));
}

#[test]
fn app_with_transcript_disabled_does_not_create_a_log_directory() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("config.toml"),
        "[model]\nprovider='fake'\n[memory]\nenabled=false\n[session]\ntranscript=false\n",
    )
    .unwrap();
    pipe(
        &fixture.0,
        &[json!({"type":"submit","text":"unlogged question"})],
    );
    assert!(!fixture.0.join("transcripts").exists());
}

fn display_texts(history: &Value) -> Value {
    json!(history
        .as_array()
        .unwrap()
        .iter()
        .map(|m| json!({"role":m["role"],"text":m["text"]}))
        .collect::<Vec<_>>())
}

#[test]
fn history_page_protocol_preserves_request_id_and_clear_boundary() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.0.join("transcripts")).unwrap();
    let mut events = String::new();
    for n in 0..10 {
        for (kind, text) in [
            ("user_message", format!("q{n}")),
            ("assistant_message", format!("a{n}")),
        ] {
            events += &(json!({"ts":format!("2026-11-26T12:{n:02}:00"),"type":kind,"payload":{"text":text,"tool_call_count":0}}).to_string()+"\n");
        }
    }
    fs::write(fixture.0.join("transcripts/2026-11-26.jsonl"), events).unwrap();
    let rows = pipe(
        &fixture.0,
        &[
            json!({"type":"history_page","direction":"first","request_id":42}),
            json!({"type":"quit"}),
        ],
    );
    let page = rows.iter().find(|r| r["type"] == "history_page").unwrap();
    assert_eq!(page["request_id"], 42);
    assert_eq!(page["history"][0]["text"], "q0");
    assert_eq!(page["has_after"], true);
    pipe(
        &fixture.0,
        &[json!({"type":"clear_chat"}), json!({"type":"quit"})],
    );
    let rows = pipe(
        &fixture.0,
        &[
            json!({"type":"history_page","direction":"last","request_id":43}),
            json!({"type":"quit"}),
        ],
    );
    let page = rows.iter().find(|r| r["type"] == "history_page").unwrap();
    assert_eq!(page["total"], 0);
}
