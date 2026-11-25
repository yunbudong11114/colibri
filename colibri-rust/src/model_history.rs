//! Model-facing history and immutable runtime-context snapshots.
use crate::messages::Message;
use serde::{Deserialize, Serialize};

pub const MAX_SNAPSHOT_BYTES: usize = 64 * 1024;
pub const MAX_SNAPSHOT_HISTORY_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUpdate {
    pub before_message: usize,
    pub text: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModelHistory {
    pub messages: Vec<Message>,
    pub summary: String,
    pub context_updates: Vec<ContextUpdate>,
}

pub fn runtime_context(memory: &str, skills: &str) -> String {
    let memory = if memory.is_empty() { "(none)" } else { memory };
    let skills = if skills.is_empty() { "(none)" } else { skills };
    let header = "<system-reminder>\nCurrent runtime context. This complete snapshot supersedes all earlier runtime-context snapshots, including runtime facts retained in summaries. Continue the current user task using the latest context.\n\nMemory:\n";
    let middle = "\nAvailable skills:\n";
    let footer = "\n</system-reminder>";
    let truncated = "\n[truncated]";
    let budget =
        MAX_SNAPSHOT_BYTES - header.len() - middle.len() - footer.len() - 2 * truncated.len();
    // Preserve space for both contributions when one is much larger than the other.
    let memory_end = char_boundary(memory, budget - skills.len().min(budget / 2));
    let skills_end = char_boundary(skills, budget - memory_end);
    let mut output = String::with_capacity(MAX_SNAPSHOT_BYTES.min(
        header.len() + middle.len() + footer.len() + memory_end + skills_end + 2 * truncated.len(),
    ));
    output.push_str(header);
    output.push_str(&memory[..memory_end]);
    if memory_end < memory.len() {
        output.push_str(truncated);
    }
    output.push_str(middle);
    output.push_str(&skills[..skills_end]);
    if skills_end < skills.len() {
        output.push_str(truncated);
    }
    output.push_str(footer);
    output
}

fn char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub fn context_bytes(updates: &[ContextUpdate]) -> usize {
    updates.iter().map(|update| update.text.len()).sum()
}

pub fn assemble(messages: &[Message], summary: &str, updates: &[ContextUpdate]) -> Vec<Message> {
    let mut output = Vec::new();
    let summary = crate::context::summary_context(summary);
    if !summary.is_empty() {
        output.push(Message::new("system", summary));
    }
    let mut updates = updates.iter().peekable();
    for index in 0..=messages.len() {
        while updates
            .peek()
            .is_some_and(|update| update.before_message == index)
        {
            let update = updates.next().unwrap();
            output.push(Message::new("user", &update.text));
        }
        if let Some(message) = messages.get(index) {
            output.push(message.clone());
        }
    }
    output
}

impl ModelHistory {
    pub fn encoded_len(&self) -> usize {
        struct Counter(usize);
        impl std::io::Write for Counter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self.0.saturating_add(bytes.len());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut counter = Counter(0);
        if serde_json::to_writer(&mut counter, self).is_err() {
            return usize::MAX;
        }
        counter.0
    }

    /// Keep a continuous suffix of whole turns, with the active runtime snapshot.
    pub fn retain_from(&mut self, start: usize) {
        let active = self
            .context_updates
            .iter()
            .rev()
            .find(|update| update.before_message <= start)
            .cloned();
        self.context_updates
            .retain(|update| update.before_message > start);
        for update in &mut self.context_updates {
            update.before_message -= start;
        }
        if let Some(mut update) = active {
            update.before_message = 0;
            self.context_updates.insert(0, update);
        }
        self.messages.drain(..start);
    }

    pub fn repair_tool_results(&mut self) {
        let mut output = Vec::new();
        let mut messages = std::mem::take(&mut self.messages).into_iter().peekable();
        let mut positions = Vec::new();
        while let Some(message) = messages.next() {
            positions.push(output.len());
            if message.role == "tool" {
                continue;
            }
            let expected = message
                .tool_calls
                .iter()
                .map(|call| call.id.clone())
                .collect::<Vec<_>>();
            output.push(message);
            if expected.is_empty() {
                continue;
            }
            let mut seen = std::collections::BTreeSet::new();
            while messages
                .peek()
                .is_some_and(|message| message.role == "tool")
            {
                positions.push(output.len());
                let result = messages.next().unwrap();
                if result
                    .tool_call_id
                    .as_ref()
                    .is_some_and(|id| expected.contains(id) && seen.insert(id.clone()))
                {
                    output.push(result);
                }
            }
            for id in expected {
                if !seen.contains(&id) {
                    let mut unknown = Message::new("tool",
                        "execution_unknown: The session ended without a recorded result. The operation may have completed. Verify its effects before retrying; do not automatically re-execute this call.");
                    unknown.tool_call_id = Some(id);
                    output.push(unknown);
                }
            }
        }
        positions.push(output.len());
        for update in &mut self.context_updates {
            update.before_message = positions[update.before_message.min(positions.len() - 1)];
        }
        self.messages = output;
    }

    pub fn bounded(
        mut self,
        message_limit: usize,
        char_limit: usize,
        byte_limit: usize,
    ) -> Option<Self> {
        if message_limit == 0 || char_limit == 0 || byte_limit == 0 {
            return None;
        }
        self.repair_tool_results();
        let starts = self
            .messages
            .iter()
            .enumerate()
            .filter_map(|(index, message)| (message.role == "user").then_some(index))
            .collect::<Vec<_>>();
        let latest = *starts.last()?;
        let mut start = latest;
        for &candidate in starts.iter().rev().skip(1) {
            let tail = &self.messages[candidate..];
            let chars = tail
                .iter()
                .map(|message| {
                    message.content.chars().count()
                        + serde_json::to_vec(&message.tool_calls).map_or(0, |bytes| bytes.len())
                })
                .sum::<usize>();
            if tail.len() > message_limit || chars > char_limit {
                break;
            }
            start = candidate;
        }
        self.retain_from(start);
        while self.encoded_len() > byte_limit {
            let next = self
                .messages
                .iter()
                .enumerate()
                .skip(1)
                .find_map(|(index, message)| (message.role == "user").then_some(index))?;
            self.retain_from(next);
        }
        Some(self)
    }
}
