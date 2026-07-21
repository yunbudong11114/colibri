from __future__ import annotations

from dataclasses import dataclass, field
import errno
import json
import os
from pathlib import Path
import select
import sys
import termios
import threading
import time
from typing import Any

from colibri.config import HardwareConfig, HardwareDeviceConfig


_LINUX_DEVICE_PATTERNS = {
    "audio": ("snd/*",),
    "camera": ("video*",),
    "display": ("fb*", "dri/card*"),
    "gpio": ("gpiochip*",),
    "i2c": ("i2c-*",),
    "iio": ("iio:device*",),
    "infrared": ("lirc*",),
    "input": ("input/event*",),
    "rtc": ("rtc*",),
    "serial": ("ttyS*", "ttyUSB*", "ttyACM*", "ttyAMA*", "rfcomm*"),
    "spi": ("spidev*",),
}
_MACOS_DEVICE_PATTERNS = {"serial": ("tty.*", "cu.*")}
_BAUD_CONSTANTS = {
    9600: termios.B9600,
    19200: termios.B19200,
    38400: termios.B38400,
    57600: termios.B57600,
    115200: termios.B115200,
    230400: termios.B230400,
}
_HARDWARE_IO_LOCK = threading.Lock()


class HardwareError(RuntimeError):
    def __init__(self, error_type: str, message: str):
        super().__init__(message)
        self.error_type = error_type


def configured_hardware_devices(config: HardwareConfig) -> list[dict[str, Any]]:
    return [
        {
            "name": device.name,
            "transport": device.transport,
            "baud_rate": device.baud_rate,
            "capabilities": list(device.capabilities),
            "allow_write": device.allow_write,
        }
        for device in config.devices
    ]


def resolve_hardware_device(
    config: HardwareConfig,
    name: str,
    *,
    capability: str,
    write: bool,
) -> HardwareDeviceConfig:
    device = next((candidate for candidate in config.devices if candidate.name == name), None)
    if device is None:
        raise HardwareError("unknown_device", f"Unknown hardware device: {name}")
    if capability not in device.capabilities:
        raise HardwareError(
            "unsupported_operation",
            f"Device {name} does not support {capability}",
        )
    if write and not device.allow_write:
        raise HardwareError("permission_denied", f"Hardware writes are disabled for device: {name}")
    return device


def serial_read(config: HardwareConfig, device: HardwareDeviceConfig, max_bytes: int) -> bytes:
    if not 1 <= max_bytes <= config.max_transfer_bytes:
        raise HardwareError("invalid_arguments", "max_bytes exceeds the configured transfer limit")
    with _HARDWARE_IO_LOCK:
        fd = _open_serial(device)
        try:
            return _read_available(fd, max_bytes, config.operation_timeout_seconds)
        finally:
            os.close(fd)


def serial_write(config: HardwareConfig, device: HardwareDeviceConfig, data: bytes) -> int:
    if not data:
        raise HardwareError("invalid_arguments", "data must not be empty")
    if len(data) > config.max_transfer_bytes:
        raise HardwareError("invalid_arguments", "data exceeds the configured transfer limit")
    with _HARDWARE_IO_LOCK:
        fd = _open_serial(device)
        try:
            _write_all(fd, data, config.operation_timeout_seconds)
            return len(data)
        finally:
            os.close(fd)


def serial_json_request(
    config: HardwareConfig,
    device: HardwareDeviceConfig,
    command: str,
    arguments: dict[str, Any],
) -> Any:
    request_id = str(time.monotonic_ns())
    request = {"id": request_id, "cmd": command, "args": arguments}
    payload = json.dumps(request, ensure_ascii=False, separators=(",", ":")).encode("utf-8") + b"\n"
    if len(payload) > config.max_transfer_bytes:
        raise HardwareError("invalid_arguments", "request exceeds the configured transfer limit")

    with _HARDWARE_IO_LOCK:
        fd = _open_serial(device)
        try:
            try:
                termios.tcflush(fd, termios.TCIFLUSH)
            except (OSError, termios.error) as error:
                raise HardwareError("io_error", str(error)) from None
            deadline = time.monotonic() + config.operation_timeout_seconds
            _write_all_until(fd, payload, deadline)
            response_bytes = _read_line_until(fd, config.max_transfer_bytes, deadline)
        finally:
            os.close(fd)

    try:
        response = json.loads(response_bytes)
    except (UnicodeDecodeError, json.JSONDecodeError):
        raise HardwareError("protocol_error", "Invalid controller response") from None
    if not isinstance(response, dict) or response.get("id") != request_id or not isinstance(response.get("ok"), bool):
        raise HardwareError("protocol_error", "Controller response did not match the request")
    if not response["ok"]:
        message = response.get("error")
        raise HardwareError("device_error", message if isinstance(message, str) else "Controller operation failed")
    if "result" not in response:
        raise HardwareError("protocol_error", "Controller response is missing result")
    return response["result"]


