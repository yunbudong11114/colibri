from colibri.config import AgentConfig
from colibri.diagnostics import build_diagnostics, rss_kb


def test_build_diagnostics_reports_core_fields(tmp_path):
    skill_dir = tmp_path / "skills" / "release"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text("# Release\n", encoding="utf-8")
    config = AgentConfig.default().with_overrides(
        {
            "memory": {"root": str(tmp_path / "memory")},
            "skills": {"dirs": [str(tmp_path / "skills")]},
        }
    )

    lines = build_diagnostics(config)

    joined = "\n".join(lines)
    assert lines[0] == "colibri diagnostics"
    assert "provider=fake model=fake-colibri-model" in joined
    assert f"memory_root={tmp_path / 'memory'} exists=false" in joined
    assert "skills_dirs=1 skills_found=2" in joined
    assert "trigger_message_limit=96 recent_message_limit=12 compact_trigger_chars=192000 summary_max_chars=12000" in joined


def test_diagnostics_reports_project_permissions_file(tmp_path):
    (tmp_path / ".colibri").mkdir()
    (tmp_path / ".colibri" / "permissions.toml").write_text(
        '[shell]\ncommands = ["pwd"]\n\n[tools]\nnames = []\n',
        encoding="utf-8",
    )
    config = AgentConfig.default()

    lines = build_diagnostics(config, None, cwd=tmp_path)

    assert "project_permissions=present" in "\n".join(lines)


def test_rss_kb_returns_integer_or_none():
    value = rss_kb()

    assert value is None or value > 0
