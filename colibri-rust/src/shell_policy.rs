use std::path::Path;

pub(crate) fn denied_shell_executable(command: &str, deny: &[String]) -> Option<String> {
    for executable in shell_executables(command) {
        let executable_name = Path::new(&executable)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| executable.clone());
        if deny
            .iter()
            .any(|denied| denied == &executable || denied == &executable_name)
        {
            return Some(executable);
        }
    }
    None
}

pub(crate) fn first_shell_executable(command: &str) -> Option<String> {
    shell_executables(command).into_iter().next()
}

pub(crate) fn shell_executables(command: &str) -> Vec<String> {
    shell_command_segments(command)
        .into_iter()
        .filter_map(|segment| {
            shell_words::split(&segment)
                .ok()
                .and_then(|argv| first_executable_token(&argv))
        })
        .collect()
}

pub(crate) fn has_dangerous_shell_features(command: &str) -> bool {
    if has_unquoted_nested_shell_syntax(command) {
        return true;
    }
    shell_command_segments(command).into_iter().any(|segment| {
        shell_words::split(&segment)
            .ok()
            .and_then(|argv| first_executable_token(&argv))
            .is_some_and(|executable| matches!(executable.as_str(), "eval" | "source" | "."))
    })
}

pub(crate) fn shell_command_segments(command: &str) -> Vec<String> {
    let chars: Vec<char> = command.chars().collect();
    let mut segments = Vec::new();
    let mut buffer = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if escaped {
            buffer.push(ch);
            escaped = false;
            index += 1;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            buffer.push(ch);
            escaped = true;
            index += 1;
            continue;
        }
        if let Some(quote_char) = quote {
            buffer.push(ch);
            if ch == quote_char {
                quote = None;
            }
            index += 1;
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            buffer.push(ch);
            index += 1;
            continue;
        }
        if ch == '\n' || ch == ';' {
            push_shell_segment(&mut segments, &mut buffer);
            index += 1;
            continue;
        }
        if ch == '|' && (index + 1 >= chars.len() || chars[index + 1] != '|') {
            push_shell_segment(&mut segments, &mut buffer);
            index += 1;
            continue;
        }
        if (ch == '&' || ch == '|') && index + 1 < chars.len() && chars[index + 1] == ch {
            push_shell_segment(&mut segments, &mut buffer);
            index += 2;
            continue;
        }
        if ch == '&' && !is_ampersand_redirection(&chars, index) {
            push_shell_segment(&mut segments, &mut buffer);
            index += 1;
            continue;
        }
        buffer.push(ch);
        index += 1;
    }
    push_shell_segment(&mut segments, &mut buffer);
    segments
}

fn has_unquoted_nested_shell_syntax(command: &str) -> bool {
    let chars: Vec<char> = command.chars().collect();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            index += 1;
            continue;
        }
        if let Some(quote_char) = quote {
            if ch == quote_char {
                quote = None;
            }
            index += 1;
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            index += 1;
            continue;
        }
        if ch == '`'
            || ch == '('
            || (ch == '$' && index + 1 < chars.len() && chars[index + 1] == '(')
        {
            return true;
        }
        if ch == '<' && index + 1 < chars.len() && chars[index + 1] == '<' {
            return true;
        }
        index += 1;
    }
    false
}

fn is_ampersand_redirection(chars: &[char], index: usize) -> bool {
    let previous = index
        .checked_sub(1)
        .and_then(|previous| chars.get(previous));
    let next = chars.get(index + 1);
    previous == Some(&'>') || next == Some(&'>')
}

fn push_shell_segment(segments: &mut Vec<String>, buffer: &mut String) {
    let segment = buffer.trim();
    if !segment.is_empty() {
        segments.push(segment.to_string());
    }
    buffer.clear();
}

fn first_executable_token(argv: &[String]) -> Option<String> {
    for token in argv {
        if is_env_assignment(token) {
            continue;
        }
        let executable = strip_inline_redirection(token);
        if !executable.is_empty() {
            return Some(executable);
        }
    }
    None
}

fn is_env_assignment(token: &str) -> bool {
    let Some((name, _value)) = token.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn strip_inline_redirection(token: &str) -> String {
    token
        .split(['<', '>'])
        .next()
        .unwrap_or_default()
        .to_string()
}
