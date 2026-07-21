from __future__ import annotations

import json
from typing import Any

from colibri.hardware import (
    HardwareError,
    configured_hardware_devices,
    probe_hardware,
    resolve_hardware_device,
    serial_json_request,
    serial_read,
    serial_write,
)
from colibri.tools.base import ToolContext, ToolResult, ToolSpec, bound_tool_text


class HardwareProbeTool:
    spec = ToolSpec(
        name="hardware.probe",
        description="List host hardware capabilities and standard device nodes without opening the devices.",
        input_schema={"type": "object", "properties": {}, "additionalProperties": False},
        read_only=True,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        return _json_result(probe_hardware(), context)


class HardwareDevicesTool:
    spec = ToolSpec(
        name="hardware.devices",
        description="List configured hardware device aliases, capabilities, and write availability.",
        input_schema={"type": "object", "properties": {}, "additionalProperties": False},
        read_only=True,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        return _json_result(configured_hardware_devices(context.config.hardware), context)


class SerialReadTool:
    spec = ToolSpec(
        name="serial.read",
        description="Read currently available text from a configured serial device alias.",
        input_schema={
            "type": "object",
            "properties": {
                "device": {"type": "string"},
                "max_bytes": {"type": "integer", "minimum": 1},
            },
            "required": ["device"],
            "additionalProperties": False,
        },
        read_only=True,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        try:
            device_name = _required_string(arguments, "device")
            max_bytes = _optional_int(
                arguments,
                "max_bytes",
                default=context.config.hardware.max_transfer_bytes,
                minimum=1,
                maximum=context.config.hardware.max_transfer_bytes,
            )
            device = resolve_hardware_device(
                context.config.hardware,
                device_name,
                capability="serial",
                write=False,
            )
            data = serial_read(context.config.hardware, device, max_bytes)
            return _bounded_text_result(data.decode("utf-8", errors="replace"), context)
        except (HardwareError, ValueError) as error:
            return _error_result(error)


class SerialWriteTool:
    spec = ToolSpec(
        name="serial.write",
        description="Write UTF-8 text to a configured serial device alias.",
        input_schema={
            "type": "object",
            "properties": {"device": {"type": "string"}, "data": {"type": "string"}},
            "required": ["device", "data"],
            "additionalProperties": False,
        },
        read_only=False,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        try:
            device_name = _required_string(arguments, "device")
            data = _required_string(arguments, "data").encode("utf-8")
            device = resolve_hardware_device(
                context.config.hardware,
                device_name,
                capability="serial",
                write=True,
            )
            written = serial_write(context.config.hardware, device, data)
            return _json_result({"device": device_name, "bytes_written": written}, context)
        except (HardwareError, ValueError) as error:
            return _error_result(error)


class GpioReadTool:
    spec = ToolSpec(
        name="gpio.read",
        description="Read a GPIO pin through a configured serial JSON controller.",
        input_schema={
            "type": "object",
            "properties": {"device": {"type": "string"}, "pin": {"type": "integer", "minimum": 0}},
            "required": ["device", "pin"],
            "additionalProperties": False,
        },
        read_only=True,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        return _controller_tool(arguments, context, "gpio", False, "gpio_read", ("pin",))


class GpioWriteTool:
    spec = ToolSpec(
        name="gpio.write",
        description="Write a GPIO pin through a configured serial JSON controller.",
        input_schema={
            "type": "object",
            "properties": {
                "device": {"type": "string"},
                "pin": {"type": "integer", "minimum": 0},
                "value": {"type": "integer", "enum": [0, 1]},
            },
            "required": ["device", "pin", "value"],
            "additionalProperties": False,
        },
        read_only=False,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        return _controller_tool(arguments, context, "gpio", True, "gpio_write", ("pin", "value"))


class I2cScanTool:
    spec = ToolSpec(
        name="i2c.scan",
        description="Scan I2C addresses through a configured serial JSON controller.",
        input_schema={
            "type": "object",
            "properties": {"device": {"type": "string"}},
            "required": ["device"],
            "additionalProperties": False,
        },
        read_only=True,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        return _controller_tool(arguments, context, "i2c", False, "i2c_scan", ())


class I2cReadTool:
    spec = ToolSpec(
        name="i2c.read",
        description="Read bytes from an I2C address through a configured serial JSON controller.",
        input_schema={
            "type": "object",
            "properties": {
                "device": {"type": "string"},
                "address": {"type": "integer", "minimum": 0, "maximum": 127},
                "register": {"type": "integer", "minimum": 0, "maximum": 255},
                "length": {"type": "integer", "minimum": 1},
            },
            "required": ["device", "address", "length"],
            "additionalProperties": False,
        },
        read_only=True,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        return _controller_tool(
            arguments,
            context,
            "i2c",
            False,
            "i2c_read",
            ("address", "register", "length"),
        )


class I2cWriteTool:
    spec = ToolSpec(
        name="i2c.write",
        description="Write hexadecimal bytes to an I2C address through a configured serial JSON controller.",
        input_schema={
            "type": "object",
            "properties": {
                "device": {"type": "string"},
                "address": {"type": "integer", "minimum": 0, "maximum": 127},
                "register": {"type": "integer", "minimum": 0, "maximum": 255},
                "data": {"type": "string", "description": "Non-empty even-length hexadecimal bytes."},
            },
            "required": ["device", "address", "data"],
            "additionalProperties": False,
        },
        read_only=False,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        return _controller_tool(
            arguments,
            context,
            "i2c",
            True,
            "i2c_write",
            ("address", "register", "data"),
        )


class SpiTransferTool:
    spec = ToolSpec(
        name="spi.transfer",
        description="Transfer hexadecimal bytes through a configured serial JSON SPI controller.",
        input_schema={
            "type": "object",
            "properties": {
                "device": {"type": "string"},
                "data": {"type": "string", "description": "Non-empty even-length hexadecimal bytes."},
            },
            "required": ["device", "data"],
            "additionalProperties": False,
        },
        read_only=False,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        return _controller_tool(arguments, context, "spi", True, "spi_transfer", ("data",))


HARDWARE_TOOLS = (
    HardwareProbeTool,
    HardwareDevicesTool,
    SerialReadTool,
    SerialWriteTool,
    GpioReadTool,
    GpioWriteTool,
    I2cScanTool,
    I2cReadTool,
    I2cWriteTool,
    SpiTransferTool,
)


def _controller_tool(
    arguments: dict[str, Any],
    context: ToolContext,
    capability: str,
    write: bool,
    command: str,
    argument_names: tuple[str, ...],
) -> ToolResult:
    try:
        device_name = _required_string(arguments, "device")
        command_arguments = _controller_arguments(arguments, argument_names, context)
        device = resolve_hardware_device(
            context.config.hardware,
            device_name,
            capability=capability,
            write=write,
        )
        result = serial_json_request(context.config.hardware, device, command, command_arguments)
        return _json_result({"device": device_name, "result": result}, context)
    except (HardwareError, ValueError) as error:
        return _error_result(error)


def _controller_arguments(
    arguments: dict[str, Any],
    names: tuple[str, ...],
    context: ToolContext,
) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for name in names:
        if name == "register" and name not in arguments:
            continue
        if name == "data":
            data = _required_hex(arguments, name)
            if len(data) // 2 > context.config.hardware.max_transfer_bytes:
                raise ValueError("data exceeds the configured transfer limit")
            result[name] = data.lower()
        elif name == "value":
            result[name] = _required_int(arguments, name, 0, 1)
        elif name == "address":
            result[name] = _required_int(arguments, name, 0, 127)
        elif name == "register":
            result[name] = _required_int(arguments, name, 0, 255)
        elif name == "length":
            result[name] = _required_int(
                arguments,
                name,
                1,
                context.config.hardware.max_transfer_bytes,
            )
        else:
            result[name] = _required_int(arguments, name, 0, 65535)
    return result


def _required_string(arguments: dict[str, Any], name: str) -> str:
    value = arguments.get(name)
    if not isinstance(value, str) or not value:
        raise ValueError(f"{name} must be a non-empty string")
    return value


def _required_int(arguments: dict[str, Any], name: str, minimum: int, maximum: int) -> int:
    value = arguments.get(name)
    if not isinstance(value, int) or isinstance(value, bool) or not minimum <= value <= maximum:
        raise ValueError(f"{name} must be an integer between {minimum} and {maximum}")
    return value


def _optional_int(
    arguments: dict[str, Any],
    name: str,
    *,
    default: int,
    minimum: int,
    maximum: int,
) -> int:
    if name not in arguments:
        return default
    return _required_int(arguments, name, minimum, maximum)


def _required_hex(arguments: dict[str, Any], name: str) -> str:
    value = _required_string(arguments, name)
    if len(value) % 2:
        raise ValueError(f"{name} must be a non-empty even-length hexadecimal string")
    try:
        bytes.fromhex(value)
    except ValueError:
        raise ValueError(f"{name} must be a non-empty even-length hexadecimal string") from None
    return value


def _json_result(value: Any, context: ToolContext) -> ToolResult:
    return _bounded_text_result(json.dumps(value, ensure_ascii=False, indent=2), context)


def _bounded_text_result(text: str, context: ToolContext) -> ToolResult:
    bounded, truncated = bound_tool_text(text, context.config.tools.max_result_chars)
    return ToolResult(ok=True, text=bounded, truncated=truncated)


def _error_result(error: HardwareError | ValueError) -> ToolResult:
    error_type = error.error_type if isinstance(error, HardwareError) else "invalid_arguments"
    return ToolResult(ok=False, text=str(error), error_type=error_type)
