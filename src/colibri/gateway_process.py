from __future__ import annotations

from dataclasses import dataclass
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import time

from colibri.diagnostics import rss_kb
from colibri.paths import colibri_home
from colibri.transcript import format_beijing_timestamp


@dataclass(frozen=True)
class GatewayProcessStatus:
    running: bool
    pid: int | None
    state_path: Path
    log_path: Path
    config_path: str
    cwd: str
    started_at: str
    agent_status: str = "healthy"
    rss_kb: int | None = None
    reason: str = ""


class GatewayProcessManager:
    def __init__(self, *, home: Path | None = None, cwd: Path | None = None):
        self.home = home or colibri_home()
        self.cwd = cwd or Path.cwd()
        self.run_dir = self.home / "run"
        self.log_dir = self.home / "logs"
        self.state_path = self.run_dir / "gateway.json"
        self.log_path = self.log_dir / "gateway.log"

    def start(self, config_path: Path | None = None) -> GatewayProcessStatus:
        status = self.status()
        if status.running:
            return status

        self.run_dir.mkdir(parents=True, exist_ok=True)
        self.log_dir.mkdir(parents=True, exist_ok=True)
        command = _gateway_run_command(config_path)
        with self.log_path.open("ab") as log:
            process = subprocess.Popen(
                command,
                cwd=self.cwd,
                stdout=log,
                stderr=subprocess.STDOUT,
                stdin=subprocess.DEVNULL,
                start_new_session=True,
            )
        state = {
            "pid": process.pid,
            "agent_status": "healthy",
            "config": str(config_path.expanduser()) if config_path is not None else "default",
            "cwd": str(self.cwd),
            "log": str(self.log_path),
            "started_at": format_beijing_timestamp(),
            "command": command,
        }
        self.state_path.write_text(json.dumps(state, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        return self.status()

    def stop(self, *, timeout_seconds: float = 5.0) -> GatewayProcessStatus:
        status = self.status()
        if not status.running or status.pid is None:
            return status
        if not _pid_matches_gateway(status.pid):
            return GatewayProcessStatus(
                running=status.running,
                pid=status.pid,
                state_path=status.state_path,
                log_path=status.log_path,
                config_path=status.config_path,
                cwd=status.cwd,
                started_at=status.started_at,
                rss_kb=status.rss_kb,
                reason="unverified_pid",
            )
        os.kill(status.pid, signal.SIGTERM)
        deadline = time.monotonic() + timeout_seconds
        while time.monotonic() < deadline:
            if not _pid_running(status.pid):
                return self.status()
            time.sleep(0.1)
        if _pid_running(status.pid):
            os.kill(status.pid, signal.SIGKILL)
        return self.status()

    def restart(self, config_path: Path | None = None) -> GatewayProcessStatus:
        self.stop()
        return self.start(config_path)

    def status(self) -> GatewayProcessStatus:
        state = self._load_state()
        pid = _int_or_none(state.get("pid"))
        running = _pid_running(pid) if pid is not None else False
        reason = ""
        if not self.state_path.exists():
            reason = "state_missing"
        elif not running:
            reason = "not_running"
        return GatewayProcessStatus(
            running=running,
            pid=pid,
            state_path=self.state_path,
            log_path=Path(str(state.get("log") or self.log_path)),
            config_path=str(state.get("config") or "default"),
            cwd=str(state.get("cwd") or ""),
            started_at=str(state.get("started_at") or ""),
            agent_status=str(state.get("agent_status") or "healthy") if running else "unhealthy",
            rss_kb=_rss_kb(pid) if running and pid is not None else None,
            reason=reason,
        )

    def _load_state(self) -> dict:
        try:
            return json.loads(self.state_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return {}


def format_gateway_status(status: GatewayProcessStatus) -> list[str]:
    lines = [
        f"running={str(status.running).lower()}",
        f"agent_status={getattr(status, 'agent_status', 'healthy' if status.running else 'unhealthy')}",
        f"pid={status.pid if status.pid is not None else 'unknown'}",
        f"rss_kb={status.rss_kb if status.rss_kb is not None else 'unknown'}",
        f"config={status.config_path}",
        f"cwd={status.cwd or 'unknown'}",
        f"log={status.log_path}",
        f"state={status.state_path}",
    ]
    if status.started_at:
        lines.append(f"started_at={status.started_at}")
    if status.reason:
        lines.append(f"reason={status.reason}")
    return lines


def update_gateway_agent_status(status: str, *, home: Path | None = None) -> None:
    if status not in {"healthy", "unhealthy"}:
        raise ValueError(f"invalid agent status: {status}")
    state_path = (home or colibri_home()) / "run" / "gateway.json"
    try:
        state = json.loads(state_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return
    if _int_or_none(state.get("pid")) != os.getpid():
        return
    state["agent_status"] = status
    temporary = state_path.with_suffix(".json.tmp")
    try:
        temporary.write_text(json.dumps(state, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        temporary.replace(state_path)
    except OSError:
        try:
            temporary.unlink()
        except OSError:
            pass


class GatewayAgentHealth:
    def __init__(self, *, home: Path | None = None, initial: str = "healthy"):
        self.home = home
        self._status = initial
        self._lock = threading.Lock()

    @property
    def status(self) -> str:
        with self._lock:
            return self._status

    def report(self, status: str) -> None:
        if status not in {"healthy", "unhealthy"}:
            raise ValueError(f"invalid agent status: {status}")
        with self._lock:
            if status == self._status:
                return
            update_gateway_agent_status(status, home=self.home)
            self._status = status


def _gateway_run_command(config_path: Path | None) -> list[str]:
    command = [sys.executable, "-m", "colibri.cli"]
    if config_path is not None:
        command.extend(["--config", str(config_path)])
    command.extend(["gateway", "run"])
    return command


def _rss_kb(pid: int) -> int | None:
    return rss_kb(pid)


def _pid_running(pid: int | None) -> bool:
    if pid is None or pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def _pid_matches_gateway(pid: int) -> bool:
    command = _pid_command(pid)
    if command is None:
        return False
    return "colibri.cli" in command and "gateway" in command and "run" in command


def _pid_command(pid: int) -> str | None:
    proc_cmdline = Path("/proc") / str(pid) / "cmdline"
    try:
        text = proc_cmdline.read_bytes().replace(b"\x00", b" ").decode("utf-8", errors="replace").strip()
        if text:
            return text
    except OSError:
        pass
    try:
        result = subprocess.run(
            ["ps", "-o", "command=", "-p", str(pid)],
            check=False,
            capture_output=True,
            text=True,
            timeout=2,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    text = result.stdout.strip()
    return text or None


def _int_or_none(value) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None
