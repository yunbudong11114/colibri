from colibri.config import AgentConfig, ConfigError
from colibri.skills import SkillCommand, SkillIndex
from colibri.tools.base import ToolContext
from colibri.tools.builtin import SkillReadTool, SkillRunTool
from pathlib import Path
import pytest


def skill_document(
    name: str,
    description: str = "Release helper",
    commands: str = "",
    body: str = "# Release Notes\n",
) -> str:
    return f"""---
name: {name}
description: {description}
{commands}---

{body}"""


def test_skill_index_scans_local_skills_without_storing_bodies(tmp_path):
    skill_dir = tmp_path / "skills" / "release"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text(skill_document("release"), encoding="utf-8")

    index = SkillIndex.scan(tmp_path / "skills")

    release = index.get("release")
    assert release is not None
    assert release.description == "Release helper"
    assert release.content is None


def test_skill_index_includes_builtin_create_colibri_skill_without_user_dir(tmp_path):
    index = SkillIndex.scan(tmp_path / "missing-skills")

    skill = index.get("create-colibri-skill")

    assert skill is not None
    assert skill.root.name == "builtin"
    assert skill.content is not None
    assert not skill.commands


def test_skill_catalog_includes_builtin_and_local_without_bodies(tmp_path):
    skill_dir = tmp_path / "skills" / "release"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text(
        skill_document(
            "release",
            commands="""commands:
  - name: render
    description: Render notes
    command: python
    args: [scripts/render.py]
    read_only: true
""",
        ),
        encoding="utf-8",
    )
    config = AgentConfig.default().with_overrides({"skills": {"dir": str(tmp_path / "skills")}})

    context = SkillIndex.scan(config.skills.dir).catalog(config.skills)

    assert context.skills[0] == "create-colibri-skill"
    assert "release" in context.skills
    assert context.text.startswith("Available skills")
    assert "skill.read" in context.text
    assert "release:" in context.text
    assert "Commands: render" in context.text
    assert "use skill.run" in context.text
    assert "shell.run" in context.text
    assert "[release]" not in context.text


def test_skill_catalog_is_bounded(tmp_path):
    for name in ("alpha", "beta", "gamma"):
        skill_dir = tmp_path / "skills" / name
        skill_dir.mkdir(parents=True)
        (skill_dir / "SKILL.md").write_text(
            skill_document(name, description="desc " * 40),
            encoding="utf-8",
        )
    config = AgentConfig.default().with_overrides(
        {"skills": {"dir": str(tmp_path / "skills"), "max_catalog": 2, "max_catalog_chars": 120}}
    )

    context = SkillIndex.scan(config.skills.dir).catalog(config.skills)

    assert len(context.skills) <= 2
    assert context.truncated
    assert len(context.text) <= config.skills.max_catalog_chars + 80


def test_skill_read_returns_bounded_body(tmp_path):
    skill_dir = tmp_path / "skills" / "release"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text(
        skill_document("release", body="# Release Notes\n\n" + "release " * 100),
        encoding="utf-8",
    )
    config = AgentConfig.default().with_overrides(
        {"skills": {"dir": str(tmp_path / "skills"), "max_instruction_chars": 80}}
    )
    context = ToolContext(config=config, cwd=tmp_path)

    result = SkillReadTool().run({"name": "release"}, context)

    assert result.ok
    assert result.text.startswith("[release]")
    assert "Base directory:" in result.text
    assert result.truncated
    assert len(result.text) <= config.skills.max_instruction_chars + 80


def test_skill_read_rejects_unknown_name(tmp_path):
    config = AgentConfig.default().with_overrides({"skills": {"dir": str(tmp_path / "skills")}})
    context = ToolContext(config=config, cwd=tmp_path)

    result = SkillReadTool().run({"name": "missing"}, context)

    assert not result.ok
    assert result.error_type == "not_found"


