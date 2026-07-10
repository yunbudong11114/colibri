use crate::messages::Message;

pub const SUMMARY_HEADER: &str = "Compacted conversation summary:";
pub const COMPACT_SYSTEM_PROMPT: &str =
    "You are a helpful AI assistant tasked with summarizing conversations.";

const COMPACT_PROMPT: &str = "CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.\n\n\
- Do NOT use shell, file, memory, network, or any other tool.\n\
- You already have all the context you need below.\n\
- Your entire response must be plain text: an <analysis> block followed by a <summary> block.\n\n\
Your task is to create a detailed summary of the conversation portion below for continuing an agent session on a small Linux device.\n\n\
Before providing your final summary, wrap your analysis in <analysis> tags. Then provide a <summary> block with these sections:\n\n\
1. Primary Request and Intent\n\
2. Key Technical Concepts\n\
3. Files and Code Sections\n\
4. Errors and fixes\n\
5. Problem Solving\n\
6. All user messages\n\
7. Pending Tasks\n\
8. Current Work\n\
9. Optional Next Step\n\n\
Preserve user goals, decisions, file paths, commands, tool names, memory changes, device constraints, unresolved errors, and the latest concrete next step. Keep tool outputs concise and summarize metadata rather than copying large outputs.\n\n\
Previous compacted summary:\n{existing_summary}\n\n\
Conversation portion to compact:\n{conversation}\n\n\
REMINDER: Do NOT call any tools. Respond with plain text only: an <analysis> block followed by a <summary> block.";