@dataclass
class HardwareSimulator:
    gpio_values: dict[int, int] = field(default_factory=dict)
    i2c_values: dict[tuple[int, int], bytes] = field(default_factory=dict)

    def handle_line(self, line: str) -> str:
        try:
            request = json.loads(line)
            response = self.handle_request(request)
        except (ValueError, TypeError, json.JSONDecodeError) as error:
            response = {"id": None, "ok": False, "error": str(error)}
        return json.dumps(response, ensure_ascii=False, separators=(",", ":"))

    def handle_request(self, request: Any) -> dict[str, Any]:
        if not isinstance(request, dict):
            raise ValueError("request must be an object")
        request_id = request.get("id")
        command = request.get("cmd")
        arguments = request.get("args", {})
        if not isinstance(request_id, str) or not request_id:
            raise ValueError("id must be a non-empty string")
        if not isinstance(command, str) or not isinstance(arguments, dict):
            raise ValueError("cmd and args are required")
        try:
            result = self._run(command, arguments)
        except ValueError as error:
            return {"id": request_id, "ok": False, "error": str(error)}
        return {"id": request_id, "ok": True, "result": result}

    def _run(self, command: str, arguments: dict[str, Any]) -> Any:
        if command == "gpio_read":
            pin = _bounded_int(arguments.get("pin"), "pin", 0, 65535)
            return {"value": self.gpio_values.get(pin, 0)}
        if command == "gpio_write":
            pin = _bounded_int(arguments.get("pin"), "pin", 0, 65535)
            value = _bounded_int(arguments.get("value"), "value", 0, 1)
            self.gpio_values[pin] = value
            return {"value": value}
        if command == "i2c_scan":
            return {"addresses": sorted({address for address, _ in self.i2c_values})}
        if command == "i2c_read":
            address = _bounded_int(arguments.get("address"), "address", 0, 127)
            register = _bounded_int(arguments.get("register", 0), "register", 0, 255)
            length = _bounded_int(arguments.get("length"), "length", 1, 65536)
            data = self.i2c_values.get((address, register), b"")
            return {"data": (data[:length] + b"\x00" * length)[:length].hex()}
        if command == "i2c_write":
            address = _bounded_int(arguments.get("address"), "address", 0, 127)
            register = _bounded_int(arguments.get("register", 0), "register", 0, 255)
            data = _hex_bytes(arguments.get("data"))
            self.i2c_values[(address, register)] = data
            return {"bytes_written": len(data)}
        if command == "spi_transfer":
            data = _hex_bytes(arguments.get("data"))
            return {"data": data.hex()}
        raise ValueError(f"unsupported command: {command}")


def probe_hardware(
    *,
    proc_root: Path = Path("/proc"),
    dev_root: Path = Path("/dev"),
    platform_name: str | None = None,
) -> dict[str, Any]:
    platform = _normalize_platform(platform_name or sys.platform)
    patterns = _LINUX_DEVICE_PATTERNS if platform == "linux" else _MACOS_DEVICE_PATTERNS if platform == "macos" else {}
    devices: dict[str, list[str]] = {}
    for capability, capability_patterns in patterns.items():
        paths = {
            _logical_device_path(path, dev_root)
            for pattern in capability_patterns
            for path in dev_root.glob(pattern)
            if path.exists()
        }
        if paths:
            devices[capability] = sorted(paths)

    return {
        "platform": platform,
        "board_model": _read_board_model(proc_root),
        "capabilities": sorted(devices),
        "devices": dict(sorted(devices.items())),
    }


def _normalize_platform(value: str) -> str:
    if value.startswith("linux"):
        return "linux"
    if value in {"darwin", "macos"}:
        return "macos"
    if value.startswith(("win", "cygwin")):
        return "windows"
    return value


def _logical_device_path(path: Path, dev_root: Path) -> str:
    try:
        relative = path.relative_to(dev_root)
    except ValueError:
        return str(path)
    return f"/dev/{relative.as_posix()}"