def test_skill_index_parses_command_metadata(tmp_path):
    skill_dir = tmp_path / "skills" / "release"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text(
        skill_document(
            "release",
            description=">\n  Release helper\n  with details.",
            commands="""commands:
  - name: render
    description: Render notes
    command: python
    args:
      - scripts/render.py
      - --verbose
    read_only: true
""",
        ),
        encoding="utf-8",
    )
    (skill_dir / "skill.toml").write_text(
        """
[[commands]]
name = "ignored"
command = "false"
read_only = false
""".strip(),
        encoding="utf-8",
    )

    index = SkillIndex.scan(tmp_path / "skills")

    release = index.get("release")
    assert release is not None
    assert release.description == "Release helper with details.\n"
    assert release.commands == [
        SkillCommand(
            name="render",
            description="Render notes",
            command="python",
            args=["scripts/render.py", "--verbose"],
            read_only=True,
        )
    ]


def test_skill_run_executes_declared_local_command(tmp_path):
    import sys

    skill_dir = tmp_path / "skills" / "release"
    scripts_dir = skill_dir / "scripts"
    scripts_dir.mkdir(parents=True)
    (scripts_dir / "render.py").write_text("print('rendered')\n", encoding="utf-8")
    (skill_dir / "SKILL.md").write_text(
        skill_document(
            "release",
            commands=f"""commands:
  - name: render
    command: {sys.executable}
    args: [scripts/render.py]
    read_only: false
""",
        ),
        encoding="utf-8",
    )
    config = AgentConfig.default().with_overrides(
        {
            "skills": {"dir": str(tmp_path / "skills")},
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
    (skill_dir / "SKILL.md").write_text(skill_document("release"), encoding="utf-8")
    config = AgentConfig.default().with_overrides({"skills": {"dir": str(tmp_path / "skills")}})
    context = ToolContext(config=config, cwd=tmp_path)

    result = SkillRunTool().run({}, context)

    assert not result.ok
    assert result.error_type == "invalid_arguments"


@pytest.mark.parametrize(
    "document",
    [
        "# Missing frontmatter\n",
        "---\ndescription: missing name\n---\n",
        "---\nname: other\ndescription: mismatch\n---\n",
        "---\nname: release\ndescription: ''\n---\n",
        "---\nname: release\ndescription: ok\ncommands: invalid\n---\n",
        "---\nname: release\ndescription: ok\ncommands:\n  - name: render\n---\n",
        (
            "---\nname: release\ndescription: ok\ncommands:\n"
            "  - name: render\n    command: python\n"
            "  - name: render\n    command: python\n---\n"
        ),
    ],
)
def test_skill_index_skips_invalid_yaml_skill(tmp_path, document):
    skill_dir = tmp_path / "skills" / "release"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text(document, encoding="utf-8")

    index = SkillIndex.scan(tmp_path / "skills")

    assert index.get("release") is None


def test_skill_read_lists_configured_commands(tmp_path):
    skill_dir = tmp_path / "skills" / "release"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text(
        skill_document(
            "release",
            commands="""commands:
  - name: render
    description: Render notes
    command: python
""",
        ),
        encoding="utf-8",
    )
    config = AgentConfig.default().with_overrides({"skills": {"dir": str(tmp_path / "skills")}})
    context = ToolContext(config=config, cwd=tmp_path)

    result = SkillReadTool().run({"name": "release"}, context)

    assert result.ok
    assert "Configured commands:\n- render: Render notes" in result.text


def test_builtin_creation_skill_uses_yaml_frontmatter():
    skill = SkillIndex.scan(Path("/missing")).get("create-colibri-skill")

    assert skill is not None
    assert skill.content is not None
    assert skill.content.startswith("---\nname: create-colibri-skill\n")
    assert "commands:" in skill.content
    assert "Do not create `skill.toml`." in skill.content


def test_skills_dirs_config_is_rejected():
    with pytest.raises(ConfigError, match=r"skills\.dirs"):
        AgentConfig.default().with_overrides({"skills": {"dirs": ["~/skills"]}})


def test_skills_max_loaded_config_is_rejected():
    with pytest.raises(ConfigError, match=r"skills\.max_loaded"):
        AgentConfig.default().with_overrides({"skills": {"max_loaded": 2}})
