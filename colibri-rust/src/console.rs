//! Console status lines and small-screen answer formatting.
//! Kept dependency-free for CardputerZero / headless Linux.

use serde_json::Value;

pub fn format_answer_for_console(text: &str, plain_answer: bool) -> String {
    if plain_answer {
        format!("\n{}\n", format_plain_answer(text))
    } else {
        text.to_string()
    }
}

pub fn format_plain_answer(text: &str) -> String {
    let mut out_lines = Vec::new();
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end();
        if is_table_separator(line) {
            continue;
        }
        if is_table_row(line) {
            table_rows.push(split_table_row(line));
            continue;
        }
        flush_table_rows(&mut out_lines, &mut table_rows);
        out_lines.push(strip_inline_markdown(line));
    }
    flush_table_rows(&mut out_lines, &mut table_rows);
    let mut result = out_lines.join("\n");
    while result.ends_with('\n') {
        result.pop();
    }
    result
}

pub fn status_line_for_event(event_type: &str, payload: &Value) -> Option<String> {
    match event_type {
        "memory_context" => {
            let files = join_string_array(payload.get("files"));
            Some(format!("[colibri] memory files={files}"))
        }
        "skill_recall" => {
            let skills = join_string_array(payload.get("skills"));
            Some(format!("[colibri] skill skills={skills}"))
        }
        "tool_call" => {
            let name = payload
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            Some(format!("[colibri] tool {name} wait_permission"))
        }
        "tool_result" => {
            let name = payload
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            let state = if payload.get("ok").and_then(|value| value.as_bool()) == Some(true) {
                "ok".to_string()
            } else {
                payload
                    .get("error_type")
                    .and_then(|value| value.as_str())
                    .unwrap_or("error")
                    .to_string()
            };
            let chars = payload
                .get("text")
                .and_then(|value| value.as_str())
                .map(|text| text.chars().count())
                .unwrap_or(0);
            Some(format!("[colibri] tool {name} {state} chars={chars}"))
        }
        "context_compact" => {
            let mode = payload
                .get("mode")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let removed = payload
                .get("removed_messages")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            let summary_chars = payload
                .get("summary_chars")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            Some(format!(
                "[colibri] compact mode={mode} removed={removed} summary_chars={summary_chars}"
            ))
        }
        "model_error" => {
            let error_type = payload
                .get("error_type")
                .and_then(|value| value.as_str())
                .unwrap_or("error");
            Some(format!("[colibri] model_error type={error_type}"))
        }
        _ => None,
    }
}

fn join_string_array(value: Option<&Value>) -> String {
    value
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default()
}

fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.matches('|').count() >= 2
}

fn is_table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.starts_with('|') {
        return false;
    }
    trimmed
        .chars()
        .all(|ch| matches!(ch, '|' | '-' | ':' | ' ' | '\t'))
        && trimmed.contains('-')
}

fn split_table_row(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| strip_inline_markdown(cell.trim()))
        .collect()
}

fn flush_table_rows(out_lines: &mut Vec<String>, rows: &mut Vec<Vec<String>>) {
    if rows.is_empty() {
        return;
    }
    for row in rows.drain(..) {
        out_lines.push(row.join(" / "));
    }
}

fn strip_inline_markdown(line: &str) -> String {
    let mut text = line.trim_start_matches('#').trim_start().to_string();
    text = text.replace("**", "");
    text = text.replace("__", "");
    text = text.replace('`', "");
    text
}
