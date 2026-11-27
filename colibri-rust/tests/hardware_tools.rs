#![cfg(unix)]
use colibri_rust::{
    config::{AgentConfig, HardwareDeviceConfig},
    tools::{run_tool, ToolContext},
};
use serde_json::{json, Value};
use std::{
    ffi::CStr,
    fs,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    path::PathBuf,
};
struct Fixture {
    home: PathBuf,
    master: fs::File,
    _slave: fs::File,
    context: ToolContext,
}
impl Fixture {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let home = std::env::temp_dir().join(format!(
            "colibri-hardware-tools-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&home).unwrap();
        let (mut m, mut s) = (-1, -1);
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut m,
                    &mut s,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let path = unsafe { CStr::from_ptr(libc::ttyname(s)) }
            .to_string_lossy()
            .to_string();
        let master = unsafe { fs::File::from_raw_fd(m) };
        let slave = unsafe { fs::File::from_raw_fd(s) };
        let mut attr = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(unsafe { libc::tcgetattr(s, &mut attr) }, 0);
        unsafe { libc::cfmakeraw(&mut attr) };
        assert_eq!(unsafe { libc::tcsetattr(s, libc::TCSANOW, &attr) }, 0);
        let mut config = AgentConfig::default();
        config.hardware.enabled = true;
        config.tools.enabled.push("hardware".into());
        config.hardware.operation_timeout_seconds = 0.1;
        config.hardware.max_transfer_bytes = 4096;
        config.model.api_key = "hardware-test-secret".into();
        config.hardware.devices.push(HardwareDeviceConfig {
            name: "test".into(),
            path: path.into(),
            transport: "serial_json".into(),
            baud_rate: 115200,
            capabilities: vec!["serial".into(), "gpio".into()],
        });
        let context = ToolContext::new(config, home.clone()).with_runtime_home(home.clone());
        Self {
            home,
            master,
            _slave: slave,
            context,
        }
    }
    fn call(&self, name: &str, args: Value) -> colibri_rust::messages::ToolResult {
        run_tool(name, &args.to_string(), &self.context).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.home);
    }
}
#[test]
fn serial_binary_read_is_archived_without_loss_and_does_not_reread() {
    let mut f = Fixture::new();
    std::sync::Arc::make_mut(&mut f.context.config)
        .tools
        .max_result_chars = 256;
    let data = (0..1000).map(|i| (i % 256) as u8).collect::<Vec<_>>();
    f.master.write_all(&data).unwrap();
    let r = f.call("serial.read", json!({"device":"test","max_bytes":1000}));
    assert!(r.ok, "{}", r.text);
    assert!(r.truncated);
    let v: Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["bytes_read"], 1000);
    let saved = fs::read_to_string(v["archive"]["path"].as_str().unwrap()).unwrap();
    let v: Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(v["encoding"], "hex");
    assert_eq!(
        v["data"].as_str().unwrap(),
        data.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let r = f.call("serial.read", json!({"device":"test","max_bytes":1}));
    assert!(!r.ok);
    assert_eq!(r.error_type.as_deref(), Some("timeout"));
}
#[test]
fn serial_text_and_hex_writes_report_transport_only_and_validate_before_io() {
    let mut f = Fixture::new();
    let bad = f.call(
        "serial.write",
        json!({"device":"test","data":"zz","encoding":"hex"}),
    );
    assert!(!bad.ok);
    assert_eq!(bad.error_type.as_deref(), Some("invalid_arguments"));
    let r = f.call(
        "serial.write",
        json!({"device":"test","data":"00ff4142","encoding":"hex"}),
    );
    assert!(r.ok);
    let v: Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["execution_confirmed"], false);
    assert_eq!(v["bytes_written"], 4);
    let mut bytes = [0; 4];
    f.master.read_exact(&mut bytes).unwrap();
    assert_eq!(bytes, [0, 255, 65, 66]);
    f.master.write_all("中文输入".as_bytes()).unwrap();
    let r = f.call(
        "serial.read",
        json!({"device":"test","max_bytes":12,"encoding":"text"}),
    );
    assert!(r.ok, "{}", r.text);
    let v: Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["data"], "中文输入");
}
#[test]
fn sent_controller_request_timeout_reports_unknown_and_never_auto_retries() {
    let mut f = Fixture::new();
    let r = f.call("gpio.write", json!({"device":"test","pin":2,"value":1}));
    assert!(!r.ok);
    assert_eq!(r.error_type.as_deref(), Some("timeout"));
    let v: Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["outcome_unknown"], true);
    assert!(v["bytes_written"].as_u64().unwrap() > 0);
    let fd = f.master.as_raw_fd();
    unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) };
    let mut captured = Vec::new();
    f.master.read_to_end(&mut captured).unwrap_err();
    assert_eq!(captured.iter().filter(|b| **b == b'\n').count(), 1);
}

#[test]
fn busy_device_is_not_written_before_the_lock_is_acquired() {
    let mut f = Fixture::new();
    assert_eq!(
        unsafe { libc::flock(f._slave.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let r = f.call(
        "serial.write",
        json!({"device":"test","data":"must-not-send"}),
    );
    assert!(!r.ok);
    assert_eq!(r.error_type.as_deref(), Some("device_busy"));
    let fd = f.master.as_raw_fd();
    unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) };
    let mut byte = [0u8; 1];
    assert_eq!(
        f.master.read(&mut byte).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
