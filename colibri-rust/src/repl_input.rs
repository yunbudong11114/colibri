use std::io::{self, BufRead, Read, Write};
use std::time::Duration;

pub struct ReplLineEditor<'a, W: Write> {
    prompt: String,
    stdout: &'a mut W,
    history: Vec<String>,
    chars: Vec<char>,
    history_index: Option<usize>,
    draft: String,
}

impl<'a, W: Write> ReplLineEditor<'a, W> {
    pub fn new(prompt: &str, stdout: &'a mut W, history: Vec<String>) -> Self {
        Self {
            prompt: prompt.to_string(),
            stdout,
            history,
            chars: Vec::new(),
            history_index: None,
            draft: String::new(),
        }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn start(&mut self) {
        let _ = write!(self.stdout, "{}", self.prompt);
        let _ = self.stdout.flush();
    }

    pub fn feed_text(&mut self, text: &str) {
        self.history_index = None;
        self.chars.extend(text.chars());
        self.redraw();
    }

    pub fn backspace(&mut self) {
        self.history_index = None;
        self.chars.pop();
        self.redraw();
    }

    pub fn history_previous(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if let Some(index) = self.history_index {
            self.history_index = Some(index.saturating_sub(1));
        } else {
            self.draft = self.text();
            self.history_index = Some(self.history.len() - 1);
        }
        self.replace_text(&self.history[self.history_index.unwrap()].clone());
    }

    pub fn history_next(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index >= self.history.len() - 1 {
            self.history_index = None;
            self.replace_text(&self.draft.clone());
            return;
        }
        self.history_index = Some(index + 1);
        self.replace_text(&self.history[index + 1].clone());
    }

    fn replace_text(&mut self, text: &str) {
        self.chars = text.chars().collect();
        self.redraw();
    }

    fn redraw(&mut self) {
        let _ = write!(self.stdout, "\r\x1b[2K{}{}", self.prompt, self.text());
        let _ = self.stdout.flush();
    }
}

pub fn write_raw_tty_newline<W: Write>(stdout: &mut W) {
    let _ = write!(stdout, "\r\n");
    let _ = stdout.flush();
}

pub fn handle_escape_sequence<W: Write>(editor: &mut ReplLineEditor<'_, W>, sequence: &[u8]) {
    if sequence == b"\x1b[A" || sequence == b"\x1bOA" {
        editor.history_previous();
    } else if sequence == b"\x1b[B" || sequence == b"\x1bOB" {
        editor.history_next();
    }
}

pub fn read_escape_sequence_with<R, W>(mut read_byte: R, mut wait_ready: W) -> Vec<u8>
where
    R: FnMut() -> Vec<u8>,
    W: FnMut(Duration) -> bool,
{
    let mut sequence = vec![0x1b];
    while sequence.len() < 8 {
        if !wait_ready(Duration::from_millis(10)) {
            break;
        }
        let next = read_byte();
        if next.is_empty() {
            break;
        }
        let byte = next[0];
        sequence.push(byte);
        if sequence.len() == 2 && (byte == b'[' || byte == b'O') {
            continue;
        }
        if (0x40..=0x7e).contains(&byte) {
            break;
        }
    }
    sequence
}

#[derive(Debug)]
pub enum ReplReadError {
    Eof,
    Interrupted,
    Io(String),
}

impl std::fmt::Display for ReplReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Eof => write!(f, "EOF"),
            Self::Interrupted => write!(f, "interrupted"),
            Self::Io(message) => write!(f, "{}", message),
        }
    }
}

/// Read one REPL line from a plain (non-TTY) stream. Returns `Ok(None)` on idle timeout.
pub fn read_repl_line<R: Read, W: Write>(
    prompt: &str,
    timeout_seconds: f64,
    history: &[String],
    stdin: &mut R,
    stdout: &mut W,
) -> Result<Option<String>, ReplReadError> {
    read_repl_line_plain(prompt, timeout_seconds, history, stdin, stdout)
}

/// Read a REPL line, using process-stdin TTY raw mode when `prefer_process_tty` is set.
pub fn read_repl_line_auto<R: Read, W: Write>(
    prompt: &str,
    timeout_seconds: f64,
    history: &[String],
    stdin: &mut R,
    stdout: &mut W,
    prefer_process_tty: bool,
) -> Result<Option<String>, ReplReadError> {
    #[cfg(unix)]
    {
        use std::io::IsTerminal;
        use std::os::fd::AsRawFd;
        if prefer_process_tty && io::stdin().is_terminal() {
            return read_repl_line_tty(
                prompt,
                timeout_seconds,
                history,
                io::stdin().as_raw_fd(),
                stdout,
            );
        }
    }
    read_repl_line_plain(prompt, timeout_seconds, history, stdin, stdout)
}

