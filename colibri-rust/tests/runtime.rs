use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use colibri_rust::cli::{run_steering_pump, run_with_io};
use colibri_rust::config::{expand_user_path, AgentConfig};
use colibri_rust::gateway::{format_gateway_status, GatewaySessionCache, GatewayStatus};
use colibri_rust::memory::MemoryContext;
use colibri_rust::messages::{MediaPart, Message, ModelLimits, ToolCall};
use colibri_rust::model::{FakeModel, ModelClient, OpenAiCompatibleModel};
use colibri_rust::permissions::{
    PermissionPolicy, PermissionPrompter, PermissionRequest, ProjectGrants, ProjectPermissionStore,
};
use colibri_rust::repl_input::{
    handle_escape_sequence, read_escape_sequence_with, read_repl_line, try_read_line,
    write_raw_tty_newline, ReplLineEditor,
};
use colibri_rust::session::AgentSession;
use colibri_rust::session_history::TranscriptHistoryLoader;
use colibri_rust::skills::{relevant_skill_context, SkillIndex};
use colibri_rust::steering::{format_steering_ack, SteerHandle, SteeringState, SKIPPED_TOOL_RESULT};
use colibri_rust::terminal_qr::render_terminal_qr;
use colibri_rust::tools::{run_tool, ToolContext, ToolInfo};
use colibri_rust::transcript::TranscriptWriter;
use colibri_rust::weixin::{
    cleanup_media_directory, decrypt_aes_ecb, download_inbound_media, encrypt_aes_ecb,
    parse_weixin_updates, permission_choice, send_weixin_media, send_weixin_text,
};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn run_cli(args: &[&str]) -> (i32, String, String) {
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let config_dir = temp_dir("cli-config");
    let config_path = config_dir.join("config.toml");
    fs::write(
        &config_path,
        "[model]\nprovider = \"fake\"\nmodel = \"fake-colibri-model\"\n",
    )
    .unwrap();
    let mut full_args = vec!["--config".to_string(), config_path.display().to_string()];
    full_args.extend(args.iter().map(|value| value.to_string()));
    let code = run_with_io(
        full_args,
        "".as_bytes(),
        SharedBuf(Arc::clone(&stdout)),
        SharedBuf(Arc::clone(&stderr)),
    );
    let stdout_text = String::from_utf8(stdout.lock().unwrap().clone()).unwrap();
    let stderr_text = String::from_utf8(stderr.lock().unwrap().clone()).unwrap();
    (code, stdout_text, stderr_text)
}

fn run_cli_raw(args: &[String], stdin: &str) -> (i32, String, String) {
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let code = run_with_io(
        args.to_vec(),
        stdin.as_bytes(),
        SharedBuf(Arc::clone(&stdout)),
        SharedBuf(Arc::clone(&stderr)),
    );
    let stdout_text = String::from_utf8(stdout.lock().unwrap().clone()).unwrap();
    let stderr_text = String::from_utf8(stderr.lock().unwrap().clone()).unwrap();
    (code, stdout_text, stderr_text)
}

#[test]
fn default_config_matches_python_runtime_defaults() {
    let config = AgentConfig::default();

    assert_eq!(config.model.provider, "fake");
    assert_eq!(config.model.model, "fake-colibri-model");
    assert_eq!(config.model.max_output_tokens, 16384);
    assert_eq!(config.model.input_context_tokens, 48000);
    assert_eq!(config.vision.model, "");
    assert_eq!(config.vision.base_url, "");
    assert_eq!(config.vision.api_key, "");
    assert_eq!(config.vision.timeout_seconds, 60);
    assert_eq!(config.vision.max_image_bytes, 4 * 1024 * 1024);
    assert_eq!(config.session.max_tool_rounds, 32);
    assert_eq!(config.session.trigger_message_limit, 96);
    assert_eq!(config.session.recent_message_limit, 12);
    assert_eq!(config.session.summary_max_chars, 12000);
    assert!(config.session.restore_transcript);
    assert_eq!(config.session.restore_message_limit, 24);
    assert_eq!(config.session.restore_char_limit, 24000);
    assert_eq!(config.session.restore_scan_bytes, 2 * 1024 * 1024);
    assert_eq!(config.session.transcript_retention_days, 30);
    assert_eq!(config.session.transcript_max_total_bytes, 128 * 1024 * 1024);
    assert_eq!(config.tools.max_result_chars, 32000);
    assert!(config.tools.enabled.contains(&"web".to_string()));
    assert!(config.tools.enabled.contains(&"image".to_string()));
    assert!(!config.tools.enabled.contains(&"mcp".to_string()));
    assert_eq!(config.web_search.engine, "baidu");
    assert_eq!(config.gateway.enabled_channels, vec!["weixin".to_string()]);
    assert!(!config.channels_weixin.enabled);
    assert_eq!(config.shell.deny[0], "rm");
    assert_eq!(config.files.roots[1], Path::new("/tmp/colibri"));
    assert!(config.console.status);
    assert!(config.console.plain_answer);
    assert!(expand_user_path("~/.colibri").is_absolute());
}

#[test]
fn load_config_overrides_nested_values() {
    let temp = temp_dir("config-overrides");
    let config_path = temp.join("agent.toml");
    fs::write(
        &config_path,
        r#"
[model]
provider = "openai_compatible"
model = "gpt-4.1-mini"
api_key = "inline-key"
timeout_seconds = 45
input_context_tokens = 1000000

[vision]
model = "vision-model"
base_url = "https://vision.example/v1"
api_key = "vision-key"
timeout_seconds = 33
max_image_bytes = 1234

[session]
idle_exit_enabled = true
idle_exit_seconds = 12
model_compact = false
restore_transcript = false
restore_message_limit = 10
restore_char_limit = 9000
restore_scan_bytes = 123456
transcript_retention_days = 7
transcript_max_total_bytes = 7654321

[files]
roots = ["~/notes", "/tmp"]

[console]
status = false

[channels.weixin]
enabled = true
token = "wx-token"
allow_from = ["user-1"]

[mcp]
enabled = true
startup = "eager"
max_active_servers = 3
"#,
    )
    .unwrap();

    let config = AgentConfig::load(Some(&config_path)).unwrap();

    assert_eq!(config.model.provider, "openai_compatible");
    assert_eq!(config.model.model, "gpt-4.1-mini");
    assert_eq!(config.model.api_key, "inline-key");
    assert_eq!(config.model.timeout_seconds, 45);
    assert_eq!(config.model.input_context_tokens, 1000000);
    assert_eq!(config.vision.model, "vision-model");
    assert_eq!(config.vision.base_url, "https://vision.example/v1");
    assert_eq!(config.vision.api_key, "vision-key");
    assert_eq!(config.vision.timeout_seconds, 33);
    assert_eq!(config.vision.max_image_bytes, 1234);
    assert!(config.session.idle_exit_enabled);
    assert_eq!(config.session.idle_exit_seconds, 12);
    assert!(!config.session.model_compact);
    assert!(!config.session.restore_transcript);
    assert_eq!(config.session.restore_message_limit, 10);
    assert_eq!(config.session.restore_char_limit, 9000);
    assert_eq!(config.session.restore_scan_bytes, 123456);
    assert_eq!(config.session.transcript_retention_days, 7);
    assert_eq!(config.session.transcript_max_total_bytes, 7654321);
    assert_eq!(config.files.roots[0].file_name().unwrap(), "notes");
    assert_eq!(config.files.roots[1], Path::new("/tmp"));
    assert!(!config.console.status);
    assert!(config.console.plain_answer);
    assert!(config.channels_weixin.enabled);
    assert_eq!(config.channels_weixin.token, "wx-token");
    assert_eq!(
        config.channels_weixin.allow_from,
        vec!["user-1".to_string()]
    );
}

#[test]
fn load_config_uses_real_toml_semantics_for_strings_with_hash() {
    let temp = temp_dir("config-toml-semantics");
    let config_path = temp.join("agent.toml");
    fs::write(
        &config_path,
        r#"
[model]
api_key = "key#not-a-comment"

[channels.weixin]
token = "wx#secret"
"#,
    )
    .unwrap();

    let config = AgentConfig::load(Some(&config_path)).unwrap();

    assert_eq!(config.model.api_key, "key#not-a-comment");
    assert_eq!(config.channels_weixin.token, "wx#secret");
}

#[test]
fn load_without_path_reads_user_default_config() {
    let _guard = env_lock().lock().unwrap();
    let temp = temp_dir("config-user-default");
    let old_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &temp);
    fs::create_dir_all(temp.join(".colibri")).unwrap();
    fs::write(
        temp.join(".colibri/config.toml"),
        "[model]\nmodel = \"from-user-default\"\n",
    )
    .unwrap();

    let config = AgentConfig::load(None).unwrap();

    restore_home(old_home);
    assert_eq!(config.model.model, "from-user-default");
}

#[test]
fn load_without_path_falls_back_when_user_default_missing() {
    let _guard = env_lock().lock().unwrap();
    let temp = temp_dir("config-user-default-missing");
    let old_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &temp);

    let config = AgentConfig::load(None).unwrap();

    restore_home(old_home);
    assert_eq!(config.model.model, "fake-colibri-model");
}

#[test]
fn ask_prints_fake_response_and_status() {
    let _guard = env_lock().lock().unwrap();
    let (code, stdout, stderr) = run_cli(&["ask", "status"]);

    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "fake: status");
    assert!(stderr.contains("[colibri] ready model=fake-colibri-model"));
    assert!(stderr.contains("[colibri] thinking"));
}

#[test]
fn console_plain_answer_and_status_events_match_python() {
    let plain = colibri_rust::console::format_plain_answer(
        "## Title\nHello **world** and `code`\n| A | B |\n| --- | --- |\n| 1 | 2 |\n",
    );
    assert_eq!(plain, "Title\nHello world and code\nA / B\n1 / 2");

    let line = colibri_rust::console::status_line_for_event(
        "tool_result",
        &serde_json::json!({"name":"files.read","ok":true,"text":"abcd"}),
    );
    assert_eq!(line.as_deref(), Some("[colibri] tool files.read ok chars=4"));

    let steered = colibri_rust::console::status_line_for_event(
        "steered",
        &serde_json::json!({"skipped": 2, "chars": 11}),
    );
    assert_eq!(
        steered.as_deref(),
        Some("[colibri] steered skipped=2 chars=11")
    );
}

#[test]
fn steering_skip_result_constant_matches_python() {
    assert_eq!(
        SKIPPED_TOOL_RESULT,
        "Skipped due to queued user message."
    );
}

#[test]
fn format_steering_ack_matches_python() {
    assert_eq!(
        format_steering_ack(2, "别用 rm"),
        "已改方向，跳过剩余 2 个工具\n改：别用 rm"
    );
    assert_eq!(
        format_steering_ack(0, "  "),
        "已改方向，跳过剩余 0 个工具"
    );

    let text = "一二三四五六七八九十一二三四五六七八九十多余";
    let ack = format_steering_ack(1, text);
    assert!(ack.starts_with("已改方向，跳过剩余 1 个工具\n改："));
    let preview = ack.split_once('\n').unwrap().1.strip_prefix("改：").unwrap();
    assert!(preview.ends_with('…'));
    assert_eq!(preview.trim_end_matches('…').chars().count(), 20);
}

#[test]
fn steer_rejected_when_turn_inactive() {
    let mut config = AgentConfig::default();
    config.session.transcript = false;
    config.memory.enabled = false;
    let session = AgentSession::new(config, Box::new(FakeModel::new()));

    assert!(!session.steer("change plan"));
    assert!(!session.is_turn_active());
}

#[test]
fn steer_rejected_while_permission_pending() {
    let mut config = AgentConfig::default();
    config.session.transcript = false;
    config.memory.enabled = false;
    let session = AgentSession::new(config, Box::new(FakeModel::new()));
    let handle = session.steer_handle();
    handle.set_turn_active_for_test(true);
    handle.set_permission_pending_for_test(true);

    assert!(!session.steer("change plan"));
    assert!(session.is_permission_pending());
}

#[test]
fn steer_skips_remaining_tools_and_injects_user_message() {
    let mut config = AgentConfig::default();
    config.session.transcript = false;
    config.memory.enabled = false;
    config.tools.default_permission = "allow".to_string();

    let acks = Arc::new(Mutex::new(Vec::new()));
    let acks_for_notifier = Arc::clone(&acks);
    let mut session = AgentSession::new(config, Box::new(TwoToolsThenTextModel::new()))
        .with_steer_notifier(Arc::new(move |text| {
            acks_for_notifier.lock().unwrap().push(text);
        }));

    let handle = session.steer_handle();
    let steered_once = Arc::new(AtomicBool::new(false));
    let steered_flag = Arc::clone(&steered_once);
    session = session.with_status_callback(
        true,
        Arc::new(move |line| {
            if line.contains("tool files.list") && line.contains(" ok ") && !steered_flag.swap(true, Ordering::SeqCst)
            {
                assert!(handle.steer("change plan"));
            }
        }),
    );

    let response = session.submit("do work").unwrap();

    assert_eq!(response.text, "steered-ok");
    assert!(session.messages.iter().any(|message| {
        message.role == "tool"
            && message.tool_call_id.as_deref() == Some("call_b")
            && message.content.contains(SKIPPED_TOOL_RESULT)
    }));
    assert!(session
        .messages
        .iter()
        .any(|message| message.role == "user" && message.content == "change plan"));
    assert_eq!(
        *acks.lock().unwrap(),
        vec![format_steering_ack(1, "change plan")]
    );
    assert!(!session.is_turn_active());
}

