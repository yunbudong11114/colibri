use crate::config::{HardwareConfig, HardwareDeviceConfig};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static HARDWARE_IO_LOCK: Mutex<()> = Mutex::new(());

pub fn probe_hardware() -> serde_json::Value {
    probe_hardware_with_roots(Path::new("/proc"), Path::new("/dev"), std::env::consts::OS)
}

pub fn probe_hardware_with_roots(
    proc_root: &Path,
    dev_root: &Path,
    platform_name: &str,
) -> serde_json::Value {
    let platform = normalize_platform(platform_name);
    let mut devices = BTreeMap::<String, Vec<String>>::new();
    if platform == "linux" {
        add_matches(&mut devices, "audio", dev_root, "snd", |_| true);
        add_matches(&mut devices, "camera", dev_root, "", |name| {
            name.starts_with("video")
        });
        add_matches(&mut devices, "display", dev_root, "", |name| {
            name.starts_with("fb")
        });
        add_matches(&mut devices, "display", dev_root, "dri", |name| {
            name.starts_with("card")
        });
        add_matches(&mut devices, "gpio", dev_root, "", |name| {
            name.starts_with("gpiochip")
        });
        add_matches(&mut devices, "i2c", dev_root, "", |name| {
            name.starts_with("i2c-")
        });
        add_matches(&mut devices, "iio", dev_root, "", |name| {
            name.starts_with("iio:device")
        });
        add_matches(&mut devices, "infrared", dev_root, "", |name| {
            name.starts_with("lirc")
        });
        add_matches(&mut devices, "input", dev_root, "input", |name| {
            name.starts_with("event")
        });
        add_matches(&mut devices, "rtc", dev_root, "", |name| {
            name.starts_with("rtc")
        });
        add_matches(&mut devices, "serial", dev_root, "", |name| {
            ["ttyS", "ttyUSB", "ttyACM", "ttyAMA", "rfcomm"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
        });
        add_matches(&mut devices, "spi", dev_root, "", |name| {
            name.starts_with("spidev")
        });
    } else if platform == "macos" {
        add_matches(&mut devices, "serial", dev_root, "", |name| {
            name.starts_with("tty.") || name.starts_with("cu.")
        });
    }

    let capabilities = devices.keys().cloned().collect::<Vec<_>>();
    serde_json::json!({
        "platform": platform,
        "board_model": read_board_model(proc_root),
        "capabilities": capabilities,
        "devices": devices,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HardwareError {
    pub error_type: String,
    pub message: String,
}

impl HardwareError {
    fn new(error_type: &str, message: impl Into<String>) -> Self {
        Self {
            error_type: error_type.to_string(),
            message: message.into(),
        }
    }
}

impl fmt::Display for HardwareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

pub fn configured_hardware_devices(config: &HardwareConfig) -> serde_json::Value {
    serde_json::Value::Array(
        config
            .devices
            .iter()
            .map(|device| {
                serde_json::json!({
                    "name": device.name,
                    "transport": device.transport,
                    "baud_rate": device.baud_rate,
                    "capabilities": device.capabilities,
                    "allow_write": device.allow_write,
                })
            })
            .collect(),
    )
}

pub fn resolve_hardware_device<'a>(
    config: &'a HardwareConfig,
    name: &str,
    capability: &str,
    write: bool,
) -> Result<&'a HardwareDeviceConfig, HardwareError> {
    let device = config
        .devices
        .iter()
        .find(|candidate| candidate.name == name)
        .ok_or_else(|| {
            HardwareError::new(
                "unknown_device",
                format!("Unknown hardware device: {}", name),
            )
        })?;
    if !device
        .capabilities
        .iter()
        .any(|candidate| candidate == capability)
    {
        return Err(HardwareError::new(
            "unsupported_operation",
            format!("Device {} does not support {}", name, capability),
        ));
    }
    if write && !device.allow_write {
        return Err(HardwareError::new(
            "permission_denied",
            format!("Hardware writes are disabled for device: {}", name),
        ));
    }
    Ok(device)
}

pub fn serial_read(
    config: &HardwareConfig,
    device: &HardwareDeviceConfig,
    max_bytes: usize,
) -> Result<Vec<u8>, HardwareError> {
    if max_bytes == 0 || max_bytes > config.max_transfer_bytes {
        return Err(HardwareError::new(
            "invalid_arguments",
            "max_bytes exceeds the configured transfer limit",
        ));
    }
    let _guard = HARDWARE_IO_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut file = open_serial(device)?;
    read_available(
        &mut file,
        max_bytes,
        Duration::from_secs_f64(config.operation_timeout_seconds),
    )
}

pub fn serial_write(
    config: &HardwareConfig,
    device: &HardwareDeviceConfig,
    data: &[u8],
) -> Result<usize, HardwareError> {
    if data.is_empty() {
        return Err(HardwareError::new(
            "invalid_arguments",
            "data must not be empty",
        ));
    }
    if data.len() > config.max_transfer_bytes {
        return Err(HardwareError::new(
            "invalid_arguments",
            "data exceeds the configured transfer limit",
        ));
    }
    let _guard = HARDWARE_IO_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut file = open_serial(device)?;
    write_all_until(
        &mut file,
        data,
        Instant::now() + Duration::from_secs_f64(config.operation_timeout_seconds),
    )?;
    Ok(data.len())
}

pub fn serial_json_request(
    config: &HardwareConfig,
    device: &HardwareDeviceConfig,
    command: &str,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, HardwareError> {
    let request_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string();
    let mut payload = serde_json::to_vec(&serde_json::json!({
        "id": request_id,
        "cmd": command,
        "args": arguments,
    }))
    .map_err(|error| HardwareError::new("protocol_error", error.to_string()))?;
    payload.push(b'\n');
    if payload.len() > config.max_transfer_bytes {
        return Err(HardwareError::new(
            "invalid_arguments",
            "request exceeds the configured transfer limit",
        ));
    }

    let _guard = HARDWARE_IO_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut file = open_serial(device)?;
    flush_serial_input(&file)?;
    let deadline = Instant::now() + Duration::from_secs_f64(config.operation_timeout_seconds);
    write_all_until(&mut file, &payload, deadline)?;
    let response_bytes = read_line_until(&mut file, config.max_transfer_bytes, deadline)?;
    let response: serde_json::Value = serde_json::from_slice(&response_bytes)
        .map_err(|_| HardwareError::new("protocol_error", "Invalid controller response"))?;
    let object = response.as_object().ok_or_else(|| {
        HardwareError::new(
            "protocol_error",
            "Controller response did not match the request",
        )
    })?;
    if object.get("id").and_then(serde_json::Value::as_str) != Some(request_id.as_str())
        || object
            .get("ok")
            .and_then(serde_json::Value::as_bool)
            .is_none()
    {
        return Err(HardwareError::new(
            "protocol_error",
            "Controller response did not match the request",
        ));
    }
    if object.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
        let message = object
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Controller operation failed");
        return Err(HardwareError::new("device_error", message));
    }
    object.get("result").cloned().ok_or_else(|| {
        HardwareError::new("protocol_error", "Controller response is missing result")
    })
}

#[derive(Default)]
pub struct HardwareSimulator {
    gpio_values: BTreeMap<u64, u8>,
    i2c_values: BTreeMap<(u8, u8), Vec<u8>>,
}

impl HardwareSimulator {
    pub fn handle_line(&mut self, line: &str) -> String {
        let response = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(request) => self.handle_request(&request),
            Err(error) => serde_json::json!({"id":null,"ok":false,"error":error.to_string()}),
        };
        serde_json::to_string(&response).unwrap_or_else(|_| {
            "{\"id\":null,\"ok\":false,\"error\":\"serialization failed\"}".to_string()
        })
    }

    pub fn handle_request(&mut self, request: &serde_json::Value) -> serde_json::Value {
        let Some(object) = request.as_object() else {
            return serde_json::json!({"id":null,"ok":false,"error":"request must be an object"});
        };
        let Some(request_id) = object.get("id").and_then(serde_json::Value::as_str) else {
            return serde_json::json!({"id":null,"ok":false,"error":"id must be a non-empty string"});
        };
        if request_id.is_empty() {
            return serde_json::json!({"id":null,"ok":false,"error":"id must be a non-empty string"});
        }
        let Some(command) = object.get("cmd").and_then(serde_json::Value::as_str) else {
            return serde_json::json!({"id":request_id,"ok":false,"error":"cmd and args are required"});
        };
        let arguments = object.get("args").and_then(serde_json::Value::as_object);
        let Some(arguments) = arguments else {
            return serde_json::json!({"id":request_id,"ok":false,"error":"cmd and args are required"});
        };
        match self.run(command, arguments) {
            Ok(result) => serde_json::json!({"id":request_id,"ok":true,"result":result}),
            Err(error) => serde_json::json!({"id":request_id,"ok":false,"error":error}),
        }
    }

    fn run(
        &mut self,
        command: &str,
        arguments: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        match command {
            "gpio_read" => {
                let pin = bounded_u64(arguments.get("pin"), "pin", 0, 65535)?;
                Ok(serde_json::json!({"value":self.gpio_values.get(&pin).copied().unwrap_or(0)}))
            }
            "gpio_write" => {
                let pin = bounded_u64(arguments.get("pin"), "pin", 0, 65535)?;
                let value = bounded_u64(arguments.get("value"), "value", 0, 1)? as u8;
                self.gpio_values.insert(pin, value);
                Ok(serde_json::json!({"value":value}))
            }
            "i2c_scan" => {
                let addresses = self
                    .i2c_values
                    .keys()
                    .map(|(address, _)| *address)
                    .collect::<std::collections::BTreeSet<_>>();
                Ok(serde_json::json!({"addresses":addresses}))
            }
            "i2c_read" => {
                let address = bounded_u64(arguments.get("address"), "address", 0, 127)? as u8;
                let register = optional_bounded_u64(arguments.get("register"), "register", 0, 255)?
                    .unwrap_or(0) as u8;
                let length = bounded_u64(arguments.get("length"), "length", 1, 65536)? as usize;
                let stored = self
                    .i2c_values
                    .get(&(address, register))
                    .cloned()
                    .unwrap_or_default();
                let mut data = stored.into_iter().take(length).collect::<Vec<_>>();
                data.resize(length, 0);
                Ok(serde_json::json!({"data":hex_encode(&data)}))
            }
            "i2c_write" => {
                let address = bounded_u64(arguments.get("address"), "address", 0, 127)? as u8;
                let register = optional_bounded_u64(arguments.get("register"), "register", 0, 255)?
                    .unwrap_or(0) as u8;
                let data = hex_decode(arguments.get("data").and_then(serde_json::Value::as_str))?;
                let written = data.len();
                self.i2c_values.insert((address, register), data);
                Ok(serde_json::json!({"bytes_written":written}))
            }
            "spi_transfer" => {
                let data = hex_decode(arguments.get("data").and_then(serde_json::Value::as_str))?;
                Ok(serde_json::json!({"data":hex_encode(&data)}))
            }
            _ => Err(format!("unsupported command: {}", command)),
        }
    }
}

fn add_matches<F>(
    devices: &mut BTreeMap<String, Vec<String>>,
    capability: &str,
    dev_root: &Path,
    relative_dir: &str,
    matches: F,
) where
    F: Fn(&str) -> bool,
{
    let directory = if relative_dir.is_empty() {
        dev_root.to_path_buf()
    } else {
        dev_root.join(relative_dir)
    };
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut discovered = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !matches(&name) {
            continue;
        }
        let relative = if relative_dir.is_empty() {
            PathBuf::from(name)
        } else {
            Path::new(relative_dir).join(name)
        };
        discovered.push(format!("/dev/{}", relative.to_string_lossy()));
    }
    if discovered.is_empty() {
        return;
    }
    let values = devices.entry(capability.to_string()).or_default();
    values.extend(discovered);
    values.sort();
    values.dedup();
}

