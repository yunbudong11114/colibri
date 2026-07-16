# Colibri Skill YAML Frontmatter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `skill.toml` with required YAML frontmatter in `SKILL.md`, expose configured commands to the model, and keep Python/Rust behavior aligned.

**Architecture:** Both runtimes parse complete YAML frontmatter into the existing metadata and command structures while retaining metadata-only indexes and bounded on-demand reads. Catalog and tool descriptions carry one global rule that matching configured commands use `skill.run`, and the builtin creation skill teaches the single-file format.

**Tech Stack:** Python 3.11, PyYAML, pytest, Rust 2021, serde, serde_yaml, Cargo tests

## Global Constraints

- Modify design documentation before code; the approved design is `docs/superpowers/specs/2026-07-16-colibri-skill-yaml-frontmatter-design.md`.
- Do not support legacy `skill.toml` or frontmatter-free skills.
- `name` and `description` are required; `name` must equal the skill directory name.
- An invalid command invalidates the entire skill.
- Unknown YAML fields are ignored.
- Do not add `skill.validate`.
- Keep `skill.run` permission-controlled at the tool level; per-command `read_only` does not alter permissions in this change.
- Migrate the installed skill named `memory-sync`, not `nas-sync`.

---

### Task 1: Python YAML Metadata Parser

**Files:**
- Modify: `pyproject.toml`
- Modify: `src/colibri/skills.py`
- Modify: `tests/unit/test_skills.py`

**Interfaces:**
- Produces: `_parse_skill_document(content: str, expected_name: str) -> tuple[str, list[SkillCommand]] | None`
- Produces: frontmatter-backed `SkillMetadata` entries with `content=None` for local skills

- [ ] **Step 1: Add failing Python parser tests**

Add fixtures using:

```python
def skill_document(name: str = "release", commands: str = "") -> str:
    return f"""---
name: {name}
description: >
  Release helper
  with details.
{commands}---

# Release Notes
"""
```

Cover:

```python
def test_skill_index_parses_yaml_frontmatter_and_commands(tmp_path):
    ...
    assert release.description == "Release helper with details.\n"
    assert release.commands[0] == SkillCommand(
        name="render",
        description="Render notes",
        command="python",
        args=["scripts/render.py"],
        read_only=True,
    )

@pytest.mark.parametrize(
    "document",
    [
        "# Missing frontmatter\n",
        "---\ndescription: missing name\n---\n",
        "---\nname: other\ndescription: mismatch\n---\n",
        "---\nname: release\ndescription: ''\n---\n",
        "---\nname: release\ndescription: ok\ncommands: invalid\n---\n",
        "---\nname: release\ndescription: ok\ncommands:\n  - name: render\n---\n",
    ],
)
def test_skill_index_skips_invalid_yaml_skill(tmp_path, document):
    ...
    assert index.get("release") is None
```

Also change existing fixtures to valid frontmatter and add a test proving a neighboring `skill.toml` is ignored.

- [ ] **Step 2: Run Python parser tests and verify RED**

Run:

```bash
uv run python -m pytest tests/unit/test_skills.py -q
```

Expected: failures because frontmatter is not parsed and invalid skills are still accepted.

- [ ] **Step 3: Add PyYAML and implement strict parsing**

Add:

```toml
"PyYAML>=6.0.2",
```

Implement frontmatter extraction that requires the first line and closing delimiter to be `---`, parses only the delimited YAML with `yaml.safe_load`, validates exact scalar/list types, rejects duplicate command names, and returns `None` on all parse or validation failures.

Remove:

```python
import tomllib
_read_skill_toml
_parse_commands(metadata)
_derive_description
```

Change the fingerprint to watch only `SKILL.md`.

- [ ] **Step 4: Run Python parser tests and verify GREEN**

Run:

```bash
uv run python -m pytest tests/unit/test_skills.py -q
```

Expected: all tests pass.

- [ ] **Step 5: Commit Python parser**

```bash
git add pyproject.toml uv.lock src/colibri/skills.py tests/unit/test_skills.py
git commit -m "feat: parse skill YAML frontmatter in Python"
```

