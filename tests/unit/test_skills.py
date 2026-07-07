from colibri.config import AgentConfig
from colibri.skills import SkillIndex, SkillRunTool
from colibri.tools.base import ToolContext


def test_skill_index_scans_local_skills_without_storing_bodies(tmp_path):
    skill_dir = tmp_path / "skills" / "release"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text("# Release Notes\n\nUse this for release summaries.\n", encoding="utf-8")

    index = SkillIndex.scan([tmp_path / "skills"])

    assert [skill.name for skill in index.skills] == ["release"]
    assert index.skills[0].description == "Release Notes"
    assert index.skills[0].content is None


def test_skill_index_parses_command_metadata(tmp_path):
    skill_dir = tmp_path / "skills" / "release"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text("# Release Notes\n", encoding="utf-8")
    (skill_dir / "skill.toml").write_text(
        """
description = "Release helper"

[[commands]]
name = "render"
description = "Render notes"
command = "python"
args = ["scripts/render.py"]
read_only = false
""".strip(),
        encoding="utf-8",
    )

    index = SkillIndex.scan([tmp_path / "skills"])

    assert index.skills[0].description == "Release helper"
    assert index.skills[0].commands[0].name == "render"
    assert index.skills[0].commands[0].args == ["scripts/render.py"]
    assert not index.skills[0].commands[0].read_only


def test_skill_index_selects_and_loads_bounded_skill_context(tmp_path):
    skill_dir = tmp_path / "skills" / "release"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text("# Release Notes\n\n" + "release " * 100, encoding="utf-8")
    config = AgentConfig.default().with_overrides(
        {"skills": {"dirs": [str(tmp_path / "skills")], "max_loaded": 1, "max_instruction_chars": 80}}
    )

    context = SkillIndex.scan(config.skills.dirs).context_for("please write release notes", config.skills)

    assert context.text.startswith("Relevant skills:")
    assert "[release]" in context.text
    assert "Base directory:" in context.text
    assert context.skills == ["release"]
    assert context.truncated
    assert len(context.text) <= config.skills.max_instruction_chars + 80


def test_skill_run_executes_declared_local_command(tmp_path):
    skill_dir = tmp_path / "skills" / "release"
    scripts_dir = skill_dir / "scripts"
    scripts_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text("# Release Notes\n", encoding="utf-8")
    (scripts_dir / "render.py").write_text("print('rendered')\n", encoding="utf-8")
    (skill_dir / "skill.toml").write_text(
        """
[[commands]]
name = "render"
command = "python"
args = ["scripts/render.py"]
read_only = false
""".strip(),
        encoding="utf-8",
    )
    config = AgentConfig.default().with_overrides(
        {
            "skills": {"dirs": [str(tmp_path / "skills")]},
            "tools": {"max_result_chars": 100, "max_shell_seconds": 2},
        }
    )
    context = ToolContext(config=config, cwd=tmp_path)

    result = SkillRunTool().run({"skill": "release", "command": "render"}, context)

    assert result.ok
    assert result.text.strip() == "rendered"


def test_skill_run_rejects_missing_command(tmp_path):
    skill_dir = tmp_path / "skills" / "release"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text("# Release Notes\n", encoding="utf-8")
    config = AgentConfig.default().with_overrides({"skills": {"dirs": [str(tmp_path / "skills")]}})
    context = ToolContext(config=config, cwd=tmp_path)

    result = SkillRunTool().run({}, context)

    assert not result.ok
    assert result.error_type == "invalid_arguments"