#[test]
fn steer_during_text_only_complete_is_applied() {
    let mut config = AgentConfig::default();
    config.session.transcript = false;
    config.memory.enabled = false;
    config.tools.default_permission = "allow".to_string();

    let acks = Arc::new(Mutex::new(Vec::new()));
    let acks_for_notifier = Arc::clone(&acks);
    let handle_slot: Arc<Mutex<Option<SteerHandle>>> = Arc::new(Mutex::new(None));
    let model = SteerDuringTextOnlyModel {
        handle_slot: Arc::clone(&handle_slot),
        calls: 0,
    };
    let mut session = AgentSession::new(config, Box::new(model)).with_steer_notifier(Arc::new(
        move |text| {
            acks_for_notifier.lock().unwrap().push(text);
        },
    ));
    *handle_slot.lock().unwrap() = Some(session.steer_handle());

    let response = session.submit("do work").unwrap();

    assert_eq!(response.text, "steered-ok");
    assert!(session
        .messages
        .iter()
        .any(|message| message.role == "user" && message.content == "change plan"));
    assert_eq!(
        *acks.lock().unwrap(),
        vec![format_steering_ack(0, "change plan")]
    );
    assert!(!session.is_turn_active());
}

#[test]
fn steering_queue_empty_after_normal_submit() {
    let mut config = AgentConfig::default();
    config.session.transcript = false;
    config.memory.enabled = false;
    let mut session = AgentSession::new(config, Box::new(PlainTextModel));

    let response = session.submit("hello").unwrap();

    assert_eq!(response.text, "done");
    assert!(!session.is_turn_active());
    assert!(session.steer_handle().drain_one_for_test().is_none());
}

#[test]
fn gateway_get_existing_does_not_create_session() {
    let mut config = AgentConfig::default();
    config.gateway.max_sessions = 2;
    config.gateway.session_idle_seconds = 0;
    let mut cache = GatewaySessionCache::new(config).unwrap();

    assert!(cache.get_existing("weixin:user-1").is_none());
    assert!(cache.steer_handle_for("weixin:user-1").is_none());
    assert!(!cache.try_steer("weixin:user-1", "change plan"));

    cache.get_or_create("weixin:user-1").unwrap();
    assert!(cache.get_existing("weixin:user-1").is_some());
    assert!(cache.steer_handle_for("weixin:user-1").is_some());
}

#[test]
fn gateway_try_steer_enqueues_when_turn_active() {
    let mut config = AgentConfig::default();
    config.gateway.max_sessions = 2;
    config.gateway.session_idle_seconds = 0;
    let mut cache = GatewaySessionCache::new(config).unwrap();
    cache.get_or_create("weixin:user-1").unwrap();
    let handle = cache.steer_handle_for("weixin:user-1").unwrap();
    handle.set_turn_active_for_test(true);

    assert!(cache.try_steer("weixin:user-1", "change plan"));
    assert_eq!(
        handle.drain_one_for_test().as_deref(),
        Some("change plan")
    );
}

#[test]
fn gateway_try_steer_works_while_session_taken_for_submit() {
    let mut config = AgentConfig::default();
    config.gateway.max_sessions = 2;
    config.gateway.session_idle_seconds = 0;
    let mut cache = GatewaySessionCache::new(config).unwrap();
    let session = cache
        .take_or_create_with_metadata_and_media_sender(
            "weixin:user-1",
            std::collections::BTreeMap::new(),
            None,
        )
        .unwrap();
    let handle = cache.steer_handle_for("weixin:user-1").unwrap();
    handle.set_turn_active_for_test(true);

    // Receive can steer without holding the session or blocking on submit.
    assert!(cache.try_steer("weixin:user-1", "change plan"));
    assert!(!cache.contains_key("weixin:user-1"));
    assert_eq!(
        handle.drain_one_for_test().as_deref(),
        Some("change plan")
    );

    cache.put_back("weixin:user-1", session);
    assert!(cache.contains_key("weixin:user-1"));
}

#[test]
fn steering_pump_forwards_line_to_steer() {
    let handle = SteerHandle::new(Arc::new(SteeringState::new()));
    handle.set_turn_active_for_test(true);
    let stop = AtomicBool::new(false);
    let lines = Mutex::new(vec![Some("change plan".to_string()), None]);

    run_steering_pump(
        &handle,
        &stop,
        |_| {
            let mut lines = lines.lock().unwrap();
            if lines.is_empty() {
                stop.store(true, Ordering::SeqCst);
                return None;
            }
            let next = lines.remove(0);
            if next.is_none() {
                stop.store(true, Ordering::SeqCst);
            }
            next
        },
        || {},
        |_| {},
    );

    assert_eq!(
        handle.drain_one_for_test().as_deref(),
        Some("change plan")
    );
}

#[test]
fn steering_pump_skips_read_while_permission_pending() {
    let handle = SteerHandle::new(Arc::new(SteeringState::new()));
    handle.set_turn_active_for_test(true);
    handle.set_permission_pending_for_test(true);
    let stop = AtomicBool::new(false);
    let read_calls = Mutex::new(Vec::new());
    let sleeps = Mutex::new(Vec::new());

    run_steering_pump(
        &handle,
        &stop,
        |timeout| {
            read_calls.lock().unwrap().push(timeout);
            stop.store(true, Ordering::SeqCst);
            Some("should-not-reach".to_string())
        },
        || {},
        |duration| {
            sleeps.lock().unwrap().push(duration);
            if sleeps.lock().unwrap().len() >= 2 {
                handle.set_permission_pending_for_test(false);
            }
        },
    );

    assert_eq!(sleeps.lock().unwrap().len(), 2);
    assert_eq!(*read_calls.lock().unwrap(), vec![0.2]);
    assert_eq!(
        handle.drain_one_for_test().as_deref(),
        Some("should-not-reach")
    );
}

#[test]
fn steering_pump_notifies_permission_pending_once() {
    let handle = SteerHandle::new(Arc::new(SteeringState::new()));
    handle.set_turn_active_for_test(true);
    let stop = AtomicBool::new(false);
    let notifies = Mutex::new(0usize);
    let sleep_count = Mutex::new(0usize);

    run_steering_pump(
        &handle,
        &stop,
        |_| {
            handle.set_permission_pending_for_test(true);
            Some("steer-me".to_string())
        },
        || {
            *notifies.lock().unwrap() += 1;
        },
        |_| {
            let mut count = sleep_count.lock().unwrap();
            *count += 1;
            if *count >= 3 {
                stop.store(true, Ordering::SeqCst);
            }
        },
    );

    assert_eq!(*notifies.lock().unwrap(), 1);
    assert!(*sleep_count.lock().unwrap() >= 3);
    assert!(handle.drain_one_for_test().is_none());
}

#[test]
fn try_read_line_returns_none_when_stdin_not_tty() {
    // In cargo test, process stdin is typically not a TTY.
    assert!(try_read_line(0.05, None).is_none());
}

#[test]
fn diagnostics_prints_key_value_lines() {
    let _guard = env_lock().lock().unwrap();
    let (code, stdout, _stderr) = run_cli(&["diagnostics"]);

    assert_eq!(code, 0);
    assert!(stdout.contains("colibri diagnostics"));
    assert!(stdout.contains("provider=fake model=fake-colibri-model"));
}

#[test]
fn gateway_without_action_prints_usage() {
    let _guard = env_lock().lock().unwrap();
    let (code, _stdout, stderr) = run_cli(&["gateway"]);

    assert_eq!(code, 2);
    assert!(stderr.contains("colibri gateway {run,start,stop,restart,status}"));
}

#[test]
fn repl_exits_on_quit_like_python_cli() {
    let _guard = env_lock().lock().unwrap();
    let temp = temp_dir("repl-quit");
    let config_path = temp.join("config.toml");
    fs::write(&config_path, "[model]\nprovider = \"fake\"\n").unwrap();
    let args = vec![
        "--config".to_string(),
        config_path.display().to_string(),
        "repl".to_string(),
    ];

    let (code, stdout, stderr) = run_cli_raw(&args, "/quit\n");

    assert_eq!(code, 0);
    assert_eq!(stdout, "colibri> ");
    assert!(stderr.contains("[colibri] ready model=fake-colibri-model"));
}

#[test]
fn repl_line_editor_backspace_removes_cjk_and_redraws_like_python() {
    let mut stdout = Vec::new();
    {
        let mut editor = ReplLineEditor::new("colibri> ", &mut stdout, Vec::new());
        editor.start();
        editor.feed_text("尿尿是豆阿斯顿");
        editor.backspace();
        editor.backspace();
        editor.feed_text("斯顿");
        assert_eq!(editor.text(), "尿尿是豆阿斯顿");
    }
    let output = String::from_utf8(stdout).unwrap();
    assert!(output.contains("\r\x1b[2Kcolibri> 尿尿是豆阿斯"));
    assert!(output.ends_with("\r\x1b[2Kcolibri> 尿尿是豆阿斯顿"));
}

#[test]
fn repl_line_editor_history_navigation_does_not_print_escape_text_like_python() {
    let mut stdout = Vec::new();
    {
        let mut editor = ReplLineEditor::new(
            "colibri> ",
            &mut stdout,
            vec!["first".to_string(), "第二个问题".to_string()],
        );
        editor.start();
        editor.feed_text("draft");
        editor.history_previous();
        editor.history_previous();
        editor.history_next();
        editor.history_next();
        assert_eq!(editor.text(), "draft");
    }
    let output = String::from_utf8(stdout).unwrap();
    assert!(!output.contains("\x1b[A"));
    assert!(!output.contains("\x1b[B"));
    assert!(output.contains("\r\x1b[2Kcolibri> 第二个问题"));
    assert!(output.contains("\r\x1b[2Kcolibri> first"));
    assert!(output.ends_with("\r\x1b[2Kcolibri> draft"));
}

#[test]
fn repl_write_raw_tty_newline_returns_cursor_to_column_zero_like_python() {
    let mut stdout = Vec::new();

    write_raw_tty_newline(&mut stdout);

    assert_eq!(String::from_utf8(stdout).unwrap(), "\r\n");
}

#[test]
fn read_repl_line_reads_unicode_from_plain_stream_like_python() {
    let mut stdin = "我有我\n".as_bytes();
    let mut stdout = Vec::new();

    let text = read_repl_line("colibri> ", 0.0, &[], &mut stdin, &mut stdout).unwrap();

    assert_eq!(text.as_deref(), Some("我有我"));
    assert_eq!(String::from_utf8(stdout).unwrap(), "colibri> ");
}

#[test]
fn handle_escape_sequence_navigates_history_for_arrow_keys_like_python() {
    let mut stdout = Vec::new();
    let mut editor = ReplLineEditor::new(
        "colibri> ",
        &mut stdout,
        vec!["first".to_string(), "第二个问题".to_string()],
    );
    editor.start();
    editor.feed_text("draft");

    handle_escape_sequence(&mut editor, b"\x1b[A");
    assert_eq!(editor.text(), "第二个问题");
    handle_escape_sequence(&mut editor, b"\x1bOA");
    assert_eq!(editor.text(), "first");
    handle_escape_sequence(&mut editor, b"\x1b[B");
    assert_eq!(editor.text(), "第二个问题");
    handle_escape_sequence(&mut editor, b"\x1bOB");
    assert_eq!(editor.text(), "draft");
}

#[test]
fn read_escape_sequence_consumes_arrow_key_bytes_like_python() {
    let mut bytes = vec![b'[', b'A'];
    let sequence = read_escape_sequence_with(
        || {
            if bytes.is_empty() {
                Vec::new()
            } else {
                vec![bytes.remove(0)]
            }
        },
        |_| true,
    );
    assert_eq!(sequence, b"\x1b[A");
}

