use colibri_rust::config::AgentConfig;
use colibri_rust::messages::{Message, ToolCall};
use colibri_rust::model::FakeModel;
use colibri_rust::session::AgentSession;
use std::sync::Arc;

fn session() -> AgentSession {
    let mut config = AgentConfig::default();
    config.session.transcript = false;
    config.memory.enabled = false;
    config.model.input_context_tokens = 4_000_000;
    AgentSession::new(config, Box::new(FakeModel::new()))
}

#[test]
fn manual_compaction_preserves_current_tool_round_and_evidence() {
    let mut app = session();
    app.messages.push(Message::new("user", "Diagnose gateway"));
    app.messages
        .push(Message::new("tool", "PID 68903 state Z RSS 0"));
    app.messages
        .push(Message::new("assistant", "Gateway is a zombie"));
    app.messages.push(Message::new("user", "Repair it"));
    let mut call = Message::new("assistant", "");
    call.tool_calls.push(ToolCall {
        id: "repair".into(),
        name: "shell.run".into(),
        arguments: Default::default(),
    });
    app.messages.push(call);
    let mut result = Message::new("tool", "running");
    result.tool_call_id = Some("repair".into());
    app.messages.push(result);
    let latest = app.messages[3..].to_vec();
    let response = app.submit("/compact").unwrap();
    assert!(response.text.contains("已压缩"));
    assert_eq!(app.messages, latest);
    assert!(app.summary.contains("state Z RSS 0"));
    assert!(!app.summary.contains("Repair it"));
    assert_eq!(app.submit("/compact").unwrap().error_type, None);
    assert_eq!(app.messages, latest);
}

#[test]
fn explicit_memory_budget_compacts_before_token_pressure() {
    let mut app = session();
    Arc::make_mut(&mut app.config)
        .session
        .max_context_memory_mib = Some(1);
    app.messages.push(Message::new("user", "old"));
    app.messages
        .push(Message::new("assistant", &"a".repeat(1_100_000)));
    app.submit("new").unwrap();
    assert!(!app.summary.is_empty());
    assert!(app.messages.iter().all(|m| m.content.len() < 1000));
}

#[test]
fn empty_compaction_does_not_create_a_conversation() {
    let mut app = session();
    assert!(app.submit("/compact").unwrap().text.contains("没有"));
    assert!(app.messages.is_empty());
    assert!(app.summary.is_empty());
}

#[test]
fn obsolete_compaction_options_are_rejected() {
    for key in [
        "max_tool_rounds",
        "trigger_message_limit",
        "recent_message_limit",
        "summary_max_chars",
    ] {
        let path =
            std::env::temp_dir().join(format!("colibri-policy-{}-{key}.toml", std::process::id()));
        std::fs::write(&path, format!("[session]\n{key}=12\n")).unwrap();
        assert!(AgentConfig::load(Some(&path)).is_err());
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn a_single_oversized_turn_returns_a_memory_budget_error() {
    let mut app = session();
    Arc::make_mut(&mut app.config)
        .session
        .max_context_memory_mib = Some(1);
    let result = app.submit(&"x".repeat(1_100_000)).unwrap();
    assert_eq!(result.error_type.as_deref(), Some("context_memory_budget"));
    assert!(app.summary.is_empty());
}

#[test]
fn default_does_not_compact_at_the_old_fixed_byte_threshold() {
    let mut app = session();
    assert_eq!(app.config.session.max_context_memory_mib, None);
    app.messages.push(Message::new("user", "old"));
    app.messages
        .push(Message::new("assistant", &"a".repeat(900_000)));
    app.submit("new").unwrap();
    assert!(app.summary.is_empty());
    assert_eq!(app.messages.len(), 4);
}

#[test]
fn optional_budget_accepts_zero_and_rejects_invalid_values() {
    for (index, value) in ["0", "10", "-1", "'ten'", "1.5"].iter().enumerate() {
        let path = std::env::temp_dir().join(format!(
            "colibri-memory-budget-{}-{index}.toml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            format!("[session]\nmax_context_memory_mib={value}\n"),
        )
        .unwrap();
        let result = AgentConfig::load(Some(&path));
        if index == 0 {
            assert_eq!(result.unwrap().session.max_context_memory_mib, None);
        } else if index == 1 {
            assert_eq!(result.unwrap().session.max_context_memory_mib, Some(10));
        } else {
            assert!(result.is_err());
        }
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn automatic_compaction_reports_activity_then_returns_to_thinking() {
    use std::sync::Mutex;
    let stages = Arc::new(Mutex::new(Vec::new()));
    let received = Arc::clone(&stages);
    let mut config = AgentConfig::default();
    config.session.transcript = false;
    config.memory.enabled = false;
    config.model.input_context_tokens = 4096;
    config.model.max_output_tokens = 128;
    config.tools.enabled.clear();
    config.skills.max_catalog = 0;
    let mut app = AgentSession::new(config, Box::new(FakeModel::new())).with_activity_callback(
        Arc::new(move |stage| received.lock().unwrap().push(stage.to_owned())),
    );
    app.messages.push(Message::new("user", "old task"));
    app.messages
        .push(Message::new("assistant", "old evidence ".repeat(6000)));
    assert_eq!(app.submit("continue").unwrap().text, "fake: continue");
    let stages = stages.lock().unwrap();
    assert_eq!(stages.as_slice(), ["压缩上下文", "思考中"]);
    assert!(!app
        .messages
        .iter()
        .any(|message| message.content == "压缩上下文"));
}