### Task 2: Python Catalog, Read Output, and Creation Guidance

**Files:**
- Modify: `src/colibri/skills.py`
- Modify: `src/colibri/tools/builtin/skills.py`
- Modify: `tests/unit/test_skills.py`
- Modify: `tests/unit/test_tools.py`
- Modify: `tests/unit/test_session.py`

**Interfaces:**
- Consumes: parsed `SkillMetadata.commands`
- Produces: command-aware catalog and `skill.read` output

- [ ] **Step 1: Add failing prompt and builtin tests**

Assert the catalog contains:

```text
Available skills:

Use skill.read to load full instructions. When a skill has a configured command matching the requested action, use skill.run instead of invoking that command's underlying executable through shell.run.
```

Assert a command-bearing entry contains:

```text
- release: Release helper Commands: render [/tmp/.../skills/release]
```

Assert `skill.read` contains:

```text
Configured commands:
- render: Render notes
```

Assert the builtin content begins with frontmatter, contains
`name: create-colibri-skill`, teaches `commands:` in `SKILL.md`, and does not
mention `skill.toml`.

Assert the `skill.run` tool description includes both `Prefer` and `shell.run`.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```bash
uv run python -m pytest tests/unit/test_skills.py tests/unit/test_tools.py tests/unit/test_session.py -q
```

Expected: catalog, read output, builtin guidance, and tool-description assertions fail.

- [ ] **Step 3: Implement command-aware model guidance**

Replace the builtin string with a complete frontmatter-based document. Parse
the builtin through the same parser using expected name
`create-colibri-skill`.

Format catalog entries with:

```python
command_text = (
    f" Commands: {', '.join(command.name for command in skill.commands)}"
    if skill.commands
    else ""
)
```

Format `skill.read` command summaries before the full document. Update
`SkillRunTool.spec.description` to prefer matching configured commands and
avoid direct `shell.run`.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run:

```bash
uv run python -m pytest tests/unit/test_skills.py tests/unit/test_tools.py tests/unit/test_session.py -q
```

Expected: all focused Python tests pass.

- [ ] **Step 5: Commit Python guidance**

```bash
git add src/colibri/skills.py src/colibri/tools/builtin/skills.py tests/unit
git commit -m "feat: expose skill commands to the model"
```

### Task 3: Rust YAML Metadata and Prompt Parity

**Files:**
- Modify: `colibri-rust/Cargo.toml`
- Modify: `colibri-rust/Cargo.lock`
- Modify: `colibri-rust/src/skills.rs`
- Modify: `colibri-rust/src/tools.rs`
- Modify: `colibri-rust/tests/runtime.rs`
- Modify: `colibri-rust/tests/parity.rs`

**Interfaces:**
- Produces: Rust frontmatter parsing and rendered strings identical to Python
- Consumes: the schema fixed in Tasks 1 and 2

- [ ] **Step 1: Record baseline and add failing Rust tests**

Record:

```bash
ls -l colibri-rust/target/release/colibri
```

Replace TOML fixtures with YAML frontmatter. Add tests for multiline
description, commands, missing frontmatter, name mismatch, invalid command,
ignored `skill.toml`, command-aware catalog, builtin frontmatter, and
`skill.read` command summaries.

Update the parity map entry
`skill_toml_parses_multiline_description_like_python` to the new YAML test
name.

- [ ] **Step 2: Run Rust tests and verify RED**

Run:

```bash
cargo test --manifest-path colibri-rust/Cargo.toml skill -- --nocapture
```

Expected: YAML metadata and prompt assertions fail.

- [ ] **Step 3: Add serde YAML dependencies and implement parser**

Add compatible dependencies:

```toml
serde = { version = "1", features = ["derive"] }
serde_yaml = "0.9"
```

Define deserialization structures with `#[derive(Deserialize)]`, validate
required non-empty strings and duplicate command names after deserialization,
and ignore unknown fields through Serde defaults.

Remove `read_skill_toml`, TOML command parsing, and title-derived descriptions.
Watch only `SKILL.md` in `dir_fingerprint`.