#[test]
fn repl_keeps_history_across_turns_for_arrow_navigation() {
    let temp = temp_dir("repl-history");
    let config_path = temp.join("config.toml");
    fs::write(
        &config_path,
        "[model]\nprovider = \"fake\"\nmodel = \"fake-colibri-model\"\n[console]\nstatus = false\n",
    )
    .unwrap();
    let args = vec![
        "--config".to_string(),
        config_path.display().to_string(),
        "repl".to_string(),
    ];
    let (code, stdout, _stderr) = run_cli_raw(&args, "hello\n/quit\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("fake: hello"));
    assert!(stdout.contains("colibri> "));
}

#[test]
fn session_records_fake_response() {
    let config = AgentConfig::default();
    let mut session = AgentSession::new(config, Box::new(FakeModel::new()));

    let response = session.submit("hello").unwrap();

    assert_eq!(response.text, "fake: hello");
    assert_eq!(session.messages.len(), 2);
    assert_eq!(session.messages[0].role, "user");
    assert_eq!(session.messages[1].role, "assistant");
}

#[test]
fn session_compacts_at_model_boundary_not_after_assistant_like_python() {
    let mut config = AgentConfig::default();
    config.session.trigger_message_limit = 6;
    config.session.recent_message_limit = 4;
    config.session.model_compact = false;
    config.session.transcript = false;
    config.memory.enabled = false;
    let mut session = AgentSession::new(config, Box::new(FakeModel::new()));

    session.submit("one").unwrap();
    session.submit("two").unwrap();
    session.submit("three").unwrap();

    assert_eq!(session.messages.len(), 6);
    assert!(session.summary.is_empty());

    session.submit("four").unwrap();

    assert_eq!(
        session
            .messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        vec![
            "fake: two",
            "three",
            "fake: three",
            "four",
            "fake: four",
        ]
    );
    assert!(session.summary.contains("user: one"));
}

#[test]
fn session_close_closes_owned_transcript() {
    let temp = temp_dir("session-close");
    let path = temp.join("events.jsonl");
    let writer = TranscriptWriter::new(path.clone(), BTreeMap::new(), 0, 0).unwrap();
    let transcript = Arc::new(Mutex::new(writer));
    let mut config = AgentConfig::default();
    config.session.transcript = false;
    config.memory.enabled = false;
    let mut session = AgentSession::from_shared(
        Arc::new(config),
        Arc::new(Mutex::new(Box::new(FakeModel::new()) as Box<dyn ModelClient>)),
        Some(Arc::clone(&transcript)),
        BTreeMap::new(),
    );
    session.submit("hello").unwrap();
    assert!(path.is_file());
    assert!(fs::read_to_string(&path).unwrap().contains("user_message"));

    session.close();
    // Exclusive Arc was closed; shared clone still exists but file handle released.
    drop(transcript);
}

#[test]
fn gateway_session_cache_close_closes_like_python() {
    let mut config = AgentConfig::default();
    config.gateway.max_sessions = 2;
    config.gateway.session_idle_seconds = 0;
    config.session.transcript = false;
    let mut cache = GatewaySessionCache::new(config).unwrap();
    cache.get_or_create("weixin:user-1").unwrap();
    assert_eq!(cache.len(), 1);
    cache.close();
    assert_eq!(cache.len(), 0);
}

#[test]
fn session_sends_media_result_through_media_sender_like_python() {
    let temp = temp_dir("session-send-media");
    let path = temp.join("report.txt");
    fs::write(&path, "hello").unwrap();
    let sent = Arc::new(Mutex::new(Vec::<MediaPart>::new()));
    let sent_for_sender = Arc::clone(&sent);
    let mut config = AgentConfig::default();
    config.files.roots = vec![temp.clone()];
    config.tools.default_permission = "allow".to_string();
    config.memory.enabled = false;
    config.session.transcript = false;
    let mut session = AgentSession::new(config, Box::new(FakeModel::new()));
    session.set_media_sender(Some(Arc::new(move |media| {
        sent_for_sender.lock().unwrap().push(media);
        Ok(())
    })));
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(&temp).unwrap();

    let response = session
        .submit(r#"tool:files.send {"path":"report.txt","caption":"请看"}"#)
        .unwrap();

    std::env::set_current_dir(old).unwrap();
    assert_eq!(response.text, "final: Sent file to channel: report.txt");
    assert_eq!(
        sent.lock().unwrap().as_slice(),
        &[MediaPart::new(
            "file",
            path.canonicalize().unwrap(),
            "report.txt",
            "text/plain",
            "请看",
        )]
    );
}

#[test]
fn session_turns_media_sender_failure_into_tool_error_like_python() {
    let temp = temp_dir("session-send-media-fail");
    fs::write(temp.join("report.txt"), "hello").unwrap();
    let mut config = AgentConfig::default();
    config.files.roots = vec![temp.clone()];
    config.tools.default_permission = "allow".to_string();
    config.memory.enabled = false;
    config.session.transcript = false;
    let mut session = AgentSession::new(config, Box::new(FakeModel::new()));
    session.set_media_sender(Some(Arc::new(|_media| Err("send failed".to_string()))));
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(&temp).unwrap();

    let response = session
        .submit(r#"tool:files.send {"path":"report.txt"}"#)
        .unwrap();

    std::env::set_current_dir(old).unwrap();
    assert_eq!(response.text, "final: media_send_error: send failed");
}

#[test]
fn submit_appends_media_paths_to_user_message_like_python() {
    let temp = temp_dir("session-inbound-media");
    let image = temp.join("photo.png");
    let mut config = AgentConfig::default();
    config.memory.enabled = false;
    config.session.transcript = false;
    let mut session = AgentSession::new(config, Box::new(FakeModel::new()));

    let response = session
        .submit_with_media(
            "这张图里有什么",
            vec![MediaPart::new(
                "image",
                image.clone(),
                "photo.png",
                "image/png",
                "",
            )],
        )
        .unwrap();

    assert!(session.messages[0]
        .content
        .contains("Attachments saved locally:"));
    assert!(session.messages[0].content.contains(&format!(
        "image: photo.png at {}, content_type=image/png",
        image.display()
    )));
    assert!(response.text.contains("Attachments saved locally:"));
}

#[test]
fn session_uses_model_assisted_compact_and_retains_latest_user_like_python() {
    let mut config = AgentConfig::default();
    config.model.provider = "openai_compatible".to_string();
    config.session.trigger_message_limit = 3;
    config.session.recent_message_limit = 1;
    config.session.model_compact = true;
    config.session.transcript = false;
    config.memory.enabled = false;
    let mut session = AgentSession::new(config, Box::new(CompactScriptModel::new()));
    session.messages.push(Message::new("user", "old user"));
    session
        .messages
        .push(Message::new("assistant", "old assistant"));

    let response = session.submit("latest request").unwrap();

    assert_eq!(response.text, "done");
    assert_eq!(session.summary, "Summary:\nimportant compacted context");
    assert!(session
        .messages
        .iter()
        .any(|message| message.role == "user" && message.content == "latest request"));
    assert!(!session.summary.contains("<analysis>"));
}

#[test]
fn session_compacts_when_model_input_tokens_reach_threshold_like_python() {
    let mut config = AgentConfig::default();
    config.model.input_context_tokens = 30;
    config.session.trigger_message_limit = 99;
    config.session.recent_message_limit = 2;
    config.session.model_compact = false;
    config.session.transcript = false;
    config.memory.enabled = false;
    let mut session = AgentSession::new(config, Box::new(BudgetInspectModel));
    session
        .messages
        .push(Message::new("user", &format!("old user {}", "x".repeat(50))));
    session.messages.push(Message::new(
        "assistant",
        &format!("old assistant {}", "y".repeat(50)),
    ));

    let response = session
        .submit("latest message")
        .unwrap();

    assert_eq!(response.text, "budget ok");
    assert!(!session.summary.is_empty());
}

#[test]
fn retain_recent_message_groups_keeps_tool_pairs_intact_like_python() {
    let call = ToolCall {
        id: "call_1".to_string(),
        name: "files.read".to_string(),
        arguments: serde_json::Map::new(),
    };
    let mut assistant = Message::new("assistant", "");
    assistant.tool_calls = vec![call];
    let messages = vec![
        Message::new("user", "active request"),
        assistant.clone(),
        Message::tool("result", "call_1"),
        Message::new("assistant", "done"),
    ];

    let kept = colibri_rust::context::retain_recent_message_groups(messages, 1);
    assert_eq!(
        kept.iter()
            .map(|message| (message.role.as_str(), message.tool_call_id.as_deref()))
            .collect::<Vec<_>>(),
        vec![("user", None), ("assistant", None)]
    );

    let oversized = vec![
        Message::new("user", "active request"),
        assistant,
        Message::tool("result", "call_1"),
    ];
    let kept_whole = colibri_rust::context::retain_recent_message_groups(oversized, 1);
    assert_eq!(
        kept_whole
            .iter()
            .map(|message| (message.role.as_str(), message.tool_call_id.as_deref()))
            .collect::<Vec<_>>(),
        vec![("user", None), ("assistant", None), ("tool", Some("call_1"))]
    );
}

#[test]
fn session_does_not_log_context_budget_for_token_triggered_compaction_like_python() {
    let temp = temp_dir("session-context-events");
    let transcript_path = temp.join("transcripts/events.jsonl");
    let mut config = AgentConfig::default();
    config.model.input_context_tokens = 30;
    config.session.trigger_message_limit = 99;
    config.session.recent_message_limit = 2;
    config.session.model_compact = false;
    config.session.transcript = true;
    config.memory.enabled = false;
    let writer = TranscriptWriter::new(transcript_path.clone(), BTreeMap::new(), 0, 0).unwrap();
    let mut session = AgentSession::from_shared(
        Arc::new(config),
        Arc::new(Mutex::new(Box::new(FakeModel::new()) as Box<dyn ModelClient>)),
        Some(Arc::new(Mutex::new(writer))),
        BTreeMap::new(),
    );

    session.submit("first xxxxxxxxxxxxxxxxxxxxxxxx").unwrap();
    session.submit("second yyyyyyyyyyyyyyyyyyyyyyyy").unwrap();

    let text = fs::read_to_string(transcript_path).unwrap();
    assert!(!text.contains("\"type\":\"context_budget\""));
    assert!(!text.contains("\"dropped_model_messages\""));
    assert!(!text.contains("drop_old_message_groups"));
    assert!(text.contains("\"type\":\"context_compact\""));
    assert!(text.contains("\"mode\":\"fallback\""));
}

#[test]
fn session_keeps_large_tool_result_text_for_model_context_like_python() {
    let _guard = env_lock().lock().unwrap();
    let temp = temp_dir("session-tool-result-summary");
    let full_text = format!("{}\n{}", "A".repeat(80), "B".repeat(80));
    fs::write(temp.join("note.txt"), &full_text).unwrap();
    let transcript_path = temp.join("transcripts/events.jsonl");
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(&temp).unwrap();
    let mut config = AgentConfig::default();
    config.files.roots = vec![temp.clone()];
    config.memory.enabled = false;
    config.tools.max_result_chars = 500;
    config.session.transcript = true;
    let writer = TranscriptWriter::new(transcript_path.clone(), BTreeMap::new(), 0, 0).unwrap();
    let mut session = AgentSession::from_shared(
        Arc::new(config),
        Arc::new(Mutex::new(Box::new(FakeModel::new()) as Box<dyn ModelClient>)),
        Some(Arc::new(Mutex::new(writer))),
        BTreeMap::new(),
    );

    let response = session
        .submit(r#"tool:files.read {"path":"note.txt"}"#)
        .unwrap();

    std::env::set_current_dir(old).unwrap();
    let tool_message = session
        .messages
        .iter()
        .find(|message| message.role == "tool")
        .expect("tool message");
    assert_eq!(tool_message.content, full_text);
    assert!(response.text.contains(&full_text));
    let transcript = fs::read_to_string(transcript_path).unwrap();
    assert!(transcript.contains(&"A".repeat(80)));
    assert!(transcript.contains(&"B".repeat(80)));
}

#[test]
fn context_pressure_warning_is_not_injected_for_large_model_input_like_python() {
    let mut config = AgentConfig::default();
    config.model.input_context_tokens = 30;
    config.session.max_tool_rounds = 4;
    config.session.transcript = false;
    config.memory.enabled = false;
    let mut session = AgentSession::new(config, Box::new(RepeatedBudgetPressureModel::new()));
    session
        .messages
        .push(Message::new("user", &format!("old {}", "x".repeat(160))));
    session
        .messages
        .push(Message::new("assistant", &format!("old {}", "y".repeat(160))));

    let response = session.submit("start").unwrap();

    assert!(response.text.contains("Tool round limit reached after 4 rounds."));
    assert!(!session
        .messages
        .iter()
        .any(|message| message.content.contains("Context budget is tight")));
}

#[test]
fn session_falls_back_and_logs_compact_error_like_python() {
    let temp = temp_dir("session-compact-error");
    let transcript_path = temp.join("transcripts/events.jsonl");
    let mut config = AgentConfig::default();
    config.model.provider = "openai_compatible".to_string();
    config.session.trigger_message_limit = 3;
    config.session.recent_message_limit = 2;
    config.session.model_compact = true;
    config.session.transcript = true;
    config.memory.enabled = false;
    let writer = TranscriptWriter::new(transcript_path.clone(), BTreeMap::new(), 0, 0).unwrap();
    let mut session = AgentSession::from_shared(
        Arc::new(config),
        Arc::new(Mutex::new(
            Box::new(FailingCompactModel::new()) as Box<dyn ModelClient>
        )),
        Some(Arc::new(Mutex::new(writer))),
        BTreeMap::new(),
    );

    session.submit("one").unwrap();
    session.submit("two").unwrap();

    let text = fs::read_to_string(transcript_path).unwrap();
    assert!(text.contains("\"type\":\"context_compact_error\""));
    assert!(text.contains("\"fallback\":true"));
    assert!(text.contains("\"mode\":\"fallback\""));
    assert!(session.summary.contains("user: one") || session.summary.contains("user: two"));
}

#[test]
fn session_round_limit_text_matches_python() {
    let mut config = AgentConfig::default();
    config.session.max_tool_rounds = 1;
    config.session.transcript = false;
    config.memory.enabled = false;
    config.tools.default_permission = "allow".to_string();
    let mut session = AgentSession::new(config, Box::new(AlwaysToolModel::new()));

    let response = session.submit("loop").unwrap();

    assert!(response
        .text
        .contains("Tool round limit reached after 1 round."));
    assert!(response.text.contains("The task may still be incomplete."));
    assert!(response.text.contains("Recent tool results:"));
    assert!(response.text.contains("You can continue the task"));
    assert!(response
        .text
        .contains("do not claim the previous task was fully completed"));
}

#[test]
fn memory_context_replaces_invalid_utf8_like_python() {
    let temp = temp_dir("memory-lossy-utf8");
    let root = temp.join("memory");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("MEMORY.md"), b"hello \xff world").unwrap();
    fs::write(root.join("USER.md"), "user prefs").unwrap();
    let mut config = AgentConfig::default();
    config.memory.root = root;

    let result = MemoryContext::new(config).load().unwrap();

    assert!(result.text.contains("[MEMORY.md]"));
    assert!(result.text.contains("hello"));
    assert!(result.text.contains("world"));
    assert!(result.text.contains("[USER.md]"));
}

#[test]
fn skill_toml_parses_multiline_description_like_python() {
    let temp = temp_dir("skill-toml-multiline");
    let skill_dir = temp.join("skills/release");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "# Release\n").unwrap();
    fs::write(
        skill_dir.join("skill.toml"),
        r#"
description = """
Release helper
with details
"""

[[commands]]
name = "render"
description = "Render notes"
command = "python"
args = ["scripts/render.py", "--verbose"]
read_only = true
"#,
    )
    .unwrap();
    let index = SkillIndex::scan(&[temp.join("skills")]);
    let release = index.get("release").unwrap();

    assert!(release.description.contains("Release helper"));
    assert!(release.description.contains("with details"));
    assert_eq!(release.commands[0].name, "render");
    assert_eq!(
        release.commands[0].args,
        vec!["scripts/render.py".to_string(), "--verbose".to_string()]
    );
    assert!(release.commands[0].read_only);
}

#[test]
fn session_denies_tool_calls_when_permission_mode_is_deny() {
    let mut config = AgentConfig::default();
    config.tools.default_permission = "deny".to_string();
    let mut session = AgentSession::new(config, Box::new(FakeModel::new()));

    let response = session.submit(r#"tool:files.list {"path":"."}"#).unwrap();

    assert!(response.text.contains("permission_denied"));
    assert!(session.messages.iter().any(
        |message| message.role == "tool" && message.content.contains("User denied files.list")
    ));
}

#[test]
fn session_allows_read_only_tool_calls_by_default() {
    let _guard = env_lock().lock().unwrap();
    let temp = temp_dir("session-readonly");
    fs::write(temp.join("note.txt"), "hello").unwrap();
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(&temp).unwrap();
    let mut config = AgentConfig::default();
    config.files.roots = vec![temp.clone()];
    config.memory.root = temp.join("memory");
    let mut session = AgentSession::new(config, Box::new(FakeModel::new()));

    let response = session
        .submit(r#"tool:files.read {"path":"note.txt"}"#)
        .unwrap();

    std::env::set_current_dir(old).unwrap();
    assert!(response.text.contains("hello"));
}

#[test]
fn session_denies_write_and_execute_tools_without_approval_by_default() {
    let _guard = env_lock().lock().unwrap();
    let temp = temp_dir("session-deny-write");
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(&temp).unwrap();
    let mut config = AgentConfig::default();
    config.files.roots = vec![temp.clone()];
    config.memory.root = temp.join("memory");
    let mut session = AgentSession::new(config, Box::new(FakeModel::new()));

    let response = session
        .submit(r#"tool:files.write {"path":"note.txt","content":"nope"}"#)
        .unwrap();

    std::env::set_current_dir(old).unwrap();
    assert!(response.text.contains("permission_denied"));
    assert!(!temp.join("note.txt").exists());
}

#[test]
fn cli_ask_confirms_write_tool_from_stdin_like_python() {
    let _guard = env_lock().lock().unwrap();
    let temp = temp_dir("cli-permission-confirm");
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(&temp).unwrap();
    let config_path = temp.join("config.toml");
    fs::write(
        &config_path,
        "[model]\nprovider = \"fake\"\n\n[files]\nroots = [\".\"]\n",
    )
    .unwrap();
    let args = vec![
        "--config".to_string(),
        config_path.display().to_string(),
        "ask".to_string(),
        r#"tool:files.write {"path":"note.txt","content":"approved"}"#.to_string(),
    ];

    let (code, stdout, stderr) = run_cli_raw(&args, "y\n");

    std::env::set_current_dir(old).unwrap();
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("file: files.write"));
    assert!(stdout.contains("final: Wrote"));
    assert_eq!(
        fs::read_to_string(temp.join("note.txt")).unwrap(),
        "approved"
    );
}

#[test]
fn cli_ask_project_permission_persists_like_python() {
    let _guard = env_lock().lock().unwrap();
    let temp = temp_dir("cli-permission-project");
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(&temp).unwrap();
    let config_path = temp.join("config.toml");
    fs::write(
        &config_path,
        "[model]\nprovider = \"fake\"\n\n[files]\nroots = [\".\"]\n",
    )
    .unwrap();
    let args = vec![
        "--config".to_string(),
        config_path.display().to_string(),
        "ask".to_string(),
        r#"tool:shell.run {"command":"pwd"}"#.to_string(),
    ];

    let (first_code, first_stdout, first_stderr) = run_cli_raw(&args, "p\n");
    let (second_code, second_stdout, second_stderr) = run_cli_raw(&args, "");

    std::env::set_current_dir(old).unwrap();
    assert_eq!(first_code, 0, "stderr={first_stderr}");
    assert!(first_stdout.contains("shell: pwd"));
    assert_eq!(second_code, 0, "stderr={second_stderr}");
    assert!(!second_stdout.contains("shell: pwd"));
    assert!(second_stdout.contains("final:"));
    let permissions = fs::read_to_string(temp.join(".colibri/permissions.toml")).unwrap();
    assert!(permissions.contains("commands = [\"pwd\"]"));
}

#[test]
fn cli_ask_allows_out_of_root_file_after_dynamic_permission_like_python() {
    let _guard = env_lock().lock().unwrap();
    let temp = temp_dir("cli-out-root-permission");
    let allowed = temp.join("allowed");
    let outside = temp.join("outside");
    fs::create_dir_all(&allowed).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("note.txt"), "outside hello").unwrap();
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(&allowed).unwrap();
    let config_path = temp.join("config.toml");
    fs::write(
        &config_path,
        format!(
            "[model]\nprovider = \"fake\"\n\n[files]\nroots = [\"{}\"]\n",
            allowed.display()
        ),
    )
    .unwrap();
    let args = vec![
        "--config".to_string(),
        config_path.display().to_string(),
        "ask".to_string(),
        format!(
            r#"tool:files.read {{"path":"{}"}}"#,
            outside.join("note.txt").display()
        ),
    ];

    let (code, stdout, stderr) = run_cli_raw(&args, "y\n");

    std::env::set_current_dir(old).unwrap();
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("file: files.read"));
    assert!(stdout.contains("final: outside hello"));
}

#[test]
fn files_tool_lists_reads_and_writes_inside_allowed_root() {
    let temp = temp_dir("files-tool");
    fs::write(temp.join("alpha.txt"), "alpha").unwrap();
    fs::create_dir_all(temp.join("nested")).unwrap();
    let mut config = AgentConfig::default();
    config.files.roots = vec![temp.clone()];
    config.tools.max_result_chars = 10;
    let context = ToolContext::new(config, temp.clone());

    let listed = run_tool("files.list", r#"{"path":"."}"#, &context).unwrap();
    assert_eq!(
        listed.text.lines().collect::<Vec<_>>(),
        vec!["alpha.txt", "nested/"]
    );

    let read = run_tool("files.read", r#"{"path":"alpha.txt"}"#, &context).unwrap();
    assert_eq!(read.text, "alpha");

    let written = run_tool(
        "files.write",
        r#"{"path":"nested/out.txt","content":"hello"}"#,
        &context,
    )
    .unwrap();
    assert!(written.ok);
    assert_eq!(
        fs::read_to_string(temp.join("nested/out.txt")).unwrap(),
        "hello"
    );
}

#[test]
fn files_read_range_and_max_chars_match_python() {
    let temp = temp_dir("files-read-range");
    fs::write(temp.join("note.txt"), "one\ntwo\nthreeeeeeeeee\nfour\n").unwrap();
    let mut config = AgentConfig::default();
    config.files.roots = vec![temp.clone()];
    config.tools.max_result_chars = 100;
    let context = ToolContext::new(config, temp.clone());

    let read = run_tool(
        "files.read",
        r#"{"path":"note.txt","start_line":2,"end_line":4,"max_chars":20}"#,
        &context,
    )
    .unwrap();

    assert!(read.ok);
    assert!(read.truncated);
    assert_eq!(read.text, "two\nt\n...[truncated]");

    let invalid = run_tool(
        "files.read",
        r#"{"path":"note.txt","start_line":3,"end_line":2}"#,
        &context,
    )
    .unwrap();
    assert!(!invalid.ok);
    assert_eq!(invalid.error_type.as_deref(), Some("invalid_arguments"));
}

#[test]
fn files_send_returns_media_result_for_allowed_file_like_python() {
    let temp = temp_dir("files-send");
    let path = temp.join("report.txt");
    fs::write(&path, "hello").unwrap();
    let mut config = AgentConfig::default();
    config.files.roots = vec![temp.clone()];
    let context =
        ToolContext::new(config, temp.clone()).with_media_sender(Arc::new(|_media| Ok(())));

    let result = run_tool(
        "files.send",
        r#"{"path":"report.txt","caption":"给你文件"}"#,
        &context,
    )
    .unwrap();

    assert!(result.ok);
    assert_eq!(result.text, "Sent file to channel: report.txt");
    assert_eq!(
        result.media,
        Some(MediaPart::new(
            "file",
            path.canonicalize().unwrap(),
            "report.txt",
            "text/plain",
            "给你文件",
        ))
    );
}

#[test]
fn files_send_requires_channel_media_sender_like_python() {
    let temp = temp_dir("files-send-unavailable");
    fs::write(temp.join("report.txt"), "hello").unwrap();
    let mut config = AgentConfig::default();
    config.files.roots = vec![temp.clone()];
    let context = ToolContext::new(config, temp);

    let result = run_tool("files.send", r#"{"path":"report.txt"}"#, &context).unwrap();

    assert!(!result.ok);
    assert_eq!(result.error_type.as_deref(), Some("media_unavailable"));
}

#[test]
fn image_understand_uses_fake_vision_model_for_allowed_image_like_python() {
    let temp = temp_dir("image-understand");
    let image = temp.join("photo.png");
    fs::write(&image, b"png").unwrap();
    let mut config = AgentConfig::default();
    config.files.roots = vec![temp.clone()];
    let analyzer_config = config.clone();
    let context = ToolContext::new(config, temp.clone()).with_image_analyzer(Arc::new(
        move |path, prompt| colibri_rust::vision::analyze_image(&analyzer_config, path, prompt),
    ));

    let result = run_tool(
        "image.understand",
        r#"{"path":"photo.png","prompt":"describe"}"#,
        &context,
    )
    .unwrap();

    assert!(result.ok);
    assert_eq!(result.text, "fake image: describe");
}

#[test]
fn shell_tool_rejects_denied_executable() {
    let temp = temp_dir("shell-tool");
    let config = AgentConfig::default();
    let context = ToolContext::new(config, temp);

    let result = run_tool("shell.run", r#"{"command":"rm file"}"#, &context).unwrap();

    assert!(!result.ok);
    assert_eq!(result.error_type.as_deref(), Some("permission_denied"));
}

#[test]
fn shell_tool_reports_invalid_quoted_command_like_python() {
    let temp = temp_dir("shell-invalid-quote");
    let config = AgentConfig::default();
    let context = ToolContext::new(config, temp);

    let result = run_tool(
        "shell.run",
        r#"{"command":"printf 'unterminated"}"#,
        &context,
    )
    .unwrap();

    assert!(!result.ok);
    assert_eq!(result.error_type.as_deref(), Some("invalid_arguments"));
}

#[test]
fn shell_tool_executes_argv_directly_without_shell_redirection() {
    let temp = temp_dir("shell-direct-argv");
    let config = AgentConfig::default();
    let context = ToolContext::new(config, temp.clone());

    let result = run_tool(
        "shell.run",
        r#"{"command":"echo hello > out.txt"}"#,
        &context,
    )
    .unwrap();

    assert!(result.ok);
    assert_eq!(result.text, "hello > out.txt\n");
    assert!(!temp.join("out.txt").exists());
}

#[test]
fn permission_policy_matches_python_session_and_project_grants() {
    let temp = temp_dir("permission-policy");
    let config = AgentConfig::default();
    let context = ToolContext::new(config.clone(), temp.clone());
    let mut prompter = FakePermissionPrompter::new(vec!["s"]);
    let mut policy =
        PermissionPolicy::from_config(&config, temp.clone(), Some(&mut prompter));
    let shell = ToolInfo::new("shell.run", false);
    let mut args = BTreeMap::new();
    args.insert("command".to_string(), "pwd".to_string());

    let first = policy.decide(&shell, &args, &context);
    let second = policy.decide(&shell, &args, &context);

    assert!(first.allowed);
    assert_eq!(first.scope, "session");
    assert!(second.allowed);
    assert_eq!(second.scope, "session");
    assert_eq!(prompter.requests.len(), 1);

    let store = ProjectPermissionStore::for_cwd(temp.clone());
    store
        .save(&ProjectGrants {
            shell_commands: vec!["git status".to_string()],
            tool_names: vec![],
            file_roots: vec![],
        })
        .unwrap();
    let mut deny_prompter = FakePermissionPrompter::new(vec!["n"]);
    let mut project_policy =
        PermissionPolicy::from_config(&config, temp.clone(), Some(&mut deny_prompter));
    let mut git_args = BTreeMap::new();
    git_args.insert("command".to_string(), "git status".to_string());
    let granted = project_policy.decide(&shell, &git_args, &context);
    assert!(granted.allowed);
    assert_eq!(granted.scope, "project");
}

#[test]
fn permission_policy_classifies_shell_redirection_as_file_path() {
    let temp = temp_dir("permission-shell-redirection");
    let config = AgentConfig::default();
    let context = ToolContext::new(config.clone(), temp.clone());
    let mut prompter = FakePermissionPrompter::new(vec!["n"]);
    let mut policy = PermissionPolicy::from_config(&config, temp.clone(), Some(&mut prompter));
    let shell = ToolInfo::new("shell.run", false);
    let mut args = BTreeMap::new();
    args.insert("command".to_string(), "printf hello > out.txt".to_string());

    let decision = policy.decide(&shell, &args, &context);

    assert!(!decision.allowed);
    assert_eq!(decision.subject_kind, "file_path");
    let expected = temp.join("out.txt").display().to_string();
    assert_eq!(
        prompter.requests[0].file_path.as_deref(),
        Some(expected.as_str())
    );
}

#[test]
fn permission_policy_hard_deny_wins_over_shell_redirection_file_path() {
    let temp = temp_dir("permission-hard-deny-redirection");
    let config = AgentConfig::default();
    let context = ToolContext::new(config.clone(), temp.clone());
    let mut prompter = FakePermissionPrompter::new(vec!["s"]);
    let mut policy = PermissionPolicy::from_config(&config, temp, Some(&mut prompter));
    let shell = ToolInfo::new("shell.run", false);
    let mut args = BTreeMap::new();
    args.insert("command".to_string(), "rm old.txt > out.txt".to_string());

    let decision = policy.decide(&shell, &args, &context);

    assert!(!decision.allowed);
    assert_eq!(decision.scope, "none");
    assert_eq!(decision.reason, "hard_deny");
    assert!(prompter.requests.is_empty());
}

#[test]
fn permission_policy_classifies_out_of_root_file_paths() {
    let temp = temp_dir("permission-file-root");
    let allowed = temp.join("allowed");
    let outside = temp.join("outside");
    fs::create_dir_all(&allowed).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let mut config = AgentConfig::default();
    config.files.roots = vec![allowed.clone()];
    let context = ToolContext::new(config.clone(), allowed.clone());
    let mut prompter = FakePermissionPrompter::new(vec!["s"]);
    let mut policy = PermissionPolicy::from_config(&config, allowed, Some(&mut prompter));
    let tool = ToolInfo::new("files.list", true);
    let mut args = BTreeMap::new();
    args.insert("path".to_string(), outside.display().to_string());

    let decision = policy.decide(&tool, &args, &context);

    assert!(decision.allowed);
    assert_eq!(decision.subject_kind, "file_path");
    assert_eq!(decision.scope, "session_file_root");
    assert_eq!(prompter.requests[0].subject_kind, "file_path");
    assert_eq!(prompter.requests[0].tool_name, "files.list");
}

#[test]
fn project_permission_store_saves_and_loads_deduplicated_toml() {
    let temp = temp_dir("permission-store");
    let store = ProjectPermissionStore::for_cwd(temp.clone());

    store
        .save(&ProjectGrants {
            shell_commands: vec!["git status".to_string(), "git status".to_string()],
            tool_names: vec!["files.read".to_string(), "files.list".to_string()],
            file_roots: vec![temp.display().to_string(), temp.display().to_string()],
        })
        .unwrap();

    let loaded = store.load();
    assert_eq!(loaded.shell_commands, vec!["git status".to_string()]);
    assert_eq!(
        loaded.tool_names,
        vec!["files.list".to_string(), "files.read".to_string()]
    );
    assert_eq!(loaded.file_roots, vec![temp.display().to_string()]);
    let text = fs::read_to_string(temp.join(".colibri/permissions.toml")).unwrap();
    assert!(text.contains("[shell]"));
    assert!(text.contains("commands = [\"git status\"]"));
}

#[test]
fn memory_context_bootstraps_and_loads_always_on_files() {
    let temp = temp_dir("memory-context");
    let mut config = AgentConfig::default();
    config.memory.root = temp.join("memory");
    let result = MemoryContext::new(config).load().unwrap();

    assert!(result.text.contains("Always-on memory:"));
    assert!(result.text.contains("[MEMORY.md]"));
    assert!(temp.join("memory/MEMORY.md").is_file());
    assert!(temp.join("memory/topics/sample.md").is_file());
}

#[test]
fn memory_context_disabled_returns_empty_and_does_not_bootstrap() {
    let temp = temp_dir("memory-disabled");
    let mut config = AgentConfig::default();
    config.memory.enabled = false;
    config.memory.root = temp.join("memory");

    let result = MemoryContext::new(config).load().unwrap();

    assert_eq!(result.text, "");
    assert!(result.files.is_empty());
    assert!(!temp.join("memory").exists());
}

#[test]
fn skill_run_executes_configured_command() {
    let temp = temp_dir("skill-run");
    let skill = temp.join("skills/release");
    fs::create_dir_all(skill.join("scripts")).unwrap();
    fs::write(skill.join("SKILL.md"), "# Release\n").unwrap();
    fs::write(skill.join("scripts/render.sh"), "printf rendered\n").unwrap();
    fs::write(
        skill.join("skill.toml"),
        r#"
[[commands]]
name = "render"
command = "sh"
args = ["scripts/render.sh"]
read_only = false
"#,
    )
    .unwrap();
    let mut config = AgentConfig::default();
    config.skills.dirs = vec![temp.join("skills")];
    let context = ToolContext::new(config, temp);

    let result = run_tool(
        "skill.run",
        r#"{"skill":"release","command":"render"}"#,
        &context,
    )
    .unwrap();

    assert!(result.ok);
    assert_eq!(result.text.trim(), "rendered");
}

#[test]
fn skill_index_includes_and_selects_builtin_create_colibri_skill_like_python() {
    let temp = temp_dir("builtin-skill");
    let mut config = AgentConfig::default();
    config.skills.dirs = vec![temp.join("missing-skills")];
    config.skills.max_loaded = 1;
    let context = ToolContext::new(config.clone(), temp);

    let (_unrelated_text, unrelated_skills, _truncated) =
        relevant_skill_context("hello status", &context);
    let (text, skills, truncated) = relevant_skill_context("帮我创建一个 colibri skill", &context);

    assert!(unrelated_skills.is_empty());
    assert_eq!(skills, vec!["create-colibri-skill".to_string()]);
    assert!(text.contains("[create-colibri-skill]"));
    assert!(text.contains("SKILL.md"));
    assert!(!truncated);
}

#[test]
fn skill_index_parses_metadata_and_loads_bounded_context_like_python() {
    let temp = temp_dir("skill-index");
    let skill_dir = temp.join("skills/release");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "# Release Notes\n\nrelease ".repeat(100),
    )
    .unwrap();
    fs::write(
        skill_dir.join("skill.toml"),
        r#"
description = "Release helper"

[[commands]]
name = "render"
description = "Render notes"
command = "python"
args = ["scripts/render.py"]
read_only = false
"#,
    )
    .unwrap();
    let mut config = AgentConfig::default();
    config.skills.dirs = vec![temp.join("skills")];
    config.skills.max_loaded = 1;
    config.skills.max_instruction_chars = 80;
    let index = SkillIndex::scan(&config.skills.dirs);
    let release = index.get("release").unwrap();
    let context = ToolContext::new(config, temp);

    let (text, skills, truncated) = relevant_skill_context("please write release notes", &context);

    assert_eq!(release.description, "Release helper");
    assert_eq!(release.commands[0].name, "render");
    assert_eq!(release.commands[0].description, "Render notes");
    assert_eq!(
        release.commands[0].args,
        vec!["scripts/render.py".to_string()]
    );
    assert!(!release.commands[0].read_only);
    assert!(text.starts_with("Relevant skills:"));
    assert!(text.contains("[release]"));
    assert!(text.contains("Base directory:"));
    assert_eq!(skills, vec!["release".to_string()]);
    assert!(truncated);
}

#[test]
fn openai_compatible_serializes_and_parses_tool_calls() {
    let _guard = env_lock().lock().unwrap();
    let server = start_http_server(|_base_url, _requests| {
        |_request| {
            TestHttpResponse::json(
                r#"{"choices":[{"message":{"content":null,"tool_calls":[{"id":"call_2","type":"function","function":{"name":"lookup","arguments":"{\"city\":\"Shanghai\"}"}}]}}]}"#,
            )
        }
    });
    let mut config = AgentConfig::default().model;
    config.provider = "openai_compatible".to_string();
    config.api_key = "test-key".to_string();
    config.model = "test-model".to_string();
    config.base_url = server.base_url.clone();
    let mut model = OpenAiCompatibleModel::from_config(&config).unwrap();
    let arguments = serde_json::json!({"path":"/tmp/a.txt"})
        .as_object()
        .unwrap()
        .clone();
    let assistant = Message {
        role: "assistant".to_string(),
        content: String::new(),
        tool_call_id: None,
        tool_calls: vec![ToolCall {
            id: "call_1".to_string(),
            name: "files.read".to_string(),
            arguments,
        }],
    };
    let tool = Message::tool("file contents", "call_1");

    let response = model
        .complete(
            &[assistant, tool],
            &colibri_rust::tools::tool_specs(),
            "system prompt",
            &ModelLimits {
                timeout_seconds: 5,
                max_output_tokens: 20,
            },
        )
        .unwrap();

    assert_eq!(response.text, "");
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].id, "call_2");
    assert_eq!(response.tool_calls[0].name, "lookup");
    assert_eq!(
        response.tool_calls[0].arguments.get("city"),
        Some(&serde_json::Value::String("Shanghai".to_string()))
    );
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/chat/completions");
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(body.contains("\"role\":\"system\""));
    assert!(body.contains("\"tool_call_id\":\"call_1\""));
    assert!(body.contains("\"tool_calls\""));
    assert!(body.contains("\"files.read\""));
    assert!(body.contains("\"tools\""));
}

#[test]
fn model_factory_rejects_unknown_provider() {
    let mut config = AgentConfig::default().model;
    config.provider = "mystery".to_string();

    let error = match colibri_rust::model::build_model(&config) {
        Ok(_) => panic!("unknown provider unexpectedly succeeded"),
        Err(error) => error,
    };

    assert!(error.contains("Unsupported model provider: mystery"));
}

#[test]
fn openai_compatible_requires_api_key_or_environment() {
    let _guard = env_lock().lock().unwrap();
    let old_key = std::env::var_os("COLIBRI_API_KEY");
    std::env::remove_var("COLIBRI_API_KEY");
    let mut config = AgentConfig::default().model;
    config.provider = "openai_compatible".to_string();
    config.api_key.clear();

    let error = match OpenAiCompatibleModel::from_config(&config) {
        Ok(_) => panic!("missing API key unexpectedly succeeded"),
        Err(error) => error,
    };

    restore_env_var("COLIBRI_API_KEY", old_key);
    assert!(error.contains("model.api_key or COLIBRI_API_KEY is required"));
}

#[test]
fn rust_http_runtime_does_not_spawn_curl() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in fs::read_dir(src).unwrap().flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("rs") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        let legacy_process_call = ["Command::new(\"", "curl", "\")"].concat();
        let legacy_helper_call = ["curl", "_json"].concat();
        assert!(
            !text.contains(&legacy_process_call) && !text.contains(&legacy_helper_call),
            "{} still contains curl runtime wiring",
            path.display()
        );
    }
}

#[test]
fn web_search_posts_baidu_request_and_formats_references() {
    let _guard = env_lock().lock().unwrap();
    let temp = temp_dir("web-search");
    let server = start_http_server(|_base_url, _requests| {
        |_request| {
            TestHttpResponse::json(
                r#"{"references":[{"title":"杭州天气","url":"https://example.test/weather","snippet":"drop","summary":"晴"}]}"#,
            )
        }
    });

    let mut config = AgentConfig::default();
    config.web_search.api_key = "search-key".to_string();
    config.web_search.endpoint = format!("{}/search", server.base_url);
    config.web_search.timeout_seconds = 3;
    config.web_search.max_results = 7;
    let context = ToolContext::new(config, temp.clone());

    let result = run_tool(
        "web.search",
        r#"{"query":"杭州天气","count":"2","freshness":"pd"}"#,
        &context,
    )
    .unwrap();

    assert!(result.ok);
    assert!(result.text.contains("杭州天气"));
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/search");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["resource_type_filter"][0]["top_k"], 2);
    assert_eq!(body["messages"][0]["content"], "杭州天气");
    assert!(body.get("search_filter").is_some());
    assert!(result.text.contains("summary"));
    assert!(!result.text.contains("snippet"));
}

#[test]
fn web_search_requires_configured_baidu_api_key() {
    let temp = temp_dir("web-search-missing-key");
    let mut config = AgentConfig::default();
    config.web_search.api_key.clear();
    let context = ToolContext::new(config, temp);

    let result = run_tool("web.search", r#"{"query":"hello"}"#, &context).unwrap();

    assert!(!result.ok);
    assert_eq!(result.error_type.as_deref(), Some("invalid_config"));
    assert!(result.text.contains("Missing Baidu web search API key"));
}

#[test]
fn session_writes_transcript_events() {
    let _guard = env_lock().lock().unwrap();
    let temp = temp_dir("session-transcript");
    let old_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &temp);
    let mut config = AgentConfig::default();
    config.session.transcript = true;
    config.memory.root = temp.join("memory");
    let mut session = AgentSession::new(config, Box::new(FakeModel::new()));

    let response = session.submit("hello").unwrap();

    restore_home(old_home);
    assert_eq!(response.text, "fake: hello");
    let transcript_dir = temp.join(".colibri/transcripts");
    let entries = fs::read_dir(transcript_dir)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(entries.len(), 1);
    let text = fs::read_to_string(entries[0].path()).unwrap();
    assert!(text.contains("\"type\":\"user_message\""));
    assert!(text.contains("\"payload\":{\"text\":\"hello\""));
    assert!(text.contains("\"type\":\"assistant_message\""));
    assert!(text.contains("\"ts\":\""));
}

#[test]
fn gateway_session_transcript_injects_channel_metadata_like_python() {
    let _guard = env_lock().lock().unwrap();
    let temp = temp_dir("gateway-transcript-metadata");
    let old_home = std::env::var_os("COLIBRI_HOME");
    std::env::set_var("COLIBRI_HOME", temp.join("home"));
    let mut config = AgentConfig::default();
    config.session.transcript = true;
    config.memory.enabled = false;
    config.gateway.max_sessions = 2;
    let mut cache = GatewaySessionCache::new(config).unwrap();
    {
        let session = cache
            .get_or_create_with_metadata(
                "weixin:user-1",
                BTreeMap::from([
                    ("channel".to_string(), "weixin".to_string()),
                    ("sender_id".to_string(), "user-1".to_string()),
                    ("session_key".to_string(), "weixin:user-1".to_string()),
                ]),
            )
            .unwrap();
        let response = session.submit("hi").unwrap();
        assert_eq!(response.text, "fake: hi");
    }
    restore_env_var("COLIBRI_HOME", old_home);

    let transcript_dir = temp.join("home/transcripts");
    let entries = fs::read_dir(transcript_dir)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(entries.len(), 1);
    let text = fs::read_to_string(entries[0].path()).unwrap();
    assert!(text.contains("\"type\":\"user_message\""));
    assert!(text.contains("\"text\":\"hi\""));
    assert!(text.contains("\"channel\":\"weixin\""));
    assert!(text.contains("\"sender_id\":\"user-1\""));
    assert!(text.contains("\"session_key\":\"weixin:user-1\""));
}

#[test]
fn transcript_history_loader_restores_complete_turns_and_strips_attachment_paths_like_python() {
    let temp = temp_dir("history-loader");
    let transcript = temp.join("transcripts/2026-07-10.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(
        &transcript,
        [
            "not-json".to_string(),
            event_json(
                "user_message",
                "请分析图片\n\nAttachments saved locally:\n1. image: a.png at /tmp/colibri/media/a.png, content_type=image/png",
                &[],
            ),
            event_json("assistant_message", "", &[("tool_call_count", "1")]),
            event_json("assistant_message", "图片内容", &[("tool_call_count", "0")]),
            event_json("user_message", "尚未回答", &[]),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();

    let messages = TranscriptHistoryLoader::new(temp, 24, 24000, 2 * 1024 * 1024).load();

    assert_eq!(
        messages
            .iter()
            .map(|message| (message.role.as_str(), message.content.as_str()))
            .collect::<Vec<_>>(),
        vec![("user", "请分析图片"), ("assistant", "图片内容")]
    );
}

#[test]
fn transcript_history_loader_pairs_each_source_by_completion_order_like_python() {
    let temp = temp_dir("history-loader-source");
    let transcript = temp.join("transcripts/2026-07-10.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(
        &transcript,
        [
            event_json(
                "user_message",
                "微信问题",
                &[("session_key", "weixin:user")],
            ),
            event_json("user_message", "REPL 问题", &[]),
            event_json(
                "assistant_message",
                "REPL 回答",
                &[("tool_call_count", "0")],
            ),
            event_json(
                "assistant_message",
                "微信回答",
                &[("tool_call_count", "0"), ("session_key", "weixin:user")],
            ),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();

    let messages = TranscriptHistoryLoader::new(temp, 24, 24000, 2 * 1024 * 1024).load();

    assert_eq!(
        messages
            .chunks(2)
            .map(|pair| (pair[0].content.as_str(), pair[1].content.as_str()))
            .collect::<Vec<_>>(),
        vec![("REPL 问题", "REPL 回答"), ("微信问题", "微信回答")]
    );
}

#[test]
fn auth_weixin_uses_native_http_and_saves_token_without_printing_secret() {
    let _guard = env_lock().lock().unwrap();
    let temp = temp_dir("weixin-auth");
    let server = start_http_server(|_base_url, _requests| {
        |request| {
            if request.path.starts_with("/ilink/bot/get_bot_qrcode") {
                TestHttpResponse::json(r#"{"qrcode_img_content":"qr-payload","qrcode":"qr-1"}"#)
            } else {
                TestHttpResponse::json(
                    r#"{"status":"confirmed","bot_token":"secret-token","ilink_bot_id":"account-1","ilink_user_id":"user-1","baseurl":"https://redirect.weixin.test/"}"#,
                )
            }
        }
    });
    let config_path = temp.join("config.toml");
    fs::write(
        &config_path,
        format!(
            r#"
[model]
provider = "fake"

[channels.weixin]
base_url = "{}"
enabled = false
allow_from = ["user-1"]
poll_timeout_seconds = 20
"#,
            server.base_url
        ),
    )
    .unwrap();

    let args = vec![
        "--config".to_string(),
        config_path.display().to_string(),
        "auth".to_string(),
        "weixin".to_string(),
    ];
    let (code, stdout, stderr) = run_cli_raw(&args, "");

    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("Weixin auth succeeded."));
    assert!(stdout.contains("██"));
    assert!(!stdout.contains("secret-token"));
    let saved = fs::read_to_string(config_path).unwrap();
    assert!(saved.contains("[channels.weixin]"));
    assert!(saved.contains("enabled = true"));
    assert!(saved.contains("token = \"secret-token\""));
    assert!(saved.contains("base_url = \"https://redirect.weixin.test/\""));
    assert!(saved.contains("allow_from = [\"user-1\"]"));
}

#[test]
fn terminal_qr_outputs_block_qr_for_weixin_payload() {
    let rendered = render_terminal_qr(
        "https://liteapp.weixin.qq.com/q/7GiQu1?qrcode=4b69ff82f873485e97acae885b11437c&bot_type=3",
    )
    .unwrap();

    assert!(rendered.contains("██"));
    assert_eq!(rendered.lines().count(), 41);
}

#[test]
fn terminal_qr_returns_none_for_large_payload() {
    assert!(render_terminal_qr(&"x".repeat(200)).is_none());
}

#[test]
fn gateway_status_reports_stale_state_file() {
    let temp = temp_dir("gateway-status");
    let state_path = temp.join("gateway.json");
    let log_path = temp.join("gateway.log");
    fs::write(
        &state_path,
        format!(
            "{{\"pid\":999999,\"config\":\"default\",\"cwd\":\"{}\",\"log\":\"{}\",\"started_at\":\"2026-07-09T08:00:00+08:00\"}}",
            temp.display(),
            log_path.display()
        ),
    )
    .unwrap();

    let status = GatewayStatus::from_paths(state_path.clone(), log_path.clone());
    let lines = format_gateway_status(&status);

    assert!(!status.running);
    assert_eq!(
        lines,
        vec![
            "running=false".to_string(),
            "pid=999999".to_string(),
            "rss_kb=unknown".to_string(),
            "config=default".to_string(),
            format!("cwd={}", temp.display()),
            format!("log={}", log_path.display()),
            format!("state={}", state_path.display()),
            "started_at=2026-07-09T08:00:00+08:00".to_string(),
            "reason=not_running".to_string(),
        ]
    );
}

#[test]
fn gateway_session_cache_reuses_and_evicts_oldest_like_python() {
    let mut config = AgentConfig::default();
    config.gateway.max_sessions = 1;
    config.gateway.session_idle_seconds = 0;
    let mut cache = GatewaySessionCache::new(config).unwrap();

    cache.get_or_create("weixin:user-1").unwrap();
    assert_eq!(cache.len(), 1);
    assert!(cache.contains_key("weixin:user-1"));
    cache.get_or_create("weixin:user-1").unwrap();
    assert_eq!(cache.len(), 1);
    cache.get_or_create("weixin:user-2").unwrap();

    assert_eq!(cache.len(), 1);
    assert!(!cache.contains_key("weixin:user-1"));
    assert!(cache.contains_key("weixin:user-2"));
}

#[test]
fn memory_write_rejects_traversal_like_python() {
    let temp = temp_dir("memory-write-traversal");
    let mut config = AgentConfig::default();
    config.memory.root = temp.join("memory");
    let context = ToolContext::new(config, temp.clone());

    let result = run_tool(
        "memory.write",
        r#"{"file":"../escaped.md","content":"secret","mode":"replace"}"#,
        &context,
    )
    .unwrap();

    assert!(!result.ok);
    assert_eq!(result.error_type.as_deref(), Some("invalid_arguments"));
    assert_eq!(result.text, "Invalid memory file");
    assert!(!temp.join("escaped.md").exists());
}

#[test]
fn memory_write_supports_topic_and_python_result_text() {
    let temp = temp_dir("memory-write-topic");
    let mut config = AgentConfig::default();
    config.memory.root = temp.join("memory");
    let context = ToolContext::new(config, temp);

    let result = run_tool(
        "memory.write",
        r#"{"topic":"project_alpha","content":"details","mode":"replace"}"#,
        &context,
    )
    .unwrap();

    assert!(result.ok, "{result:?}");
    assert_eq!(
        fs::read_to_string(context.config.memory.root.join("topics/project_alpha.md")).unwrap(),
        "details\n"
    );
    assert_eq!(
        result.text,
        "Updated memory file: topics/project_alpha.md\nRemember to update INDEX.md so this topic can be found by memory.search."
    );
}

#[test]
fn shell_timeout_terminates_without_waiting_for_natural_exit_like_python() {
    let temp = temp_dir("shell-real-timeout");
    let mut config = AgentConfig::default();
    config.tools.max_shell_seconds = 0.05;
    let context = ToolContext::new(config, temp);

    let started = Instant::now();
    let result = run_tool("shell.run", r#"{"command":"sleep 1"}"#, &context).unwrap();

    assert!(!result.ok);
    assert_eq!(result.error_type.as_deref(), Some("timeout"));
    assert_eq!(result.text, "Command timed out");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "timeout returned too late: {:?}",
        started.elapsed()
    );
}

#[test]
fn nonzero_shell_exit_uses_python_error_type() {
    let temp = temp_dir("shell-nonzero-type");
    let config = AgentConfig::default();
    let context = ToolContext::new(config, temp);

    let result = run_tool("shell.run", r#"{"command":"false"}"#, &context).unwrap();

    assert!(!result.ok);
    assert_eq!(result.error_type.as_deref(), Some("nonzero_exit"));
}

#[test]
fn transcript_total_budget_is_applied_by_session_like_python() {
    let _guard = env_lock().lock().unwrap();
    let temp = temp_dir("transcript-total-budget");
    let old_home = std::env::var_os("COLIBRI_HOME");
    std::env::set_var("COLIBRI_HOME", &temp);
    let transcript_dir = temp.join("transcripts");
    fs::create_dir_all(&transcript_dir).unwrap();
    let old_path = transcript_dir.join("2000-01-01.jsonl");
    fs::write(&old_path, "x".repeat(1024)).unwrap();

    let mut config = AgentConfig::default();
    config.memory.enabled = false;
    config.session.transcript = true;
    config.session.transcript_retention_days = 0;
    config.session.transcript_max_total_bytes = 1;
    let mut session = AgentSession::new(config, Box::new(FakeModel::new()));
    session.submit("hello").unwrap();

    restore_env_var("COLIBRI_HOME", old_home);
    assert!(!old_path.exists(), "old transcript was not removed");
}

#[test]
fn transcript_writer_removes_expired_files_but_preserves_active_like_python() {
    let temp = temp_dir("transcript-retention-expired");
    let directory = temp.join("transcripts");
    fs::create_dir_all(&directory).unwrap();
    let expired = directory.join("2026-01-01.jsonl");
    let recent = directory.join("2026-07-09.jsonl");
    let active = directory.join("2026-07-10.jsonl");
    fs::write(&expired, "expired").unwrap();
    fs::write(&recent, "recent").unwrap();
    fs::write(&active, "active").unwrap();
    set_old_mtime(&expired);

    let mut writer = TranscriptWriter::new(active.clone(), BTreeMap::new(), 30, 0).unwrap();
    writer.close();

    assert!(!expired.exists());
    assert!(recent.exists());
    assert!(active.exists());
}

#[test]
fn transcript_writer_removes_oldest_inactive_files_to_fit_size_limit_like_python() {
    let temp = temp_dir("transcript-retention-size");
    let directory = temp.join("transcripts");
    fs::create_dir_all(&directory).unwrap();
    let oldest = directory.join("2026-07-08.jsonl");
    let newest = directory.join("2026-07-09.jsonl");
    let active = directory.join("2026-07-10.jsonl");
    fs::write(&oldest, "a".repeat(10)).unwrap();
    fs::write(&newest, "b".repeat(10)).unwrap();

    let mut writer = TranscriptWriter::new(active.clone(), BTreeMap::new(), 0, 15).unwrap();
    writer.close();

    assert!(!oldest.exists());
    assert!(newest.exists());
    assert!(active.exists());
}

#[test]
fn tool_schemas_match_python_property_types_and_required_fields() {
    let config = AgentConfig::default();
    let specs = colibri_rust::tools::tool_specs_for_config(&config);
    let by_name = |name: &str| {
        specs
            .iter()
            .find(|spec| spec["function"]["name"] == name)
            .unwrap()
    };

    let files_list = by_name("files.list");
    assert_eq!(
        files_list["function"]["parameters"]["required"],
        serde_json::json!(["path"])
    );
    let web = by_name("web.search");
    assert_eq!(
        web["function"]["parameters"]["properties"]["count"]["type"],
        "integer"
    );
    assert_eq!(web["function"]["parameters"]["additionalProperties"], false);
    let memory_write = by_name("memory.write");
    assert!(memory_write["function"]["parameters"]["properties"]["topic"].is_object());
    assert_eq!(
        memory_write["function"]["parameters"]["required"],
        serde_json::json!(["content"])
    );
}

#[test]
fn web_search_rejects_invalid_freshness_before_network_like_python() {
    let temp = temp_dir("web-invalid-freshness");
    let mut config = AgentConfig::default();
    config.web_search.api_key = "test-key".to_string();
    config.web_search.endpoint = "http://127.0.0.1:9/unreachable".to_string();
    config.web_search.timeout_seconds = 1;
    let context = ToolContext::new(config, temp);

    let result = run_tool(
        "web.search",
        r#"{"query":"hello","freshness":"yesterday"}"#,
        &context,
    )
    .unwrap();

    assert!(!result.ok);
    assert_eq!(result.error_type.as_deref(), Some("invalid_arguments"));
    assert_eq!(
        result.text,
        "web.search freshness must be pd, pw, pm, py, or YYYY-MM-DDtoYYYY-MM-DD"
    );
}

#[test]
fn config_rejects_unknown_nested_field_instead_of_silently_ignoring_it() {
    let temp = temp_dir("config-unknown-field");
    let path = temp.join("config.toml");
    fs::write(&path, "[model]\nunknown_option = true\n").unwrap();

    let error = AgentConfig::load(Some(&path)).unwrap_err();

    assert!(error.contains("model.unknown_option"), "{error}");
}

#[test]
fn config_rejects_legacy_model_input_char_limit_like_python() {
    let temp = temp_dir("config-legacy-input-char-limit");
    let path = temp.join("config.toml");
    fs::write(&path, "[session]\nmodel_input_char_limit = 192000\n").unwrap();

    let error = AgentConfig::load(Some(&path)).unwrap_err();

    assert!(error.contains("session.model_input_char_limit"), "{error}");
}

#[test]
fn config_rejects_legacy_model_input_byte_limit_like_python() {
    let temp = temp_dir("legacy-input-byte-limit-config");
    let path = temp.join("agent.toml");
    fs::write(&path, "[model]\ninput_byte_limit = 192000\n").unwrap();

    let error = AgentConfig::load(Some(&path)).unwrap_err().to_string();

    assert!(error.contains("model.input_byte_limit"), "{error}");
}

#[test]
fn memory_bootstrap_content_and_per_file_limits_match_python() {
    let temp = temp_dir("memory-bootstrap-exact");
    let mut config = AgentConfig::default();
    config.memory.root = temp.join("memory");
    config.memory.max_recall_chars = 10_000;
    let first = MemoryContext::new(config.clone()).load().unwrap();

    let memory_template = fs::read_to_string(config.memory.root.join("MEMORY.md")).unwrap();
    assert!(memory_template
        .contains("description: Colibri 长期事实和项目上下文；首次真实写入时直接覆盖样例文本"));
    assert!(memory_template.contains("updated: 2026-07-09"));
    assert!(memory_template.contains("修改规则"));
    assert!(first.text.contains("Always-on memory:\n\n[MEMORY.md]"));

    fs::write(config.memory.root.join("MEMORY.md"), "M".repeat(2_000)).unwrap();
    fs::write(config.memory.root.join("USER.md"), "U".repeat(800)).unwrap();
    let bounded = MemoryContext::new(config).load().unwrap();
    let memory_block = bounded
        .text
        .split("[MEMORY.md]\n")
        .nth(1)
        .unwrap()
        .split("\n\n[USER.md]")
        .next()
        .unwrap();
    let user_block = bounded.text.split("[USER.md]\n").nth(1).unwrap();
    assert_eq!(memory_block.chars().count(), 1_800);
    assert_eq!(user_block.chars().count(), 600);
    assert!(memory_block.ends_with("\n...[truncated]"));
    assert!(user_block.ends_with("\n...[truncated]"));
    assert!(bounded.truncated);
}

#[test]
fn openai_tool_arguments_preserve_json_value_types_like_python() {
    let _guard = env_lock().lock().unwrap_or_else(|error| error.into_inner());
    let server = start_http_server(|_base_url, _requests| {
        |_request| {
            TestHttpResponse::json(
                r#"{"choices":[{"message":{"content":null,"tool_calls":[{"id":"call","function":{"name":"web.search","arguments":"{\"count\":2,\"filters\":{\"fresh\":true}}"}}]}}]}"#,
            )
        }
    });
    let mut config = AgentConfig::default().model;
    config.provider = "openai_compatible".to_string();
    config.api_key = "test-key".to_string();
    config.base_url = server.base_url;
    let mut model = OpenAiCompatibleModel::from_config(&config).unwrap();

    let response = model
        .complete(
            &[],
            &[],
            "",
            &ModelLimits {
                timeout_seconds: 2,
                max_output_tokens: 10,
            },
        )
        .unwrap();

    assert_eq!(response.tool_calls[0].arguments["count"].as_i64(), Some(2));
    assert_eq!(
        response.tool_calls[0].arguments["filters"]["fresh"].as_bool(),
        Some(true)
    );
}

#[test]
fn gateway_stop_refuses_unverified_pid_like_python() {
    let _guard = env_lock().lock().unwrap_or_else(|error| error.into_inner());
    let temp = temp_dir("gateway-refuse-unverified-pid");
    let old_home = std::env::var_os("COLIBRI_HOME");
    std::env::set_var("COLIBRI_HOME", &temp);
    fs::create_dir_all(temp.join("run")).unwrap();
    let mut child = std::process::Command::new("sleep")
        .arg("5")
        .spawn()
        .unwrap();
    fs::write(
        temp.join("run/gateway.json"),
        format!("{{\"pid\":{},\"config\":\"default\"}}\n", child.id()),
    )
    .unwrap();

    let result = colibri_rust::gateway::stop_gateway();
    std::thread::sleep(Duration::from_millis(50));
    let still_running = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();
    restore_env_var("COLIBRI_HOME", old_home);

    let status = result.unwrap();
    assert_eq!(status.reason, "unverified_pid");
    assert!(
        still_running,
        "stop_gateway terminated an unrelated process"
    );
}

#[test]
fn weixin_updates_parse_text_and_media_with_context_like_python() {
    let temp = temp_dir("weixin-parse-media");
    let image_path = temp.join("photo.png");
    fs::write(&image_path, b"image").unwrap();
    let mut config = AgentConfig::default();
    config.channels_weixin.allow_from = vec!["user-1".to_string()];
    let body = r#"{
        "get_updates_buf":"next",
        "msgs":[
          {"message_type":1,"message_state":2,"from_user_id":"user-1","message_id":"m1","context_token":"ctx","item_list":[{"type":1,"text_item":{"text":"hello"}}]},
          {"message_type":1,"message_state":2,"from_user_id":"user-1","message_id":"m2","context_token":"ctx","item_list":[{"type":2,"image_item":{"file_name":"photo.png","media":{"full_url":"https://example.test/photo"}}}]}
        ]
    }"#;

    let (next, messages) = parse_weixin_updates(&config, body, |_item| {
        Ok(MediaPart::new(
            "image",
            image_path.clone(),
            "photo.png",
            "image/png",
            "",
        ))
    })
    .unwrap();

    assert_eq!(next, "next");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].text, "hello");
    assert!(messages[0].media.is_empty());
    assert_eq!(messages[1].text, "[image: photo.png]");
    assert_eq!(messages[1].message_id, "m2");
    assert_eq!(messages[1].context_token, "ctx");
    assert_eq!(messages[1].media[0].path, image_path);
}

#[test]
fn weixin_updates_keep_text_when_media_download_fails_like_python() {
    let config = AgentConfig::default();
    let body = r#"{"msgs":[{"message_type":1,"message_state":2,"from_user_id":"user","item_list":[{"type":1,"text_item":{"text":"keep me"}},{"type":4,"file_item":{"file_name":"bad.bin","media":{"full_url":"https://bad"}}}]}]}"#;

    let (_, messages) =
        parse_weixin_updates(&config, body, |_item| Err("download failed".to_string())).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "keep me");
    assert!(messages[0].media.is_empty());
}

#[test]
fn weixin_aes_ecb_pkcs7_round_trip_matches_python() {
    let key = *b"0123456789abcdef";
    let plaintext = b"hello weixin media";

    let encrypted = encrypt_aes_ecb(plaintext, &key).unwrap();
    let decrypted = decrypt_aes_ecb(&encrypted, &key).unwrap();

    assert_ne!(encrypted, plaintext);
    assert_eq!(encrypted.len() % 16, 0);
    assert_eq!(decrypted, plaintext);
}

#[test]
fn weixin_media_cleanup_removes_oldest_files_to_fit_budget_like_python() {
    let temp = temp_dir("weixin-media-cleanup");
    let first = temp.join("a.bin");
    let second = temp.join("b.bin");
    fs::write(&first, b"1234").unwrap();
    std::thread::sleep(Duration::from_millis(20));
    fs::write(&second, b"5678").unwrap();

    cleanup_media_directory(&temp, u64::MAX, 4);

    assert!(!first.exists());
    assert!(second.exists());
}

#[test]
fn weixin_permission_reply_aliases_match_python() {
    for (reply, expected) in [
        ("yes", "y"),
        ("session", "s"),
        ("executable-session", "e"),
        ("project", "p"),
        ("deny", "n"),
        ("something else", "n"),
    ] {
        assert_eq!(permission_choice(reply), expected);
    }
}

#[test]
fn weixin_download_inbound_media_decrypts_and_stores_file_like_python() {
    let _guard = env_lock().lock().unwrap_or_else(|error| error.into_inner());
    let key = *b"0123456789abcdef";
    let cipher = encrypt_aes_ecb(b"downloaded content", &key).unwrap();
    let server = start_http_server(move |_base_url, _requests| {
        move |_request| TestHttpResponse::bytes(cipher.clone())
    });
    let mut config = AgentConfig::default();
    config.channels_weixin.token = "token".to_string();
    let item = serde_json::json!({
        "type":4,
        "file_item":{
            "file_name":"report.txt",
            "media":{
                "full_url":format!("{}/download", server.base_url),
                "aes_key":"MDEyMzQ1Njc4OWFiY2RlZg=="
            }
        }
    });

    let media = download_inbound_media(&config, &item).unwrap();

    assert_eq!(media.media_type, "file");
    assert_eq!(media.filename, "report.txt");
    assert_eq!(media.content_type, "text/plain");
    assert_eq!(fs::read(&media.path).unwrap(), b"downloaded content");
    let _ = fs::remove_file(media.path);
}

#[test]
fn weixin_send_media_encrypts_uploads_and_sends_metadata_like_python() {
    let _guard = env_lock().lock().unwrap_or_else(|error| error.into_inner());
    let temp = temp_dir("weixin-send-media");
    let server = start_http_server(|base_url, _requests| {
        move |request| {
            if request.path.contains("getuploadurl") {
                TestHttpResponse::json(&format!(r#"{{"upload_full_url":"{}/upload"}}"#, base_url))
            } else if request.path == "/upload" {
                TestHttpResponse {
                    status: 200,
                    headers: vec![(
                        "X-Encrypted-Param".to_string(),
                        "encrypted-param".to_string(),
                    )],
                    body: Vec::new(),
                }
            } else {
                TestHttpResponse::json(r#"{"ret":0}"#)
            }
        }
    });
    let file = temp.join("report.txt");
    fs::write(&file, b"plain content").unwrap();
    let mut config = AgentConfig::default();
    config.channels_weixin.token = "token".to_string();
    config.channels_weixin.base_url = server.base_url.clone();
    let media = MediaPart::new("file", file, "report.txt", "text/plain", "");

    send_weixin_media(&config, "user-1", "ctx", &media).unwrap();

    let requests = server.requests.lock().unwrap();
    let encrypted = requests
        .iter()
        .find(|request| request.path == "/upload")
        .unwrap()
        .body
        .clone();
    assert_ne!(encrypted, b"plain content");
    assert_eq!(encrypted.len() % 16, 0);
    let body: serde_json::Value = serde_json::from_slice(
        &requests
            .iter()
            .find(|request| request.path.contains("sendmessage"))
            .unwrap()
            .body,
    )
    .unwrap();
    assert_eq!(body["msg"]["to_user_id"], "user-1");
    assert_eq!(body["msg"]["context_token"], "ctx");
    assert_eq!(
        body["msg"]["item_list"][0]["file_item"]["media"]["encrypt_query_param"],
        "encrypted-param"
    );
    assert_eq!(
        body["msg"]["item_list"][0]["file_item"]["file_name"],
        "report.txt"
    );
}

#[test]
fn weixin_send_text_uses_unique_client_id_like_python() {
    let server = start_http_server(|_base_url, _requests| |_| TestHttpResponse::json(r#"{"ret":0}"#));
    let mut config = AgentConfig::default();
    config.channels_weixin.token = "token".to_string();
    config.channels_weixin.base_url = server.base_url.clone();

    send_weixin_text(&config, "user-1", "ctx-1", "first reply").unwrap();
    send_weixin_text(&config, "user-1", "ctx-1", "second reply").unwrap();

    let requests = server.requests.lock().unwrap();
    let client_ids: Vec<String> = requests
        .iter()
        .filter(|request| request.path.contains("sendmessage"))
        .map(|request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            body["msg"]["client_id"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(client_ids.len(), 2);
    assert!(client_ids[0].starts_with("colibri-"));
    assert!(client_ids[1].starts_with("colibri-"));
    assert_ne!(
        client_ids[0], client_ids[1],
        "reusing client_id causes Weixin to drop later replies"
    );
    assert_ne!(client_ids[0], format!("colibri-{}", std::process::id()));
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let path =
        std::env::temp_dir().join(format!("colibri-rust-test-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn event_json(event_type: &str, text: &str, fields: &[(&str, &str)]) -> String {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "text".to_string(),
        serde_json::Value::String(text.to_string()),
    );
    for (key, value) in fields {
        let json_value = value
            .parse::<u64>()
            .map(serde_json::Value::from)
            .unwrap_or_else(|_| serde_json::Value::String((*value).to_string()));
        payload.insert((*key).to_string(), json_value);
    }
    serde_json::json!({
        "ts": "2026-07-10T08:00:00+08:00",
        "type": event_type,
        "payload": payload,
    })
    .to_string()
}

fn set_old_mtime(path: &Path) {
    std::process::Command::new("touch")
        .arg("-t")
        .arg("200001010000")
        .arg(path)
        .status()
        .expect("touch old mtime");
}

struct FakePermissionPrompter {
    replies: Vec<String>,
    requests: Vec<PermissionRequest>,
}

impl FakePermissionPrompter {
    fn new(replies: Vec<&str>) -> Self {
        Self {
            replies: replies.into_iter().map(ToString::to_string).rev().collect(),
            requests: Vec::new(),
        }
    }
}

impl PermissionPrompter for FakePermissionPrompter {
    fn confirm(&mut self, request: PermissionRequest) -> String {
        self.requests.push(request);
        self.replies.pop().unwrap_or_else(|| "n".to_string())
    }
}

struct CompactScriptModel {
    calls: usize,
}

impl CompactScriptModel {
    fn new() -> Self {
        Self { calls: 0 }
    }
}

impl ModelClient for CompactScriptModel {
    fn complete(
        &mut self,
        messages: &[Message],
        _tools: &[serde_json::Value],
        system: &str,
        _limits: &ModelLimits,
    ) -> Result<colibri_rust::messages::ModelResponse, String> {
        self.calls += 1;
        if system.contains("summarizing conversations") {
            assert_eq!(messages.len(), 1);
            assert!(messages[0].content.contains("latest request"));
            return Ok(colibri_rust::messages::ModelResponse {
                text: "<analysis>private reasoning</analysis><summary>important compacted context</summary>"
                    .to_string(),
                tool_calls: Vec::new(),
            });
        }
        Ok(colibri_rust::messages::ModelResponse {
            text: "done".to_string(),
            tool_calls: Vec::new(),
        })
    }
}

struct FailingCompactModel {
    calls: usize,
}

impl FailingCompactModel {
    fn new() -> Self {
        Self { calls: 0 }
    }
}

impl ModelClient for FailingCompactModel {
    fn complete(
        &mut self,
        _messages: &[Message],
        _tools: &[serde_json::Value],
        system: &str,
        _limits: &ModelLimits,
    ) -> Result<colibri_rust::messages::ModelResponse, String> {
        self.calls += 1;
        if system.contains("summarizing conversations") {
            return Err("compact boom".to_string());
        }
        Ok(colibri_rust::messages::ModelResponse {
            text: format!("ok-{}", self.calls),
            tool_calls: Vec::new(),
        })
    }
}

struct AlwaysToolModel;

impl AlwaysToolModel {
    fn new() -> Self {
        Self
    }
}

impl ModelClient for AlwaysToolModel {
    fn complete(
        &mut self,
        _messages: &[Message],
        _tools: &[serde_json::Value],
        _system: &str,
        _limits: &ModelLimits,
    ) -> Result<colibri_rust::messages::ModelResponse, String> {
        Ok(colibri_rust::messages::ModelResponse {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "call_loop".to_string(),
                name: "files.list".to_string(),
                arguments: {
                    let mut map = serde_json::Map::new();
                    map.insert("path".to_string(), serde_json::Value::String(".".to_string()));
                    map
                },
            }],
        })
    }
}

struct PlainTextModel;

impl ModelClient for PlainTextModel {
    fn complete(
        &mut self,
        _messages: &[Message],
        _tools: &[serde_json::Value],
        _system: &str,
        _limits: &ModelLimits,
    ) -> Result<colibri_rust::messages::ModelResponse, String> {
        Ok(colibri_rust::messages::ModelResponse {
            text: "done".to_string(),
            tool_calls: Vec::new(),
        })
    }
}

struct TwoToolsThenTextModel {
    calls: usize,
}

impl TwoToolsThenTextModel {
    fn new() -> Self {
        Self { calls: 0 }
    }
}

impl ModelClient for TwoToolsThenTextModel {
    fn complete(
        &mut self,
        _messages: &[Message],
        _tools: &[serde_json::Value],
        _system: &str,
        _limits: &ModelLimits,
    ) -> Result<colibri_rust::messages::ModelResponse, String> {
        self.calls += 1;
        if self.calls == 1 {
            let mut args = serde_json::Map::new();
            args.insert("path".to_string(), serde_json::Value::String(".".to_string()));
            return Ok(colibri_rust::messages::ModelResponse {
                text: String::new(),
                tool_calls: vec![
                    ToolCall {
                        id: "call_a".to_string(),
                        name: "files.list".to_string(),
                        arguments: args.clone(),
                    },
                    ToolCall {
                        id: "call_b".to_string(),
                        name: "files.list".to_string(),
                        arguments: args,
                    },
                ],
            });
        }
        Ok(colibri_rust::messages::ModelResponse {
            text: "steered-ok".to_string(),
            tool_calls: Vec::new(),
        })
    }
}

struct SteerDuringTextOnlyModel {
    handle_slot: Arc<Mutex<Option<SteerHandle>>>,
    calls: usize,
}

impl ModelClient for SteerDuringTextOnlyModel {
    fn complete(
        &mut self,
        _messages: &[Message],
        _tools: &[serde_json::Value],
        _system: &str,
        _limits: &ModelLimits,
    ) -> Result<colibri_rust::messages::ModelResponse, String> {
        self.calls += 1;
        if self.calls == 1 {
            let handle = self
                .handle_slot
                .lock()
                .unwrap()
                .clone()
                .expect("steer handle");
            assert!(handle.steer("change plan"));
            return Ok(colibri_rust::messages::ModelResponse {
                text: "almost done".to_string(),
                tool_calls: Vec::new(),
            });
        }
        Ok(colibri_rust::messages::ModelResponse {
            text: "steered-ok".to_string(),
            tool_calls: Vec::new(),
        })
    }
}

struct BudgetInspectModel;

impl ModelClient for BudgetInspectModel {
    fn complete(
        &mut self,
        messages: &[Message],
        _tools: &[serde_json::Value],
        _system: &str,
        _limits: &ModelLimits,
    ) -> Result<colibri_rust::messages::ModelResponse, String> {
        assert!(messages.iter().any(|message| message.role == "user"
            && message.content == "latest message"));
        assert!(!messages.iter().any(|message| {
            message.role == "user" && message.content.starts_with("old user")
        }));
        Ok(colibri_rust::messages::ModelResponse {
            text: "budget ok".to_string(),
            tool_calls: Vec::new(),
        })
    }
}

struct RepeatedBudgetPressureModel {
    calls: usize,
}

impl RepeatedBudgetPressureModel {
    fn new() -> Self {
        Self { calls: 0 }
    }
}

impl ModelClient for RepeatedBudgetPressureModel {
    fn complete(
        &mut self,
        messages: &[Message],
        _tools: &[serde_json::Value],
        _system: &str,
        _limits: &ModelLimits,
    ) -> Result<colibri_rust::messages::ModelResponse, String> {
        self.calls += 1;
        if messages.iter().any(|message| {
            message.role == "system" && message.content.contains("Context budget is tight")
        }) {
            return Ok(colibri_rust::messages::ModelResponse {
                text: "stopped bulk read".to_string(),
                tool_calls: Vec::new(),
            });
        }
        Ok(colibri_rust::messages::ModelResponse {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: format!("call_{}", self.calls),
                name: "unknown.tool".to_string(),
                arguments: serde_json::Map::new(),
            }],
        })
    }
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn restore_home(value: Option<std::ffi::OsString>) {
    restore_env_var("HOME", value);
}

fn restore_env_var(key: &str, value: Option<std::ffi::OsString>) {
    if let Some(value) = value {
        std::env::set_var(key, value);
    } else {
        std::env::remove_var(key);
    }
}

#[derive(Clone, Debug)]
struct CapturedHttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

struct TestHttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl TestHttpResponse {
    fn json(body: &str) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: body.as_bytes().to_vec(),
        }
    }

    fn bytes(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body,
        }
    }
}

struct TestHttpServer {
    base_url: String,
    requests: Arc<Mutex<Vec<CapturedHttpRequest>>>,
}

fn start_http_server<F, H>(make_handler: F) -> TestHttpServer
where
    F: FnOnce(String, Arc<Mutex<Vec<CapturedHttpRequest>>>) -> H,
    H: Fn(CapturedHttpRequest) -> TestHttpResponse + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(make_handler(base_url.clone(), Arc::clone(&requests)));
    let requests_for_thread = Arc::clone(&requests);
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let Ok(request) = read_http_request(stream.try_clone().unwrap()) else {
                continue;
            };
            let response = handler(request.clone());
            requests_for_thread.lock().unwrap().push(request);
            let _ = write_http_response(stream, response);
        }
    });
    TestHttpServer { base_url, requests }
}

