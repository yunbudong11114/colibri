use std::ffi::{CStr, CString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub(crate) struct Scratch(pub(crate) PathBuf);

impl Scratch {
    pub(crate) fn new(parent: &Path) -> io::Result<Self> {
        let pattern = CString::new(parent.join(".colibri-XXXXXX").as_os_str().as_bytes())?;
        let mut bytes = pattern.into_bytes_with_nul();
        let result = unsafe { libc::mkdtemp(bytes.as_mut_ptr().cast()) };
        if result.is_null() {
            return Err(io::Error::last_os_error());
        }
        let path = unsafe { CStr::from_ptr(result) }.to_bytes();
        Ok(Self(PathBuf::from(std::ffi::OsStr::from_bytes(path))))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn lock(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(file);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing parent directory"))?;
    let scratch = Scratch::new(parent)?;
    let temporary = scratch.0.join("value");
    let mut file = File::create(&temporary)?;
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(temporary, path)
}

/// Same advisory lock as `lock`, but lets an agent stop while another writer holds it.
pub(crate) fn lock_interruptible(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    loop {
        if crate::cancellation::cancelled() {
            return Err(io::Error::other(
                "cancelled while waiting for Memory write lock",
            ));
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(file);
        }
        let e = io::Error::last_os_error();
        if !matches!(
            e.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
        ) {
            return Err(e);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
