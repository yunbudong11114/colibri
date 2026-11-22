import json
from pathlib import Path
import re
import signal
import subprocess

from colibri.gateway_logging import format_gateway_log
from colibri.gateway_process import GatewayAgentHealth, GatewayProcessManager, format_gateway_status


def test_gateway_process_start_writes_state_and_uses_gateway_run(monkeypatch, tmp_path):
    captured = {}

    class FakeProcess:
        pid = 12345

        def wait(self, timeout):
            raise subprocess.TimeoutExpired("gateway", timeout)

    def fake_popen(command, cwd, stdout, stderr, stdin, start_new_session):
        captured["command"] = command
        captured["cwd"] = cwd
        captured["start_new_session"] = start_new_session
        return FakeProcess()

    monkeypatch.setattr("colibri.gateway_process.subprocess.Popen", fake_popen)
    monkeypatch.setattr("colibri.gateway_process._pid_running", lambda pid: pid == 12345)
    monkeypatch.setattr("colibri.gateway_process._rss_kb", lambda pid: 2048)
    manager = GatewayProcessManager(home=tmp_path / "home", cwd=tmp_path / "project")

    status = manager.start(Path("config.toml"))

    assert status.running
    assert status.pid == 12345
    assert captured["command"][-2:] == ["gateway", "run"]
    assert "--config" in captured["command"]
    assert captured["start_new_session"]
    assert manager.state_path.exists()
    assert manager.log_path.exists()


def test_gateway_process_status_handles_missing_state(tmp_path):
    manager = GatewayProcessManager(home=tmp_path / "home", cwd=tmp_path / "project")

    status = manager.status()

    assert not status.running
    assert status.pid is None
    assert status.reason == "state_missing"


def test_format_gateway_status_is_key_value(tmp_path):
    manager = GatewayProcessManager(home=tmp_path / "home", cwd=tmp_path / "project")
    status = manager.status()

    lines = format_gateway_status(status)

    assert "running=false" in lines
    assert "agent_status=unhealthy" in lines
    assert f"state={manager.state_path}" in lines


def test_gateway_process_stop_refuses_unverified_pid(monkeypatch, tmp_path):
    kills = []
    manager = GatewayProcessManager(home=tmp_path / "home", cwd=tmp_path / "project")
    manager.run_dir.mkdir(parents=True)
    manager.state_path.write_text(
        json.dumps({"pid": 12345, "config": "default", "cwd": str(tmp_path), "log": str(manager.log_path)}),
        encoding="utf-8",
    )
    monkeypatch.setattr("colibri.gateway_process._pid_running", lambda pid: pid == 12345)
    monkeypatch.setattr("colibri.gateway_process._pid_matches_gateway", lambda pid: False)
    monkeypatch.setattr("colibri.gateway_process._rss_kb", lambda pid: 1024)
    monkeypatch.setattr("colibri.gateway_process.os.kill", lambda pid, sig: kills.append((pid, sig)))

    status = manager.stop()

    assert status.running
    assert status.pid == 12345
    assert status.reason == "unverified_pid"
    assert kills == []
    def unexpected_start(*_):
        raise AssertionError("restart must not start after a failed stop")

    monkeypatch.setattr(manager, "start", unexpected_start)
    assert manager.restart().reason == "unverified_pid"


def test_gateway_process_stop_terminates_verified_gateway_pid(monkeypatch, tmp_path):
    kills = []
    manager = GatewayProcessManager(home=tmp_path / "home", cwd=tmp_path / "project")
    manager.run_dir.mkdir(parents=True)
    manager.state_path.write_text(
        json.dumps({"pid": 12345, "config": "default", "cwd": str(tmp_path), "log": str(manager.log_path)}),
        encoding="utf-8",
    )

    def fake_pid_running(pid):
        return pid == 12345 and not kills

    monkeypatch.setattr("colibri.gateway_process._pid_running", fake_pid_running)
    monkeypatch.setattr("colibri.gateway_process._pid_matches_gateway", lambda pid: True)
    monkeypatch.setattr("colibri.gateway_process._rss_kb", lambda pid: 1024)
    monkeypatch.setattr("colibri.gateway_process.os.kill", lambda pid, sig: kills.append((pid, sig)))

    status = manager.stop()

    assert not status.running
    assert kills


def test_gateway_stop_waits_for_exit_after_sigkill(monkeypatch, tmp_path):
    manager = GatewayProcessManager(home=tmp_path)
    manager.run_dir.mkdir()
    manager.state_path.write_text('{"pid":12345}')
    checks_after_kill = []
    signals = []

    def pid_running(pid):
        if signals == [signal.SIGTERM, signal.SIGKILL]:
            checks_after_kill.append(pid)
            return len(checks_after_kill) == 1
        return True

    monkeypatch.setattr("colibri.gateway_process._pid_running", pid_running)
    monkeypatch.setattr("colibri.gateway_process._pid_matches_gateway", lambda pid: True)
    monkeypatch.setattr("colibri.gateway_process._rss_kb", lambda pid: None)
    monkeypatch.setattr("colibri.gateway_process.os.kill", lambda pid, sig: signals.append(sig))

    assert not manager.stop(timeout_seconds=0).running
    assert signals == [signal.SIGTERM, signal.SIGKILL]
    assert len(checks_after_kill) >= 2


def test_zombie_gateway_is_not_running(monkeypatch, tmp_path):
    manager = GatewayProcessManager(home=tmp_path)
    manager.run_dir.mkdir()
    manager.state_path.write_text('{"pid":12345}')
    monkeypatch.setattr("colibri.gateway_process._ps_field", lambda pid, field: "Z+")

    assert not manager.stop().running


def test_gateway_agent_health_persists_only_on_state_change(monkeypatch):
    writes = []
    monkeypatch.setattr(
        "colibri.gateway_process.update_gateway_agent_status",
        lambda status, home=None: writes.append(status),
    )
    health = GatewayAgentHealth()

    health.report("healthy")
    health.report("unhealthy")
    health.report("unhealthy")
    health.report("healthy")

    assert writes == ["unhealthy", "healthy"]


def test_gateway_log_lines_include_beijing_timestamp():
    line = format_gateway_log("started pid=123")

    assert re.fullmatch(
        r"\[20\d\d-\d\d-\d\dT\d\d:\d\d:\d\d\+08:00\] \[gateway\] started pid=123",
        line,
    )