fn normalize_platform(value: &str) -> String {
    if value.starts_with("linux") {
        "linux".to_string()
    } else if matches!(value, "darwin" | "macos") {
        "macos".to_string()
    } else if value.starts_with("win") || value.starts_with("cygwin") {
        "windows".to_string()
    } else {
        value.to_string()
    }
}

fn read_board_model(proc_root: &Path) -> Option<String> {
    let bytes = fs::read(proc_root.join("device-tree").join("model")).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let model = text.trim_matches(|ch: char| ch == '\0' || ch.is_whitespace());
    (!model.is_empty()).then(|| model.to_string())
}

#[cfg(unix)]
fn open_serial(device: &HardwareDeviceConfig) -> Result<std::fs::File, HardwareError> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
        .open(&device.path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                HardwareError::new(
                    "not_found",
                    format!("Hardware device does not exist: {}", device.name),
                )
            } else {
                HardwareError::new("io_error", error.to_string())
            }
        })?;
    configure_serial(&file, device.baud_rate)?;
    Ok(file)
}

#[cfg(not(unix))]
fn open_serial(_device: &HardwareDeviceConfig) -> Result<std::fs::File, HardwareError> {
    Err(HardwareError::new(
        "io_error",
        "Serial hardware is only supported on Unix in this phase",
    ))
}

