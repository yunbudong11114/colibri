use crate::messages::Message;
use std::mem::MaybeUninit;

const TIME_FORMAT: &[u8] = b"%Y-%m-%d %H:%M|%z\0";
const TIME_CONTEXT_SEPARATOR: &str = "\n\n## Current Time\n";
const TIME_USAGE_INSTRUCTION: &str = "Use this only when needed. Do not mention or repeat the current time unless the user asks or it is relevant.";

pub fn current_time_context() -> String {
    let timestamp = unsafe { libc::time(std::ptr::null_mut()) };
    let (date_time, offset) = local_time_parts(timestamp)
        .or_else(|| utc_time_parts(timestamp))
        .unwrap_or_else(|| ("1970-01-01 00:00".to_string(), "+0000".to_string()));
    format_time_context(&date_time, &offset)
}

pub fn format_time_context(date_time: &str, offset: &str) -> String {
    format!(
        "## Current Time\n{} ({})\n{}",
        date_time,
        normalize_utc_offset(offset),
        TIME_USAGE_INSTRUCTION
    )
}

pub fn append_time_context_to_latest_user(messages: &mut [Message], time_context: &str) -> bool {
    let Some(message) = messages
        .iter_mut()
        .rev()
        .find(|message| message.role == "user")
    else {
        return false;
    };
    if message.content.is_empty() {
        message.content.push_str(time_context);
    } else if let Some(time_line) = time_context.strip_prefix("## Current Time\n") {
        message.content.push_str(TIME_CONTEXT_SEPARATOR);
        message.content.push_str(time_line);
    } else {
        message.content.push_str("\n\n");
        message.content.push_str(time_context);
    }
    true
}

pub fn user_text_without_time_context(content: &str) -> &str {
    content
        .rsplit_once(TIME_CONTEXT_SEPARATOR)
        .filter(|(_, time_line)| is_formatted_time_line(time_line))
        .map(|(user_text, _)| user_text)
        .unwrap_or(content)
}

fn is_formatted_time_line(value: &str) -> bool {
    let Some((value, instruction)) = value.split_once('\n') else {
        return false;
    };
    if instruction != TIME_USAGE_INSTRUCTION {
        return false;
    }
    let bytes = value.as_bytes();
    bytes.len() == 25
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b' '
        && bytes[13] == b':'
        && bytes[16] == b' '
        && bytes[17] == b'('
        && matches!(bytes[18], b'+' | b'-')
        && bytes[21] == b':'
        && bytes[24] == b')'
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 17 | 18 | 21 | 24) || byte.is_ascii_digit()
        })
}

fn normalize_utc_offset(offset: &str) -> String {
    let offset = offset.trim();
    if offset.len() == 5
        && matches!(offset.as_bytes()[0], b'+' | b'-')
        && offset.as_bytes()[1..].iter().all(u8::is_ascii_digit)
    {
        return format!("{}{}:{}", &offset[..1], &offset[1..3], &offset[3..5]);
    }
    if offset.len() == 6
        && matches!(offset.as_bytes()[0], b'+' | b'-')
        && offset.as_bytes()[3] == b':'
        && offset
            .as_bytes()
            .iter()
            .enumerate()
            .all(|(index, value)| index == 0 || index == 3 || value.is_ascii_digit())
    {
        return offset.to_string();
    }
    "+00:00".to_string()
}

#[cfg(unix)]
fn local_time_parts(timestamp: libc::time_t) -> Option<(String, String)> {
    format_time_parts(timestamp, false)
}

#[cfg(unix)]
fn utc_time_parts(timestamp: libc::time_t) -> Option<(String, String)> {
    format_time_parts(timestamp, true).map(|(date_time, _)| (date_time, "+0000".to_string()))
}

