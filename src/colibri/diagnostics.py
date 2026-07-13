from __future__ import annotations

from pathlib import Path
import platform
import resource
import subprocess
import sys

from colibri.config import AgentConfig
from colibri.permissions_store import ProjectPermissionStore
from colibri.skills import SkillIndex


def build_diagnostics(
    config: AgentConfig,
    config_path: Path | None = None,
    cwd: Path | None = None,
) -> list[str]:
    memory_exists = config.memory.root.exists()
    skills_found = len(SkillIndex.scan(config.skills.dirs).skills)
    project_store = ProjectPermissionStore.for_cwd(cwd or Path.cwd())
    project_permissions = "present" if project_store.path.exists() else "missing"
    rss = rss_kb()
    lines = [
        "colibri diagnostics",
        f"python={platform.python_version()} platform={sys.platform}",
        f"provider={config.model.provider} model={config.model.model}",
        f"config={config_path if config_path is not None else 'default'}",
        f"tools={','.join(config.tools.enabled)}",
        f"memory_root={config.memory.root} exists={str(memory_exists).lower()}",
        f"skills_dirs={len(config.skills.dirs)} skills_found={skills_found}",
        f"project_permissions={project_permissions}",
        f"transcript={str(config.session.transcript).lower()} rss_kb={rss if rss is not None else 'unknown'}",
        (
            f"trigger_message_limit={config.session.trigger_message_limit} "
            f"recent_message_limit={config.session.recent_message_limit} "
            f"input_context_tokens={config.model.input_context_tokens} "
            f"summary_max_chars={config.session.summary_max_chars}"
        ),
    ]
    return lines


def rss_kb(pid: int | None = None) -> int | None:
    if pid is not None:
        return _pid_rss_kb(pid)

    proc_status = Path("/proc/self/status")
    try:
        for line in proc_status.read_text(encoding="utf-8").splitlines():
            if line.startswith("VmRSS:"):
                parts = line.split()
                if len(parts) >= 2:
                    return int(parts[1])
    except OSError:
        pass

    try:
        usage = resource.getrusage(resource.RUSAGE_SELF)
    except (OSError, AttributeError):
        return None
    value = int(usage.ru_maxrss)
    if sys.platform == "darwin":
        return max(1, value // 1024)
    return value if value > 0 else None


def _pid_rss_kb(pid: int) -> int | None:
    proc_status = Path("/proc") / str(pid) / "status"
    try:
        for line in proc_status.read_text(encoding="utf-8").splitlines():
            if line.startswith("VmRSS:"):
                parts = line.split()
                if len(parts) >= 2:
                    return int(parts[1])
    except OSError:
        pass
    try:
        result = subprocess.run(
            ["ps", "-o", "rss=", "-p", str(pid)],
            check=False,
            capture_output=True,
            text=True,
            timeout=2,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    text = result.stdout.strip()
    return int(text) if text.isdigit() else None