fn read_http_request(mut stream: TcpStream) -> Result<CapturedHttpRequest, String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| error.to_string())?;
    let mut data = Vec::new();
    let mut buffer = [0u8; 1024];
    let header_end;
    loop {
        let read = stream
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("connection closed before headers".to_string());
        }
        data.extend_from_slice(&buffer[..read]);
        if let Some(index) = find_header_end(&data) {
            header_end = index;
            break;
        }
    }
    let headers = String::from_utf8_lossy(&data[..header_end]).to_string();
    let mut lines = headers.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let content_length = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("Content-Length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    while data.len().saturating_sub(body_start) < content_length {
        let read = stream
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..read]);
    }
    Ok(CapturedHttpRequest {
        method,
        path,
        body: data[body_start..body_start + content_length.min(data.len() - body_start)].to_vec(),
    })
}

fn write_http_response(mut stream: TcpStream, response: TestHttpResponse) -> Result<(), String> {
    let mut header = format!(
        "HTTP/1.1 {} OK\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        response.body.len()
    );
    for (key, value) in response.headers {
        header.push_str(&format!("{}: {}\r\n", key, value));
    }
    header.push_str("\r\n");
    stream
        .write_all(header.as_bytes())
        .and_then(|_| stream.write_all(&response.body))
        .map_err(|error| error.to_string())
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|window| window == b"\r\n\r\n")
}
