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
    pub(super) error: Option<String>,
}
impl Output {
    pub(super) fn append(&mut self, data: &[u8], dir: &Path) {
        self.total += data.len();
        let room = LOG_CAP.saturating_sub(self.bytes);
        let n = room.min(data.len());
        if self.head.write_all(&data[..n]).is_ok() {
            self.bytes += n;
        } else {
            self.error = Some("output_write_failed: saved log is incomplete".into());
        }
        self.tail.extend_from_slice(data);
        if self.tail.len() > TAIL_CAP {
            self.tail.drain(..self.tail.len() - TAIL_CAP);
        }
        if self.total > LOG_CAP && self.last_flush.elapsed() > Duration::from_millis(500) {
            if crate::config_io::atomic_write(&dir.join("tail.log"), &self.tail).is_err() {
                self.error = Some("tail_write_failed: saved log is incomplete".into());
            }
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
        let mut redactor = crate::secrets::StreamRedactor::new(secrets);
        let mut chunk = [0u8; 8192];
        loop {
            let n = match pipe.read(&mut chunk) {
                Ok(n) => n,
                Err(_) => {
                    if let Ok(mut s) = state.lock() {
                        s.error = Some("output_read_failed: saved log is incomplete".into());
                    }
                    0
                }
            };
            let output = redactor.feed(&chunk[..n], n == 0);
            if let Ok(mut s) = state.lock() {
                s.append(&output, &dir)
            }
            if n == 0 {
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_errors_are_explicit_instead_of_claiming_complete_output() {
        let dir = crate::config_io::Scratch::new(&std::env::temp_dir()).unwrap();
        let p = dir.0.join("readonly");
        fs::write(&p, "").unwrap();
        let mut state = Output {
            head: fs::File::open(&p).unwrap(),
            tail: Vec::new(),
            bytes: 0,
            total: 0,
            last_flush: Instant::now(),
            error: None,
        };
        state.append(b"command output", &dir.0);
        assert!(state
            .error
            .as_deref()
            .unwrap()
            .contains("output_write_failed"));
        assert_eq!(state.bytes, 0);
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("broken pipe"))
            }
        }
        let state = Arc::new(Mutex::new(state));
        reader(Broken, state.clone(), dir.0.clone(), vec![])
            .join()
            .unwrap();
        assert!(state
            .lock()
            .unwrap()
            .error
            .as_deref()
            .unwrap()
            .contains("output_read_failed"));
    }
}
