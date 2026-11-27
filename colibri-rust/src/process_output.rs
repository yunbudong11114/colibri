use std::io::Read;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

const TRUNCATION_MARKER: &str = "\n...[truncated]...\n";

pub struct BoundedCommandOutput {
    pub success: bool,
    pub text: String,
    pub truncated: bool,
    pub exit_code: Option<i32>,
    pub termination: Option<&'static str>,
}

pub enum CommandRunError {
    Timeout,
    Cancelled,
    Io(String),
}

struct BoundedBytes {
    capacity: usize,
    head_capacity: usize,
    tail_capacity: usize,
    buffer: Vec<u8>,
    head: Vec<u8>,
    tail: Vec<u8>,
    truncated: bool,
}

impl BoundedBytes {
    fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let head_capacity = capacity / 4;
        Self {
            capacity,
            head_capacity,
            tail_capacity: capacity - head_capacity,
            buffer: Vec::new(),
            head: Vec::new(),
            tail: Vec::new(),
            truncated: false,
        }
    }

    fn feed(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        if !self.truncated && self.buffer.len().saturating_add(chunk.len()) <= self.capacity {
            self.buffer.extend_from_slice(chunk);
            return;
        }
        if !self.truncated {
            let mut combined = std::mem::take(&mut self.buffer);
            combined.extend_from_slice(chunk);
            self.head
                .extend_from_slice(&combined[..combined.len().min(self.head_capacity)]);
            let tail_start = combined.len().saturating_sub(self.tail_capacity);
            self.tail.extend_from_slice(&combined[tail_start..]);
            self.truncated = true;
            return;
        }
        self.tail.extend_from_slice(chunk);
        if self.tail.len() > self.tail_capacity {
            self.tail
                .drain(..self.tail.len().saturating_sub(self.tail_capacity));
        }
    }

    fn sampled(self) -> (Vec<u8>, bool) {
        if !self.truncated {
            return (self.buffer, false);
        }
        let mut sampled = self.head;
        sampled.extend(self.tail);
        (sampled, true)
    }
}

pub fn run_bounded_command(
    command: Command,
    timeout: Duration,
    max_result_chars: usize,
) -> Result<BoundedCommandOutput, CommandRunError> {
    run_command(command, timeout, max_result_chars, None)
}

pub(crate) fn run_tool_command(
    command: Command,
    timeout: Duration,
    context: &crate::tools::ToolContext,
) -> Result<BoundedCommandOutput, CommandRunError> {
    run_command(
        command,
        timeout,
        context.config.tools.max_result_chars.min(4096),
        Some(context),
    )
}

fn run_command(
    mut command: Command,
    timeout: Duration,
    max_result_chars: usize,
    context: Option<&crate::tools::ToolContext>,
) -> Result<BoundedCommandOutput, CommandRunError> {
    if crate::cancellation::cancelled() {
        return Err(CommandRunError::Cancelled);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| CommandRunError::Io(error.to_string()))?;
    let raw_capacity = max_result_chars.saturating_mul(4).max(64);
    let stream_capacity = (raw_capacity / 2).max(32);
    let stdout_reader = child.stdout.take().map(|stdout| {
        spawn_reader(
            stdout,
            stream_capacity,
            context.map(|c| crate::tool_archive::Archive::new(c, max_result_chars / 2)),
        )
    });
    let stderr_reader = child.stderr.take().map(|stderr| {
        spawn_reader(
            stderr,
            stream_capacity,
            context.map(|c| crate::tool_archive::Archive::new(c, max_result_chars / 2)),
        )
    });
    let started = Instant::now();
    let mut termination = None;
    let status = loop {
        if crate::cancellation::cancelled() {
            kill_process_tree(&mut child);
            termination = Some("cancelled");
            break child
                .wait()
                .map_err(|e| CommandRunError::Io(e.to_string()))?;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                kill_process_tree(&mut child);
                termination = Some("timeout");
                break child
                    .wait()
                    .map_err(|e| CommandRunError::Io(e.to_string()))?;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(error) => return Err(CommandRunError::Io(error.to_string())),
        }
    };
    let (stdout, stdout_archive) = join_reader(stdout_reader)?;
    let (stderr, stderr_archive) = join_reader(stderr_reader)?;
    let (mut sampled, stdout_truncated) = stdout.sampled();
    let (stderr_sampled, stderr_truncated) = stderr.sampled();
    sampled.extend(stderr_sampled);
    let text = String::from_utf8_lossy(&sampled).into_owned();
    let (text, truncated) = bound_head_tail_text(
        &text,
        max_result_chars,
        stdout_truncated || stderr_truncated,
    );
    let mut text = text;
    if context.is_some() {
        text += &format!(
            "\nexit_code={} termination={}\n",
            status.code().map_or("signal".into(), |v| v.to_string()),
            termination.unwrap_or("completed")
        );
        if !stdout_archive.is_empty() {
            text += &format!("stdout {stdout_archive}\n");
        }
        if !stderr_archive.is_empty() {
            text += &format!("stderr {stderr_archive}\n");
        }
    } else if let Some(reason) = termination {
        return Err(if reason == "cancelled" {
            CommandRunError::Cancelled
        } else {
            CommandRunError::Timeout
        });
    }
    Ok(BoundedCommandOutput {
        success: status.success() && termination.is_none(),
        exit_code: status.code(),
        termination,
        text,
        truncated,
    })
}

fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    capacity: usize,
    mut archive: Option<crate::tool_archive::Archive>,
) -> JoinHandle<Result<(BoundedBytes, String), String>> {
    std::thread::spawn(move || {
        let mut bounded = BoundedBytes::new(capacity);
        let mut chunk = [0u8; 8192];
        loop {
            let read = reader.read(&mut chunk).map_err(|error| error.to_string())?;
            if read == 0 {
                if let Some(a) = &mut archive {
                    bounded.feed(&a.drain(true));
                }
                return Ok((bounded, archive.map(|a| a.finish()).unwrap_or_default()));
            }
            if let Some(a) = &mut archive {
                bounded.feed(&a.feed(&chunk[..read]));
            } else {
                bounded.feed(&chunk[..read]);
            }
        }
    })
}

fn join_reader(
    reader: Option<JoinHandle<Result<(BoundedBytes, String), String>>>,
) -> Result<(BoundedBytes, String), CommandRunError> {
    let Some(reader) = reader else {
        return Ok((BoundedBytes::new(1), String::new()));
    };
    reader
        .join()
        .map_err(|_| CommandRunError::Io("command output reader panicked".to_string()))?
        .map_err(CommandRunError::Io)
}

fn bound_head_tail_text(text: &str, max_chars: usize, force: bool) -> (String, bool) {
    let char_count = text.chars().count();
    if !force && char_count <= max_chars {
        return (text.to_string(), false);
    }
    if max_chars == 0 {
        return (String::new(), force || char_count > 0);
    }
    let marker_chars = TRUNCATION_MARKER.chars().count();
    if max_chars <= marker_chars {
        return (TRUNCATION_MARKER.chars().take(max_chars).collect(), true);
    }
    let available = max_chars - marker_chars;
    let head_chars = available / 4;
    let tail_chars = available - head_chars;
    let head = text.chars().take(head_chars).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(tail_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    (format!("{}{}{}", head, TRUNCATION_MARKER, tail), true)
}

#[cfg(unix)]
fn kill_process_tree(child: &mut std::process::Child) {
    let pgid = child.id() as i32;
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_process_tree(child: &mut std::process::Child) {
    let _ = child.kill();
}