def _read_board_model(proc_root: Path) -> str | None:
    try:
        raw = (proc_root / "device-tree" / "model").read_bytes()
    except OSError:
        return None
    model = raw.decode("utf-8", errors="replace").strip("\x00\r\n ")
    return model or None


def _open_serial(device: HardwareDeviceConfig) -> int:
    try:
        fd = os.open(device.path, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    except FileNotFoundError:
        raise HardwareError("not_found", f"Hardware device does not exist: {device.name}") from None
    except OSError as error:
        raise HardwareError("io_error", str(error)) from None
    try:
        attributes = termios.tcgetattr(fd)
        attributes[0] = 0
        attributes[1] = 0
        attributes[2] = termios.CLOCAL | termios.CREAD | termios.CS8
        attributes[3] = 0
        attributes[4] = _BAUD_CONSTANTS[device.baud_rate]
        attributes[5] = _BAUD_CONSTANTS[device.baud_rate]
        attributes[6][termios.VMIN] = 0
        attributes[6][termios.VTIME] = 0
        termios.tcsetattr(fd, termios.TCSANOW, attributes)
    except (KeyError, OSError, termios.error) as error:
        os.close(fd)
        raise HardwareError("io_error", f"Failed to configure serial device: {error}") from None
    return fd


def _read_available(fd: int, max_bytes: int, timeout_seconds: float) -> bytes:
    deadline = time.monotonic() + timeout_seconds
    chunks = bytearray()
    while len(chunks) < max_bytes:
        _wait_fd(fd, reading=True, deadline=deadline)
        try:
            chunk = os.read(fd, max_bytes - len(chunks))
        except BlockingIOError:
            continue
        except OSError as error:
            raise HardwareError("io_error", str(error)) from None
        if not chunk:
            break
        chunks.extend(chunk)
        readable, _, _ = select.select([fd], [], [], 0)
        if not readable:
            break
    return bytes(chunks)


def _write_all(fd: int, data: bytes, timeout_seconds: float) -> None:
    _write_all_until(fd, data, time.monotonic() + timeout_seconds)


def _write_all_until(fd: int, data: bytes, deadline: float) -> None:
    offset = 0
    while offset < len(data):
        _wait_fd(fd, reading=False, deadline=deadline)
        try:
            written = os.write(fd, data[offset:])
        except BlockingIOError:
            continue
        except OSError as error:
            raise HardwareError("io_error", str(error)) from None
        if written == 0:
            raise HardwareError("io_error", "Serial device accepted zero bytes")
        offset += written


def _read_line_until(fd: int, max_bytes: int, deadline: float) -> bytes:
    buffer = bytearray()
    while True:
        _wait_fd(fd, reading=True, deadline=deadline)
        try:
            chunk = os.read(fd, min(1024, max_bytes + 1 - len(buffer)))
        except BlockingIOError:
            continue
        except OSError as error:
            raise HardwareError("io_error", str(error)) from None
        if not chunk:
            raise HardwareError("protocol_error", "Controller closed before sending a response")
        buffer.extend(chunk)
        newline = buffer.find(b"\n")
        if newline >= 0:
            if newline > max_bytes:
                raise HardwareError("result_too_large", "Controller response exceeds the configured transfer limit")
            return bytes(buffer[:newline])
        if len(buffer) > max_bytes:
            raise HardwareError("result_too_large", "Controller response exceeds the configured transfer limit")


def _wait_fd(fd: int, *, reading: bool, deadline: float) -> None:
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise HardwareError("timeout", "Hardware operation timed out")
    try:
        readable, writable, _ = select.select([fd] if reading else [], [] if reading else [fd], [], remaining)
    except OSError as error:
        if error.errno == errno.EINTR:
            return _wait_fd(fd, reading=reading, deadline=deadline)
        raise HardwareError("io_error", str(error)) from None
    if not (readable if reading else writable):
        raise HardwareError("timeout", "Hardware operation timed out")


def _bounded_int(value: Any, name: str, minimum: int, maximum: int) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or not minimum <= value <= maximum:
        raise ValueError(f"{name} must be an integer between {minimum} and {maximum}")
    return value


def _hex_bytes(value: Any) -> bytes:
    if not isinstance(value, str) or not value or len(value) % 2:
        raise ValueError("data must be a non-empty even-length hexadecimal string")
    try:
        return bytes.fromhex(value)
    except ValueError:
        raise ValueError("data must be a non-empty even-length hexadecimal string") from None
