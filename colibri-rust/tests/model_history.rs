use colibri_rust::messages::{Message, ToolCall};
use colibri_rust::model_history::{
    assemble, runtime_context, ContextUpdate, ModelHistory, MAX_SNAPSHOT_BYTES,
};
use colibri_rust::session_history::TranscriptHistoryLoader;
use serde_json::json;
use std::fs;

fn call(id: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "shell.exec".into(),
        arguments: json!({"command":"true"}).as_object().unwrap().clone(),
    }
}

#[test]
fn runtime_snapshots_are_utf8_bounded_and_preserve_prior_wire_prefix() {
    let text = runtime_context(&"中文记忆".repeat(40000), &"技能目录".repeat(40000));
    assert!(text.len() <= MAX_SNAPSHOT_BYTES);
    assert!(text.contains("Available skills:") && text.ends_with("</system-reminder>"));
    let messages = vec![
        Message::new("user", "first"),
        Message::new("assistant", "answer"),
    ];
    let mut updates = vec![ContextUpdate {
        before_message: 0,
        text: "snapshot one".into(),
    }];
    let previous = assemble(&messages, "", &updates);
    updates.push(ContextUpdate {
        before_message: 2,
        text: "snapshot two".into(),
    });
    let later = assemble(&messages, "", &updates);
    assert_eq!(&later[..previous.len()], previous);
    assert_eq!(later.last().unwrap().content, "snapshot two");
}

#[test]
fn recovery_keeps_real_results_and_marks_only_missing_calls_unknown() {
    let mut assistant = Message::new("assistant", "");
    assistant.tool_calls = vec![call("a"), call("b")];
    let mut history = ModelHistory {
        messages: vec![
            Message::new("user", "work"),
            assistant,
            Message::tool("done", "a"),
            Message::tool("duplicate", "a"),
            Message::tool("orphan", "other"),
        ],
        context_updates: vec![ContextUpdate {
            before_message: 5,
            text: "latest".into(),
        }],
        ..Default::default()
    };
    history.repair_tool_results();
    assert_eq!(history.messages.len(), 4);
    assert_eq!(history.messages[2].content, "done");
    assert_eq!(history.messages[3].tool_call_id.as_deref(), Some("b"));
    assert!(history.messages[3]
        .content
        .starts_with("execution_unknown:"));
    assert_eq!(history.context_updates[0].before_message, 4);
    let once = history.messages.clone();
    history.repair_tool_results();
    assert_eq!(once, history.messages);
}

