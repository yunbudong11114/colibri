from pathlib import Path

from colibri.gateway_process import GatewayProcessManager, format_gateway_status


def test_gateway_process_start_writes_state_and_uses_gateway_run(monkeypatch, tmp_path):
    captured = {}

    class FakeProcess:
        pid = 12345

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
    assert f"state={manager.state_path}" in lines