fn read_repl_line_plain<R: Read, W: Write>(
    prompt: &str,
    _timeout_seconds: f64,
    _history: &[String],
    stdin: &mut R,
    stdout: &mut W,
) -> Result<Option<String>, ReplReadError> {
    write!(stdout, "{}", prompt).map_err(|error| ReplReadError::Io(error.to_string()))?;
    stdout
        .flush()
        .map_err(|error| ReplReadError::Io(error.to_string()))?;
    let mut reader = io::BufReader::new(stdin);
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .map_err(|error| ReplReadError::Io(error.to_string()))?;
    if read == 0 {
        return Err(ReplReadError::Eof);
    }
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
    Ok(Some(line))
}

/// Interactive TTY REPL entry used when stdin is a terminal.
#[cfg(unix)]
pub fn read_repl_line_tty<W: Write>(
    prompt: &str,
    timeout_seconds: f64,
    history: &[String],
    fd: i32,
    stdout: &mut W,
) -> Result<Option<String>, ReplReadError> {
    let previous = set_raw_mode(fd)?;
    let result = (|| {
        let mut editor = ReplLineEditor::new(prompt, stdout, history.to_vec());
        let mut decoder = Utf8Decoder::default();
        editor.start();
        loop {
            if timeout_seconds > 0.0 && !wait_fd_readable(fd, timeout_seconds) {
                drop(editor);
                write_raw_tty_newline(stdout);
                return Ok(None);
            }
            let data = read_tty_byte(fd)?;
            if data.is_empty() {
                return Err(ReplReadError::Eof);
            }
            let byte = data[0];
            if byte == b'\r' || byte == b'\n' {
                let text = editor.text();
                drop(editor);
                write_raw_tty_newline(stdout);
                return Ok(Some(text));
            }
            if byte == 0x03 {
                return Err(ReplReadError::Interrupted);
            }
            if byte == 0x04 {
                if editor.text().is_empty() {
                    return Err(ReplReadError::Eof);
                }
                continue;
            }
            if byte == 0x1b {
                decoder.reset();
                let sequence = read_escape_sequence_with(
                    || read_tty_byte(fd).unwrap_or_default(),
                    |timeout| wait_fd_readable(fd, timeout.as_secs_f64()),
                );
                handle_escape_sequence(&mut editor, &sequence);
                continue;
            }
            if byte == 0x7f || byte == 0x08 {
                decoder.reset();
                editor.backspace();
                continue;
            }
            if let Some(text) = decoder.push(byte) {
                editor.feed_text(&text);
            }
        }
    })();
    restore_mode(fd, previous)?;
    result
}

#[cfg(not(unix))]
pub fn read_repl_line_tty<W: Write>(
    prompt: &str,
    timeout_seconds: f64,
    history: &[String],
    _fd: i32,
    stdout: &mut W,
) -> Result<Option<String>, ReplReadError> {
    let mut empty: &[u8] = &[];
    read_repl_line_plain(prompt, timeout_seconds, history, &mut empty, stdout)
}

#[derive(Default)]
struct Utf8Decoder {
    buf: Vec<u8>,
}

impl Utf8Decoder {
    fn reset(&mut self) {
        self.buf.clear();
    }

    fn push(&mut self, byte: u8) -> Option<String> {
        self.buf.push(byte);
        match std::str::from_utf8(&self.buf) {
            Ok(text) => {
                let out = text.to_string();
                self.buf.clear();
                Some(out)
            }
            Err(error) if error.error_len().is_none() => None,
            Err(_) => {
                self.buf.clear();
                None
            }
        }
    }
}

#[cfg(unix)]
fn read_tty_byte(fd: i32) -> Result<Vec<u8>, ReplReadError> {
    let mut buf = [0u8; 1];
    loop {
        let read = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, 1) };
        if read < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(ReplReadError::Io(err.to_string()));
        }
        if read == 0 {
            return Ok(Vec::new());
        }
        return Ok(vec![buf[0]]);
    }
}

#[cfg(unix)]
fn wait_fd_readable(fd: i32, timeout_seconds: f64) -> bool {
    let mut fds = [libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }];
    let timeout_ms = if timeout_seconds <= 0.0 {
        0
    } else {
        (timeout_seconds * 1000.0).ceil() as i32
    };
    loop {
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, timeout_ms) };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return false;
        }
        return ready > 0;
    }
}

#[cfg(unix)]
fn set_raw_mode(fd: i32) -> Result<libc::termios, ReplReadError> {
    let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
        return Err(ReplReadError::Io(io::Error::last_os_error().to_string()));
    }
    let mut raw = original;
    raw.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
    raw.c_oflag &= !libc::OPOST;
    raw.c_cflag |= libc::CS8;
    raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
    raw.c_cc[libc::VMIN] = 1;
    raw.c_cc[libc::VTIME] = 0;
    if unsafe { libc::tcsetattr(fd, libc::TCSADRAIN, &raw) } != 0 {
        return Err(ReplReadError::Io(io::Error::last_os_error().to_string()));
    }
    Ok(original)
}

#[cfg(unix)]
fn restore_mode(fd: i32, previous: libc::termios) -> Result<(), ReplReadError> {
    if unsafe { libc::tcsetattr(fd, libc::TCSADRAIN, &previous) } != 0 {
        return Err(ReplReadError::Io(io::Error::last_os_error().to_string()));
    }
    Ok(())
}
