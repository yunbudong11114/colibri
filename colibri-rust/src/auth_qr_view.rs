//! Optional host-provided QR presenter; authentication remains platform independent.
#[cfg(unix)]
mod host {
    use std::io::{IsTerminal, Read, Write};
    use std::os::fd::AsRawFd;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    pub(crate) struct AuthQrView(Child);
    impl AuthQrView {
        pub(crate) fn ensure_open(&mut self) -> Result<(), String> {
            if self.0.try_wait().map_err(|e| e.to_string())?.is_some() {
                return Err("Weixin QR view closed; authentication cancelled".into());
            }
            Ok(())
        }
        pub(crate) fn open(payload: &str) -> Result<Option<Self>, String> {
            let Some(command) = std::env::var_os("COLIBRI_QR_VIEWER") else {
                return Ok(None);
            };
            if !std::io::stdout().is_terminal() {
                return Ok(None);
            }
            let command =
                shell_words::split(&command.to_string_lossy()).map_err(|e| e.to_string())?;
            let Some(executable) = command.first() else {
                return Err("Empty QR viewer command".into());
            };
            let matrix = crate::terminal_qr::qr_matrix(payload)
                .ok_or("Weixin QR payload cannot be encoded")?;
            let mut view = Self(
                Command::new(executable)
                    .args(&command[1..])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| format!("Cannot open host QR view: {e}"))?,
            );
            let setup = serde_json::json!({"matrix":matrix,
                "demo":payload.starts_with("https://example.invalid/colibri-demo")});
            writeln!(view.0.stdin.as_mut().unwrap(), "{setup}").map_err(|e| e.to_string())?;
            let output = view.0.stdout.as_mut().unwrap();
            let mut fd = libc::pollfd {
                fd: output.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut fd, 1, 3000) };
            let mut response = [0; 6];
            if ready <= 0 || output.read_exact(&mut response).is_err() || &response != b"ready\n" {
                return Err("Host QR presenter did not become ready".into());
            }
            Ok(Some(view))
        }
    }
    impl Drop for AuthQrView {
        fn drop(&mut self) {
            self.0.stdin.take(); // EOF returns the view to the CLI before auth prints its result.
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match self.0.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    _ if Instant::now() >= deadline => {
                        let _ = self.0.kill();
                        let _ = self.0.wait();
                        break;
                    }
                    _ => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        }
    }
}
#[cfg(unix)]
pub(crate) use host::AuthQrView;
#[cfg(not(unix))]
pub(crate) struct AuthQrView;
#[cfg(not(unix))]
impl AuthQrView {
    pub(crate) fn ensure_open(&mut self) -> Result<(), String> {
        Ok(())
    }
    pub(crate) fn open(_: &str) -> Result<Option<Self>, String> {
        Ok(None)
    }
}
