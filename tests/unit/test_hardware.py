import json
from io import StringIO
from dataclasses import replace
import os
from pathlib import Path
import threading

from colibri.cli import main
from colibri.config import AgentConfig, HardwareConfig, HardwareDeviceConfig
from colibri.hardware import (
    HardwareSimulator,
    configured_hardware_devices,
    probe_hardware,
    serial_json_request,
)
from colibri.messages import ToolCall
from colibri.tools.base import ToolContext
from colibri.tools.registry import ToolRegistry


def test_probe_hardware_detects_standard_linux_nodes(tmp_path):
    proc_root = tmp_path / "proc"
    dev_root = tmp_path / "dev"
    (proc_root / "device-tree").mkdir(parents=True)
    (proc_root / "device-tree" / "model").write_bytes(b"CardputerZero Test\x00")
    for relative in (
        "gpiochip0",
        "i2c-1",
        "spidev0.0",
        "ttyACM0",
        "video0",
        "input/event0",
        "iio:device0",
        "rtc0",
        "lirc0",
        "snd/controlC0",
        "dri/card0",
    ):
        path = dev_root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.touch()

    result = probe_hardware(proc_root=proc_root, dev_root=dev_root, platform_name="linux")

    assert result["platform"] == "linux"
    assert result["board_model"] == "CardputerZero Test"
    assert result["capabilities"] == [
        "audio",
        "camera",
        "display",
        "gpio",
        "i2c",
        "iio",
        "infrared",
        "input",
        "rtc",
        "serial",
        "spi",
    ]
    assert result["devices"]["serial"] == ["/dev/ttyACM0"]
    assert result["devices"]["display"] == ["/dev/dri/card0"]


def test_probe_hardware_returns_compact_empty_inventory(tmp_path):
    result = probe_hardware(
        proc_root=tmp_path / "proc",
        dev_root=tmp_path / "dev",
        platform_name="unsupported",
    )

    assert result == {
        "platform": "unsupported",
        "board_model": None,
        "capabilities": [],
        "devices": {},
    }


def test_hardware_probe_tool_requires_both_config_gates():
    base = AgentConfig.default()
    category_only = replace(base, tools=replace(base.tools, enabled=[*base.tools.enabled, "hardware"]))
    enabled_only = replace(base, hardware=HardwareConfig(enabled=True))
    both = replace(category_only, hardware=HardwareConfig(enabled=True))

    assert ToolRegistry.from_config(category_only).get("hardware.probe") is None
    assert ToolRegistry.from_config(enabled_only).get("hardware.probe") is None
    tool = ToolRegistry.from_config(both).get("hardware.probe")
    assert tool is not None
    assert tool.spec.read_only


def test_hardware_probe_cli_prints_json(monkeypatch, capsys):
    expected = {
        "platform": "linux",
        "board_model": "Test Board",
        "capabilities": ["gpio"],
        "devices": {"gpio": ["/dev/gpiochip0"]},
    }
    monkeypatch.setattr("colibri.cli.probe_hardware", lambda: expected)

    exit_code = main(["hardware", "probe"], config_loader=lambda _: AgentConfig.default())

    captured = capsys.readouterr()
    assert exit_code == 0
    assert json.loads(captured.out) == expected


def hardware_config(*, allow_write=True):
    return HardwareConfig(
        enabled=True,
        devices=[
            HardwareDeviceConfig(
                name="controller",
                path=Path("/dev/ttyACM0"),
                capabilities=["serial", "gpio", "i2c", "spi"],
                allow_write=allow_write,
            )
        ],
    )


def test_configured_hardware_devices_exposes_alias_not_path():
    result = configured_hardware_devices(hardware_config())

    assert result == [
        {
            "name": "controller",
            "transport": "serial_json",
            "baud_rate": 115200,
            "capabilities": ["serial", "gpio", "i2c", "spi"],
            "allow_write": True,
        }
    ]
    assert "/dev/ttyACM0" not in json.dumps(result)


def test_hardware_simulator_round_trips_gpio_i2c_and_spi():
    simulator = HardwareSimulator()

    assert simulator.handle_request(
        {"id": "1", "cmd": "gpio_write", "args": {"pin": 13, "value": 1}}
    ) == {"id": "1", "ok": True, "result": {"value": 1}}
    assert simulator.handle_request(
        {"id": "2", "cmd": "gpio_read", "args": {"pin": 13}}
    ) == {"id": "2", "ok": True, "result": {"value": 1}}
    assert simulator.handle_request(
        {"id": "3", "cmd": "i2c_write", "args": {"address": 32, "register": 1, "data": "a10b"}}
    )["ok"]
    assert simulator.handle_request(
        {"id": "4", "cmd": "i2c_scan", "args": {}}
    )["result"] == {"addresses": [32]}
    assert simulator.handle_request(
        {"id": "5", "cmd": "i2c_read", "args": {"address": 32, "register": 1, "length": 4}}
    )["result"] == {"data": "a10b0000"}
    assert simulator.handle_request(
        {"id": "6", "cmd": "spi_transfer", "args": {"data": "CAFE"}}
    )["result"] == {"data": "cafe"}