#[cfg(unix)]
fn configure_serial(file: &std::fs::File, baud_rate: u32) -> Result<(), HardwareError> {
    use std::os::fd::AsRawFd;

    let speed = baud_constant(baud_rate)
        .ok_or_else(|| HardwareError::new("io_error", "Unsupported serial baud rate"))?;
    let mut attributes = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(file.as_raw_fd(), &mut attributes) } != 0 {
        return Err(HardwareError::new(
            "io_error",
            format!(
                "Failed to configure serial device: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    unsafe {
        libc::cfmakeraw(&mut attributes);
    }
    attributes.c_cflag |= libc::CLOCAL | libc::CREAD;
    attributes.c_cflag &= !libc::CSIZE;
    attributes.c_cflag |= libc::CS8;
    if unsafe { libc::cfsetispeed(&mut attributes, speed) } != 0
        || unsafe { libc::cfsetospeed(&mut attributes, speed) } != 0
        || unsafe { libc::tcsetattr(file.as_raw_fd(), libc::TCSANOW, &attributes) } != 0
    {
        return Err(HardwareError::new(
            "io_error",
            format!(
                "Failed to configure serial device: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn baud_constant(baud_rate: u32) -> Option<libc::speed_t> {
    Some(match baud_rate {
        9600 => libc::B9600,
        19200 => libc::B19200,
        38400 => libc::B38400,
        57600 => libc::B57600,
        115200 => libc::B115200,
        230400 => libc::B230400,
        _ => return None,
    })
}

#[cfg(unix)]
fn flush_serial_input(file: &std::fs::File) -> Result<(), HardwareError> {
    use std::os::fd::AsRawFd;

    if unsafe { libc::tcflush(file.as_raw_fd(), libc::TCIFLUSH) } != 0 {
        return Err(HardwareError::new(
            "io_error",
            std::io::Error::last_os_error().to_string(),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn flush_serial_input(_file: &std::fs::File) -> Result<(), HardwareError> {
    Ok(())
}

fn read_available(
    file: &mut std::fs::File,
    max_bytes: usize,
    timeout: Duration,
) -> Result<Vec<u8>, HardwareError> {
    let deadline = Instant::now() + timeout;
    let mut output = Vec::new();
    while output.len() < max_bytes {
        wait_file(file, true, deadline)?;
        let mut buffer = vec![0; (max_bytes - output.len()).min(1024)];
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => output.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(HardwareError::new("io_error", error.to_string())),
        }
        if !file_ready(file, true, Duration::ZERO)? {
            break;
        }
    }
    Ok(output)
}

fn write_all_until(
    file: &mut std::fs::File,
    data: &[u8],
    deadline: Instant,
) -> Result<(), HardwareError> {
    let mut offset = 0;
    while offset < data.len() {
        wait_file(file, false, deadline)?;
        match file.write(&data[offset..]) {
            Ok(0) => {
                return Err(HardwareError::new(
                    "io_error",
                    "Serial device accepted zero bytes",
                ))
            }
            Ok(count) => offset += count,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(HardwareError::new("io_error", error.to_string())),
        }
    }
    Ok(())
}

fn read_line_until(
    file: &mut std::fs::File,
    max_bytes: usize,
    deadline: Instant,
) -> Result<Vec<u8>, HardwareError> {
    let mut output = Vec::new();
    loop {
        wait_file(file, true, deadline)?;
        let remaining = max_bytes.saturating_add(1).saturating_sub(output.len());
        let mut buffer = vec![0; remaining.min(1024).max(1)];
        match file.read(&mut buffer) {
            Ok(0) => {
                return Err(HardwareError::new(
                    "protocol_error",
                    "Controller closed before sending a response",
                ))
            }
            Ok(count) => output.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(HardwareError::new("io_error", error.to_string())),
        }
        if let Some(newline) = output.iter().position(|byte| *byte == b'\n') {
            if newline > max_bytes {
                return Err(HardwareError::new(
                    "result_too_large",
                    "Controller response exceeds the configured transfer limit",
                ));
            }
            output.truncate(newline);
            return Ok(output);
        }
        if output.len() > max_bytes {
            return Err(HardwareError::new(
                "result_too_large",
                "Controller response exceeds the configured transfer limit",
            ));
        }
    }
}

fn wait_file(file: &std::fs::File, reading: bool, deadline: Instant) -> Result<(), HardwareError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(HardwareError::new(
            "timeout",
            "Hardware operation timed out",
        ));
    }
    if file_ready(file, reading, remaining)? {
        Ok(())
    } else {
        Err(HardwareError::new(
            "timeout",
            "Hardware operation timed out",
        ))
    }
}

#[cfg(unix)]
fn file_ready(
    file: &std::fs::File,
    reading: bool,
    timeout: Duration,
) -> Result<bool, HardwareError> {
    use std::os::fd::AsRawFd;

    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    let mut descriptor = libc::pollfd {
        fd: file.as_raw_fd(),
        events: if reading { libc::POLLIN } else { libc::POLLOUT },
        revents: 0,
    };
    loop {
        let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if result > 0 {
            return Ok(true);
        }
        if result == 0 {
            return Ok(false);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(HardwareError::new("io_error", error.to_string()));
        }
    }
}

#[cfg(not(unix))]
fn file_ready(
    _file: &std::fs::File,
    _reading: bool,
    _timeout: Duration,
) -> Result<bool, HardwareError> {
    Err(HardwareError::new(
        "io_error",
        "Serial hardware is only supported on Unix in this phase",
    ))
}

fn bounded_u64(
    value: Option<&serde_json::Value>,
    name: &str,
    minimum: u64,
    maximum: u64,
) -> Result<u64, String> {
    let value = value.and_then(serde_json::Value::as_u64).ok_or_else(|| {
        format!(
            "{} must be an integer between {} and {}",
            name, minimum, maximum
        )
    })?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!(
            "{} must be an integer between {} and {}",
            name, minimum, maximum
        ));
    }
    Ok(value)
}

fn optional_bounded_u64(
    value: Option<&serde_json::Value>,
    name: &str,
    minimum: u64,
    maximum: u64,
) -> Result<Option<u64>, String> {
    value
        .map(|value| bounded_u64(Some(value), name, minimum, maximum))
        .transpose()
}

fn hex_decode(value: Option<&str>) -> Result<Vec<u8>, String> {
    let Some(value) = value else {
        return Err("data must be a non-empty even-length hexadecimal string".to_string());
    };
    if value.is_empty() || value.len() % 2 != 0 {
        return Err("data must be a non-empty even-length hexadecimal string".to_string());
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| "data must be a non-empty even-length hexadecimal string".to_string())
        })
        .collect()
}

fn hex_encode(data: &[u8]) -> String {
    let mut output = String::with_capacity(data.len() * 2);
    for byte in data {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{:02x}", byte);
    }
    output
}