#[test]
fn transcript_recovery_isolates_sources_and_replays_compacted_tool_history() {
    let home = std::env::temp_dir().join(format!(
        "colibri-history-maintenance-{}",
        std::process::id()
    ));
    fs::create_dir_all(home.join("transcripts")).unwrap();
    let events = vec![
        json!({"type":"model_seed","payload":{"model_stream":"1-100-0","history":{"messages":[],"summary":"retained summary","context_updates":[]}}}),
        json!({"type":"user_message","payload":{"model_stream":"1-100-0","text":"question","model_text":"question\nfixed-time"}}),
        json!({"type":"runtime_context","payload":{"model_stream":"1-100-0","update":{"before_message":0,"text":"snapshot"}}}),
        json!({"type":"assistant_message","payload":{"model_stream":"1-100-0","text":"","tool_calls":[call("a")]}}),
        json!({"type":"tool_result","payload":{"model_stream":"1-100-0","id":"a","ok":true,"text":"display","model_text":"full result"}}),
        json!({"type":"assistant_message","payload":{"model_stream":"1-100-0","text":"done"}}),
        json!({"type":"user_message","payload":{"model_stream":"1-200-0","session_key":"weixin:friend","text":"other source"}}),
    ];
    let text = events
        .iter()
        .map(|event| event.to_string() + "\n")
        .collect::<String>();
    fs::write(home.join("transcripts/2026-11-25-test.jsonl"), text).unwrap();
    let loaded = TranscriptHistoryLoader::for_home(home.clone()).load_model("local");
    assert_eq!(loaded.summary, "retained summary");
    assert_eq!(loaded.messages.len(), 4);
    assert_eq!(loaded.messages[0].content, "question\nfixed-time");
    assert_eq!(loaded.messages[1].tool_calls[0].id, "a");
    assert_eq!(loaded.messages[2].content, "full result");
    assert_eq!(loaded.context_updates[0].text, "snapshot");
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn recovery_preserves_wire_prefix_beyond_all_display_limits() {
    let home = std::env::temp_dir().join(format!("colibri-full-recovery-{}", std::process::id()));
    fs::create_dir_all(home.join("transcripts")).unwrap();
    let mut messages = Vec::new();
    for i in 0..20 {
        messages.push(Message::new("user", format!("question {i}")));
        messages.push(Message::new("assistant", "中文".repeat(20000)));
    }
    let history = ModelHistory {
        messages,
        summary: "retained summary".into(),
        context_updates: vec![ContextUpdate {
            before_message: 17,
            text: "immutable context".into(),
        }],
    };
    let checkpoint = json!({"ts":"2026-11-25T23:59:00", "type":"model_seed", "payload":{
        "model_stream":"1-100-0", "history":history
    }});
    assert!(checkpoint.to_string().len() > 2 * 1024 * 1024);
    fs::write(
        home.join("transcripts/2026-11-25.jsonl"),
        checkpoint.to_string() + "\n",
    )
    .unwrap();
    let mut events = json!({"ts":"2026-11-26T00:01:00", "type":"user_message", "payload":{
        "model_stream":"1-100-0", "text":"next", "model_text":"next\nfixed time"
    }})
    .to_string()
        + "\n";
    // A newer stream in another source must never replace local history.
    events += &(json!({"ts":"2026-11-26T00:02:00", "type":"user_message", "payload":{
        "model_stream":"1-200-0", "session_key":"weixin:other", "text":"private"
    }})
    .to_string()
        + "\n{broken partial line");
    fs::write(home.join("transcripts/2026-11-26.jsonl"), events).unwrap();
    let restored = TranscriptHistoryLoader::for_home(home.clone()).load_model("local");
    assert_eq!(restored.messages.len(), 41);
    assert_eq!(&restored.messages[..40], history.messages);
    assert_eq!(restored.context_updates, history.context_updates);
    assert_eq!(restored.summary, history.summary);
    let before = assemble(
        &history.messages,
        &history.summary,
        &history.context_updates,
    );
    let after = assemble(
        &restored.messages,
        &restored.summary,
        &restored.context_updates,
    );
    assert_eq!(&after[..before.len()], before);
    assert_eq!(
        restored.messages.last().unwrap().content,
        "next\nfixed time"
    );
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn latest_stream_restores_checkpoint_and_repairs_interrupted_tool_call() {
    let home = std::env::temp_dir().join(format!(
        "colibri-interrupted-recovery-{}",
        std::process::id()
    ));
    fs::create_dir_all(home.join("transcripts")).unwrap();
    let mut assistant = Message::new("assistant", "");
    assistant.tool_calls = vec![call("unfinished")];
    let history = ModelHistory {
        messages: vec![Message::new("user", "current task"), assistant],
        ..Default::default()
    };
    let events = [
        json!({"ts":"2026-11-26T00:01:00", "type":"user_message", "payload":{"model_stream":"1-100-0", "text":"old"}}),
        json!({"ts":"2026-11-26T00:02:00", "type":"model_seed", "payload":{"model_stream":"1-200-0", "history":history}}),
    ];
    fs::write(
        home.join("transcripts/2026-11-26.jsonl"),
        events
            .iter()
            .map(|e| e.to_string() + "\n")
            .collect::<String>(),
    )
    .unwrap();
    let restored = TranscriptHistoryLoader::for_home(home.clone()).load_model("local");
    assert_eq!(restored.messages[0].content, "current task");
    assert_eq!(restored.messages.len(), 3);
    assert!(restored.messages[2]
        .content
        .starts_with("execution_unknown:"));
    assert_eq!(
        restored.messages[2].tool_call_id.as_deref(),
        Some("unfinished")
    );
    fs::remove_dir_all(home).unwrap();
}
