//! Bounded output capture and streaming credential redaction.
use super::{LOG_CAP, TAIL_CAP};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
pub(super) struct Output {
    pub(super) head: fs::File,
    pub(super) tail: Vec<u8>,
    pub(super) bytes: usize,
    pub(super) total: usize,
    pub(super) last_flush: Instant,
}
impl Output {
    pub(super) fn append(&mut self, data: &[u8], dir: &Path) {
        self.total += data.len();
        let room = LOG_CAP.saturating_sub(self.bytes);
        let n = room.min(data.len());
        if self.head.write_all(&data[..n]).is_ok() {
            self.bytes += n
        }
        self.tail.extend_from_slice(data);
        if self.tail.len() > TAIL_CAP {
            self.tail.drain(..self.tail.len() - TAIL_CAP);
        }
        if self.total > LOG_CAP && self.last_flush.elapsed() > Duration::from_millis(500) {
            let _ = crate::config_io::atomic_write(&dir.join("tail.log"), &self.tail);
            self.last_flush = Instant::now();
        }
    }
}
pub(super) fn reader<R: Read + Send + 'static>(
    mut pipe: R,
    state: Arc<Mutex<Output>>,
    dir: PathBuf,
    secrets: Vec<String>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let guard = secrets.iter().map(String::len).max().unwrap_or(1).max(1) - 1;
        let mut pending = Vec::new();
        let mut chunk = [0u8; 8192];
        let mut output = Vec::with_capacity(8192);
        loop {
            let n = match pipe.read(&mut chunk) {
                Ok(n) => n,
                Err(_) => 0,
            };
            pending.extend_from_slice(&chunk[..n]);
            let emit = if n == 0 {
                pending.len()
            } else {
                pending.len().saturating_sub(guard)
            };
            output.clear();
            let mut pos = 0;
            while pos < emit {
                if let Some(secret) = secrets
                    .iter()
                    .find(|s| !s.is_empty() && pending[pos..].starts_with(s.as_bytes()))
                {
                    output.extend_from_slice(b"<redacted>");
                    pos += secret.len()
                } else {
                    output.push(pending[pos]);
                    pos += 1
                }
            }
            pending.drain(..pos);
            if let Ok(mut s) = state.lock() {
                s.append(&output, &dir)
            }
            if n == 0 {
                break;
            }
        }
    })
}