#[cfg(unix)]
fn format_time_parts(timestamp: libc::time_t, utc: bool) -> Option<(String, String)> {
    let mut time = MaybeUninit::<libc::tm>::uninit();
    let converted = unsafe {
        if utc {
            libc::gmtime_r(&timestamp, time.as_mut_ptr())
        } else {
            libc::localtime_r(&timestamp, time.as_mut_ptr())
        }
    };
    if converted.is_null() {
        return None;
    }
    let time = unsafe { time.assume_init() };
    let mut buffer = [0u8; 64];
    let length = unsafe {
        libc::strftime(
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            TIME_FORMAT.as_ptr().cast(),
            &time,
        )
    };
    if length == 0 {
        return None;
    }
    let formatted = std::str::from_utf8(&buffer[..length]).ok()?;
    let (date_time, offset) = formatted.split_once('|')?;
    Some((date_time.to_string(), offset.to_string()))
}

#[cfg(not(unix))]
fn local_time_parts(_timestamp: libc::time_t) -> Option<(String, String)> {
    None
}

#[cfg(not(unix))]
fn utc_time_parts(_timestamp: libc::time_t) -> Option<(String, String)> {
    None
}

#[cfg(test)]
mod tests {
    use super::{
        append_time_context_to_latest_user, format_time_context, normalize_utc_offset,
        user_text_without_time_context,
    };
    use crate::messages::Message;

    #[test]
    fn formats_shared_time_context() {
        assert_eq!(
            format_time_context("2026-08-13 09:05", "+0800"),
            "## Current Time\n2026-08-13 09:05 (+08:00)\nUse this only when needed. Do not mention or repeat the current time unless the user asks or it is relevant."
        );
        assert_eq!(
            format_time_context("2026-08-12 18:35", "-0730"),
            "## Current Time\n2026-08-12 18:35 (-07:30)\nUse this only when needed. Do not mention or repeat the current time unless the user asks or it is relevant."
        );
    }

    #[test]
    fn normalizes_utc_and_invalid_offsets() {
        assert_eq!(normalize_utc_offset("+0000"), "+00:00");
        assert_eq!(normalize_utc_offset("+08:00"), "+08:00");
        assert_eq!(normalize_utc_offset("unknown"), "+00:00");
    }

    #[test]
    fn appends_only_to_latest_user_and_preserves_stable_prefix() {
        let stable_prefix = vec![
            Message::new("system", "summary"),
            Message::new("user", "previous question"),
            Message::new("assistant", "previous answer"),
        ];
        let mut first = stable_prefix.clone();
        first.push(Message::new(
            "user",
            "[channel=weixin sender=alice]\ncurrent question",
        ));
        let mut second = stable_prefix.clone();
        second.push(Message::new(
            "user",
            "[channel=console sender=bob]\ncurrent question",
        ));

        assert!(append_time_context_to_latest_user(
            &mut first,
            "## Current Time\n2026-08-20 10:00 (+08:00)",
        ));
        assert!(append_time_context_to_latest_user(
            &mut second,
            "## Current Time\n2026-08-20 10:01 (+08:00)",
        ));

        assert_eq!(&first[..stable_prefix.len()], stable_prefix.as_slice());
        assert_eq!(&second[..stable_prefix.len()], stable_prefix.as_slice());
        assert_ne!(
            first.last().unwrap().content,
            second.last().unwrap().content
        );
    }

    #[test]
    fn returns_false_without_a_user_message() {
        let mut messages = vec![Message::new("assistant", "answer")];
        assert!(!append_time_context_to_latest_user(
            &mut messages,
            "## Current Time\n2026-08-20 10:00 (+08:00)",
        ));
    }

    #[test]
    fn removes_only_a_valid_appended_time_context() {
        let augmented = "hello\n\n## Current Time\n2026-08-20 10:00 (+08:00)\nUse this only when needed. Do not mention or repeat the current time unless the user asks or it is relevant.";
        assert_eq!(user_text_without_time_context(augmented), "hello");
        assert_eq!(
            user_text_without_time_context("hello\n\n## Current Time\nnot-runtime-data"),
            "hello\n\n## Current Time\nnot-runtime-data"
        );
    }
}