pub fn summarize_messages(messages: &[Message], max_line_chars: usize) -> String {
    let tool_names = tool_names_by_id(messages);
    let mut lines = Vec::new();
    for message in messages {
        if message.role == "user" || message.role == "assistant" {
            if !message.tool_calls.is_empty() {
                let names = message
                    .tool_calls
                    .iter()
                    .map(|call| call.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(bound_line(
                    &format!("{} tool_calls: {}", message.role, names),
                    max_line_chars,
                ));
            }
            if !message.content.is_empty() {
                lines.push(bound_line(
                    &format!("{}: {}", message.role, message.content),
                    max_line_chars,
                ));
            }
        } else if message.role == "tool" {
            let tool_name = message
                .tool_call_id
                .as_deref()
                .and_then(|id| tool_names.get(id).map(String::as_str))
                .unwrap_or("unknown");
            let status = tool_status(&message.content);
            lines.push(format!(
                "tool {} {}: {} chars",
                tool_name,
                status,
                message.content.chars().count()
            ));
        }
    }
    lines.join("\n")
}

pub fn compact_prompt_message(existing_summary: &str, messages: &[Message]) -> Message {
    let conversation = summarize_messages(messages, 500);
    Message::new(
        "user",
        COMPACT_PROMPT
            .replace(
                "{existing_summary}",
                if existing_summary.trim().is_empty() {
                    "(none)"
                } else {
                    existing_summary.trim()
                },
            )
            .replace(
                "{conversation}",
                if conversation.is_empty() {
                    "(no messages)"
                } else {
                    &conversation
                },
            ),
    )
}

pub fn format_model_summary(summary: &str) -> String {
    let without_analysis = strip_tag_block(summary, "analysis");
    if let Some(content) = extract_tag_block(&without_analysis, "summary") {
        return format!("Summary:\n{}", content.trim());
    }
    without_analysis.trim().to_string()
}

pub fn append_summary(existing: &str, addition: &str, max_chars: usize) -> String {
    let combined = [existing.trim(), addition.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if combined.chars().count() <= max_chars {
        return combined;
    }
    let lines = combined.lines().collect::<Vec<_>>();
    let mut kept = Vec::new();
    let mut total = 0usize;
    for line in lines.iter().rev() {
        let line_len = line.chars().count() + usize::from(!kept.is_empty());
        if !kept.is_empty() && total + line_len > max_chars {
            break;
        }
        if kept.is_empty() && line.chars().count() > max_chars {
            return line
                .chars()
                .rev()
                .take(max_chars)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
        }
        kept.push(*line);
        total += line_len;
    }
    kept.into_iter().rev().collect::<Vec<_>>().join("\n")
}

pub fn summary_context(summary: &str) -> String {
    if summary.is_empty() {
        String::new()
    } else {
        format!("{}\n\n{}", SUMMARY_HEADER, summary)
    }
}

pub fn budget_model_messages(messages: Vec<Message>, max_chars: usize) -> (Vec<Message>, usize) {
    if message_chars(&messages) <= max_chars {
        return (messages, 0);
    }
    let mut kept = message_groups(messages);
    let mut dropped = 0usize;
    while kept.len() > 1 && message_chars(&flatten_groups(&kept)) > max_chars {
        let Some(drop_index) = oldest_droppable_group_index(&kept) else {
            break;
        };
        dropped += kept[drop_index].len();
        kept.remove(drop_index);
    }
    (flatten_groups(&kept), dropped)
}

pub fn retain_recent_message_groups(messages: Vec<Message>, recent_limit: usize) -> Vec<Message> {
    if messages.is_empty() {
        return Vec::new();
    }
    let groups = message_groups(messages);
    let mut kept_reversed: Vec<Vec<Message>> = Vec::new();
    let mut kept_messages = 0usize;
    if recent_limit > 0 {
        for group in groups.iter().rev() {
            if !kept_reversed.is_empty() && kept_messages + group.len() > recent_limit {
                break;
            }
            kept_reversed.push(group.clone());
            kept_messages += group.len();
        }
    }
    let mut kept_groups = kept_reversed.into_iter().rev().collect::<Vec<_>>();
    if let Some(latest_user_group) = latest_user_group(&groups) {
        let already_kept = kept_groups.iter().any(|group| group == latest_user_group);
        if !already_kept {
            kept_groups.insert(0, latest_user_group.clone());
        }
    }
    flatten_groups(&kept_groups)
}

pub fn model_input_chars(messages: &[Message]) -> usize {
    message_chars(messages)
}

pub fn round_limit_text(messages: &[Message], max_tool_rounds: usize, max_chars: usize) -> String {
    let round_word = if max_tool_rounds == 1 {
        "round"
    } else {
        "rounds"
    };
    let mut lines = vec![
        format!(
            "Tool round limit reached after {} {}.",
            max_tool_rounds, round_word
        ),
        "The task may still be incomplete.".to_string(),
    ];
    let recent = recent_tool_summaries(messages, 4);
    if !recent.is_empty() {
        lines.push("Recent tool results:".to_string());
        lines.extend(recent.into_iter().map(|item| format!("- {}", item)));
    }
    lines.push(
        "You can continue the task, or increase session.max_tool_rounds if this is expected."
            .to_string(),
    );
    bound_text_block(&lines.join("\n"), max_chars)
}

fn recent_tool_summaries(messages: &[Message], limit: usize) -> Vec<String> {
    let tool_names = tool_names_by_id(messages);
    let mut summaries = Vec::new();
    for message in messages.iter().rev() {
        if message.role != "tool" {
            continue;
        }
        let tool_name = message
            .tool_call_id
            .as_deref()
            .and_then(|id| tool_names.get(id).map(String::as_str))
            .unwrap_or("unknown");
        let mut text = message
            .content
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.chars().count() > 120 {
            text = text.chars().take(116).collect::<String>() + " ...";
        }
        summaries.push(format!("{}: {}", tool_name, text));
        if summaries.len() >= limit {
            break;
        }
    }
    summaries.reverse();
    summaries
}

fn bound_text_block(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let suffix = "\n...[truncated]";
    let keep = max_chars.saturating_sub(suffix.chars().count());
    text.chars().take(keep).collect::<String>() + suffix
}

fn tool_names_by_id(messages: &[Message]) -> std::collections::BTreeMap<String, String> {
    let mut names = std::collections::BTreeMap::new();
    for message in messages {
        for call in &message.tool_calls {
            names.insert(call.id.clone(), call.name.clone());
        }
    }
    names
}

fn tool_status(content: &str) -> &str {
    for prefix in ["permission_denied:", "unknown_tool:", "tool_error:"] {
        if content.starts_with(prefix) {
            return content.split_once(':').map(|(left, _)| left).unwrap_or("ok");
        }
    }
    "ok"
}

fn bound_line(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let keep = max_chars.saturating_sub(" ...".chars().count());
    normalized.chars().take(keep).collect::<String>() + " ..."
}

fn strip_tag_block(text: &str, tag: &str) -> String {
    let start_marker = format!("<{}>", tag);
    let end_marker = format!("</{}>", tag);
    let Some(start) = text.find(&start_marker) else {
        return text.to_string();
    };
    let Some(end) = text.find(&end_marker) else {
        return text.to_string();
    };
    if end < start {
        return text.to_string();
    }
    format!("{}{}", &text[..start], &text[end + end_marker.len()..])
}

fn extract_tag_block(text: &str, tag: &str) -> Option<String> {
    let start_marker = format!("<{}>", tag);
    let end_marker = format!("</{}>", tag);
    let start = text.find(&start_marker)?;
    let end = text.find(&end_marker)?;
    if end < start {
        return None;
    }
    Some(text[start + start_marker.len()..end].to_string())
}

fn message_chars(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|message| message.role.chars().count() + message.content.chars().count())
        .sum()
}

fn message_groups(messages: Vec<Message>) -> Vec<Vec<Message>> {
    let mut groups = Vec::new();
    let mut index = 0usize;
    while index < messages.len() {
        let message = messages[index].clone();
        let mut group = vec![message.clone()];
        index += 1;
        if message.role == "assistant" && !message.tool_calls.is_empty() {
            let call_ids = message
                .tool_calls
                .iter()
                .map(|call| call.id.clone())
                .collect::<std::collections::BTreeSet<_>>();
            while index < messages.len() {
                let candidate = &messages[index];
                let Some(tool_call_id) = &candidate.tool_call_id else {
                    break;
                };
                if candidate.role != "tool" || !call_ids.contains(tool_call_id) {
                    break;
                }
                group.push(candidate.clone());
                index += 1;
            }
        }
        groups.push(group);
    }
    groups
}

fn flatten_groups(groups: &[Vec<Message>]) -> Vec<Message> {
    groups.iter().flatten().cloned().collect()
}

fn oldest_droppable_group_index(groups: &[Vec<Message>]) -> Option<usize> {
    let latest_user = latest_user_group(groups);
    groups.iter().enumerate().find_map(|(index, group)| {
        if group.iter().any(|message| message.role == "system") {
            return None;
        }
        if latest_user.is_some_and(|latest| group == latest) {
            return None;
        }
        Some(index)
    })
}

fn latest_user_group(groups: &[Vec<Message>]) -> Option<&Vec<Message>> {
    groups
        .iter()
        .rev()
        .find(|group| group.iter().any(|message| message.role == "user"))
}