def test_serial_json_transport_round_trips_over_pty():
    master_fd, slave_fd = os.openpty()
    slave_path = Path(os.ttyname(slave_fd))
    config = HardwareConfig(
        enabled=True,
        operation_timeout_seconds=1,
        devices=[
            HardwareDeviceConfig(
                name="controller",
                path=slave_path,
                capabilities=["gpio"],
                allow_write=True,
            )
        ],
    )

    def controller():
        request = json.loads(_read_fd_line(master_fd))
        response = {
            "id": request["id"],
            "ok": True,
            "result": {"value": request["args"]["pin"] % 2},
        }
        os.write(master_fd, json.dumps(response, separators=(",", ":")).encode() + b"\n")

    thread = threading.Thread(target=controller)
    thread.start()
    try:
        result = serial_json_request(config, config.devices[0], "gpio_read", {"pin": 13})
    finally:
        thread.join(timeout=2)
        os.close(master_fd)
        os.close(slave_fd)

    assert result == {"value": 1}
    assert not thread.is_alive()


def test_hardware_tools_use_alias_and_controller_protocol(monkeypatch, tmp_path):
    base = AgentConfig.default()
    config = replace(
        base,
        tools=replace(base.tools, enabled=[*base.tools.enabled, "hardware"]),
        hardware=hardware_config(),
    )
    context = ToolContext(config=config, cwd=tmp_path)
    registry = ToolRegistry.from_config(config)
    calls = []

    def fake_request(hardware, device, command, arguments):
        calls.append((device.name, command, arguments))
        return {"value": 1}

    monkeypatch.setattr("colibri.tools.builtin.hardware.serial_json_request", fake_request)

    result = registry.run(
        ToolCall(id="call-1", name="gpio.read", arguments={"device": "controller", "pin": 13}),
        context,
    )

    assert result.ok
    assert json.loads(result.text) == {"device": "controller", "result": {"value": 1}}
    assert calls == [("controller", "gpio_read", {"pin": 13})]
    names = {spec["function"]["name"] for spec in registry.specs()}
    assert {
        "hardware.probe",
        "hardware.devices",
        "serial.read",
        "serial.write",
        "gpio.read",
        "gpio.write",
        "i2c.scan",
        "i2c.read",
        "i2c.write",
        "spi.transfer",
    } <= names


def test_hardware_tools_enforce_capability_write_and_transfer_limits(tmp_path):
    base = AgentConfig.default()
    hardware = HardwareConfig(
        enabled=True,
        max_transfer_bytes=2,
        devices=[
            HardwareDeviceConfig(
                name="sensor",
                path=Path("/dev/ttyACM0"),
                capabilities=["gpio"],
                allow_write=False,
            )
        ],
    )
    config = replace(
        base,
        tools=replace(base.tools, enabled=[*base.tools.enabled, "hardware"]),
        hardware=hardware,
    )
    context = ToolContext(config=config, cwd=tmp_path)
    registry = ToolRegistry.from_config(config)

    denied = registry.run(
        ToolCall(id="1", name="gpio.write", arguments={"device": "sensor", "pin": 1, "value": 1}),
        context,
    )
    unsupported = registry.run(
        ToolCall(id="2", name="i2c.scan", arguments={"device": "sensor"}),
        context,
    )
    too_large = registry.run(
        ToolCall(id="3", name="i2c.write", arguments={"device": "sensor", "address": 1, "data": "000102"}),
        context,
    )

    assert (denied.ok, denied.error_type) == (False, "permission_denied")
    assert (unsupported.ok, unsupported.error_type) == (False, "unsupported_operation")
    assert (too_large.ok, too_large.error_type) == (False, "invalid_arguments")


def test_hardware_simulator_cli_reads_and_writes_ndjson(monkeypatch, capsys):
    stdin = StringIO(
        '{"id":"1","cmd":"gpio_write","args":{"pin":2,"value":1}}\n'
        '{"id":"2","cmd":"gpio_read","args":{"pin":2}}\n'
    )
    monkeypatch.setattr("sys.stdin", stdin)

    exit_code = main(["hardware", "simulate"], config_loader=lambda _: AgentConfig.default())

    lines = capsys.readouterr().out.splitlines()
    assert exit_code == 0
    assert json.loads(lines[0]) == {"id": "1", "ok": True, "result": {"value": 1}}
    assert json.loads(lines[1]) == {"id": "2", "ok": True, "result": {"value": 1}}


def _read_fd_line(fd: int) -> bytes:
    data = bytearray()
    while not data.endswith(b"\n"):
        data.extend(os.read(fd, 1024))
    return bytes(data)
