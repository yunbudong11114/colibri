use crate::messages::Message;

pub const SUMMARY_HEADER: &str = "Compacted conversation summary:";
pub const COMPACT_SYSTEM_PROMPT: &str =
    "You are a helpful AI assistant tasked with summarizing conversations.";

const COMPACT_PROMPT: &str = "CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.\n\n\
- Do NOT use shell, file, memory, network, or any other tool.\n\
- You already have all the context you need below.\n\
- Your entire response must be plain text inside one <summary> block.\n\n\
Your task is to create a detailed summary of the conversation portion below for continuing an agent session on a small Linux device.\n\n\
Provide a concise <summary> block with these sections:\n\n\
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
REMINDER: Do NOT call any tools. Respond with plain text only inside a <summary> block.";

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
                "tool {} {}: {}",
                tool_name,
                status,
                bound_line(&message.content, max_line_chars)
            ));
        }
    }
    let mut output = String::new();
    for line in lines {
        if output.len() + line.len() > 24 * 1024 {
            output.push_str("\n[Earlier evidence truncated to fit summary budget]");
            break;
        }
        output.push_str(&line);
        output.push('\n');
    }
    output.trim_end().to_string()
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

pub fn retain_recent_message_groups(messages: Vec<Message>, recent_limit: usize) -> Vec<Message> {
    retain_recent_message_groups_with_token_limit(messages, recent_limit, None)
}

pub fn retain_recent_message_groups_with_token_limit(
    messages: Vec<Message>,
    recent_limit: usize,
    recent_token_limit: Option<usize>,
) -> Vec<Message> {
    if messages.is_empty() {
        return Vec::new();
    }
    let groups = message_groups(messages);
    let mut kept_reversed: Vec<Vec<Message>> = Vec::new();
    let mut kept_messages = 0usize;
    let mut kept_tokens = 0usize;
    if recent_limit > 0 {
        for group in groups.iter().rev() {
            let group_tokens = estimate_model_input_tokens(group);
            let exceeds_messages = kept_messages + group.len() > recent_limit;
            let exceeds_tokens = recent_token_limit
                .is_some_and(|limit| kept_tokens.saturating_add(group_tokens) > limit);
            if !kept_reversed.is_empty() && (exceeds_messages || exceeds_tokens) {
                break;
            }
            kept_reversed.push(group.clone());
            kept_messages += group.len();
            kept_tokens = kept_tokens.saturating_add(group_tokens);
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

pub fn estimate_model_input_tokens(messages: &[Message]) -> usize {
    estimate_completion_input_tokens(messages, "", &[])
}

pub fn estimate_completion_input_tokens(
    messages: &[Message],
    system: &str,
    tools: &[serde_json::Value],
) -> usize {
    let mut byte_count = if system.is_empty() {
        0
    } else {
        "system".len() + system.len()
    };
    if !tools.is_empty() {
        byte_count = byte_count.saturating_add(
            serde_json::to_vec(tools)
                .map(|encoded| encoded.len())
                .unwrap_or_default(),
        );
    }
    for message in messages {
        byte_count = byte_count
            .saturating_add(message.role.len())
            .saturating_add(message.content.len());
        if let Some(tool_call_id) = &message.tool_call_id {
            byte_count = byte_count.saturating_add(tool_call_id.len());
        }
        for call in &message.tool_calls {
            byte_count = byte_count
                .saturating_add(call.id.len())
                .saturating_add(call.name.len())
                .saturating_add(
                    serde_json::to_vec(&call.arguments)
                        .map(|encoded| encoded.len())
                        .unwrap_or_default(),
                );
        }
    }
    (byte_count + 3) / 4
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
            return content
                .split_once(':')
                .map(|(left, _)| left)
                .unwrap_or("ok");
        }
    }
    "ok"
}

fn bound_line(text: &str, max_chars: usize) -> String {
    let normalized = text
        .chars()
        .take(max_chars.saturating_mul(4))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
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

fn latest_user_group(groups: &[Vec<Message>]) -> Option<&Vec<Message>> {
    groups
        .iter()
        .rev()
        .find(|group| group.iter().any(|message| message.role == "user"))
}