Apply the catalog, builtin, `skill.read`, and tool-description formats from
Task 2 exactly.

- [ ] **Step 4: Run Rust focused tests and verify GREEN**

Run:

```bash
cargo test --manifest-path colibri-rust/Cargo.toml skill -- --nocapture
```

Expected: all skill-focused tests pass.

- [ ] **Step 5: Build release and record final size**

Run:

```bash
cargo build --release --manifest-path colibri-rust/Cargo.toml
ls -l colibri-rust/target/release/colibri
```

Expected: release build succeeds and before/after byte sizes are recorded in
the implementation handoff.

- [ ] **Step 6: Commit Rust parity**

```bash
git add colibri-rust/Cargo.toml colibri-rust/Cargo.lock colibri-rust/src/skills.rs colibri-rust/src/tools.rs colibri-rust/tests
git commit -m "feat: parse skill YAML frontmatter in Rust"
```

### Task 4: Documentation and Installed `memory-sync` Migration

**Files:**
- Modify: `README.md`
- Modify: `README.zh-CN.md`
- Modify: `~/.colibri/skills/memory-sync/SKILL.md`
- Delete: `~/.colibri/skills/memory-sync/skill.toml`

**Interfaces:**
- Produces: one-file user-facing Skill format and a migrated installed skill

- [ ] **Step 1: Update README examples**

Document:

```markdown
---
name: example
description: Explain when the skill is useful.
commands:
  - name: check
    description: Run the local check.
    command: python
    args: [scripts/check.py]
    read_only: true
---
```

Remove every claim that `skill.toml` is supported. Explain that command names
appear in the catalog and matching actions should use `skill.run`.

- [ ] **Step 2: Migrate `memory-sync`**

Move the existing description and the `upload`/`download` definitions into the
top of `SKILL.md`:

```yaml
---
name: memory-sync
description: 通过 rclone 在 Colibri 核心目录与 NAS WebDAV 存储之间执行非删除式备份和恢复，实现 Colibri 的记忆上传和同步。
commands:
  - name: upload
    description: 上传 ~/.colibri 到 NAS (mynas:/Colibri)，不删除 NAS 独有文件
    command: sh
    args: [scripts/upload.sh]
    read_only: false
  - name: download
    description: 从 NAS 下载恢复到 ~/.colibri，不删除本地独有文件
    command: sh
    args: [scripts/download.sh]
    read_only: false
---
```

Delete `~/.colibri/skills/memory-sync/skill.toml`. Do not change either sync
script in this task.

- [ ] **Step 3: Run repository-wide format searches**

Run:

```bash
rg -n "skill\\.toml|read_skill_toml|skill_toml" README.md README.zh-CN.md src tests colibri-rust --glob '!target/**'
```

Expected: no runtime, current documentation, or active test references remain.
Historical design documents may still describe the superseded format.

- [ ] **Step 4: Commit repository documentation**

```bash
git add README.md README.zh-CN.md
git commit -m "docs: document YAML skill frontmatter"
```

The installed `memory-sync` files live outside the repository and are not part
of the Git commit.

### Task 5: Full Verification

**Files:**
- Verify only

**Interfaces:**
- Consumes: all prior tasks
- Produces: completion evidence

- [ ] **Step 1: Run the full Python suite**

```bash
uv run python -m pytest -q
```

Expected: all Python tests pass.

- [ ] **Step 2: Run the full Rust suite**

```bash
cargo test --manifest-path colibri-rust/Cargo.toml
```

Expected: all Rust unit, runtime, and parity tests pass.

- [ ] **Step 3: Verify installed Skill discovery**

Run a read-only diagnostic or focused Python command using the active
configuration and assert:

```text
memory-sync
Commands: upload, download
```

appears in the generated catalog, and `skill.read("memory-sync")` reports both
configured commands.

- [ ] **Step 4: Review final diff**

```bash
git status --short
git diff --check
git diff --stat
```

Expected: only intended repository changes plus pre-existing untracked user
files; no `.DS_Store` files are staged.
