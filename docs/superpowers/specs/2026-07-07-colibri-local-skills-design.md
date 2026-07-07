# Colibri Local Skills Design

Date: 2026-07-07
Status: Implemented
Milestone: 7
Scope: Local filesystem skills with progressive disclosure

## 1. Goal

Milestone 7 adds local skills to Colibri without adding installation, marketplace, registry, remote fetch, plugin distribution, or package-management behavior.

After this milestone, Colibri should:

- scan configured local skill directories,
- build a small in-memory index from skill metadata only,
- select relevant skills for each user turn,
- read and inject only selected skill instructions,
- expose a minimal `skill.run` tool for configured local skill commands,
- run skill commands through the existing permission system,
- keep the implementation standard-library only and headless-server safe.

## 2. Non-Goals

Milestone 7 must not implement:

- skill installation,
- skill marketplace discovery,
- remote skill download,
- package registries,
- plugin management,
- bundled skill catalogs,
- MCP skills,
- model-based skill search,
- recursive loading of arbitrary referenced files.

Users add skills by placing files under configured local directories.

## 3. Skill Layout

The supported v1 layout is:

```text
skills/<name>/
  SKILL.md
  skill.toml        # optional
  scripts/...       # optional
```

`SKILL.md` is required. `skill.toml` is optional and uses Python `tomllib`.

Minimal `skill.toml`:

```toml
description = "Summarize a release note."

[[commands]]
name = "render"
description = "Render the release note template"
command = "python"
args = ["scripts/render.py"]
read_only = false
```

If `skill.toml` is absent, Colibri should derive:

- `name` from the directory name,
- `description` from the first non-empty heading or paragraph in `SKILL.md`,
- no runnable commands.

## 4. Progressive Disclosure

The design intentionally follows Claude Code's skill disclosure shape, but is stricter about memory use for CardputerZero.

Rules:

1. Scan time stores only metadata:
   - name,
   - description,
   - path to `SKILL.md`,
   - command metadata from `skill.toml`.
2. Scan time must not retain full `SKILL.md` bodies in the long-lived skill index.
3. On each user turn, score skill metadata with simple keyword overlap.
4. Load full `SKILL.md` only for selected skills.
5. Inject at most `skills.max_loaded` selected skills.
6. Bound injected instruction text by `skills.max_instruction_chars`.
7. Do not recursively read files referenced by `SKILL.md`. Future milestones can add explicit file-reference loading.
8. If a selected skill exceeds the budget, truncate the injected instruction and mark it as truncated.

This yields:

```text
local dirs -> metadata index -> per-turn selection -> read selected SKILL.md -> bounded injection
```

## 5. Model Context

`AgentSession` should build model input from:

1. compacted summary context,
2. recalled memory context,
3. selected skill instruction context,
4. current recent messages.

Skill context is a temporary `system` message and is not appended to `AgentSession.messages`.

Format:

```text
Relevant skills:

[release-notes]
Base directory: /home/user/.colibri/skills/release-notes

<bounded SKILL.md content>
```

When skills are injected, write transcript event:

```text
skill_recall
```

Payload:

```json
{
  "skills": ["release-notes"],
  "truncated": false
}
```

## 6. `skill.run` Tool

The minimal `skill.run` tool executes a configured command from a local skill.

Input schema:

```json
{
  "type": "object",
  "properties": {
    "skill": { "type": "string" },
    "command": { "type": "string" },
    "args": { "type": "array", "items": { "type": "string" } }
  },
  "required": ["skill", "command"]
}
```

Rules:

- `skill.run` is registered only when `"skills"` is enabled in `tools.enabled`.
- A command must be declared in the local skill's `skill.toml`.
- The process working directory is the skill directory.
- Extra `args` are appended after the configured command args.
- Results are bounded by `tools.max_result_chars`.
- Missing skill, missing command, invalid command config, timeout, and subprocess errors return `ToolResult(ok=False, ...)`.
- `read_only` defaults to `false` for skill commands unless explicitly set to `true`.

Because `skill.run` can execute local scripts, permission checks use the existing tool permission system. The tool's own `ToolSpec.read_only` should be `false` so the default policy asks before execution.

## 7. Config

Extend `SkillsConfig`:

```python
dirs: list[Path] = ["~/.colibri/skills"]
max_loaded: int = 3
max_instruction_chars: int = 6000
```

`max_loaded` should mean per-turn injected skills, not total available skills. The scanner may discover more local skills, but model context should stay bounded.

## 8. Error Handling

- Missing skill directory: treat as empty.
- Skill directory without `SKILL.md`: skip it.
- Invalid `skill.toml`: skip runnable commands for that skill but keep `SKILL.md` metadata when possible.
- Unreadable `SKILL.md`: skip the skill and continue.
- Duplicate skill names: first directory in config order wins.
- Oversized instruction: truncate and set transcript `truncated=true`.

Do not fail a user turn because a local skill is malformed.

## 9. Testing

Required tests:

- scans local `skills/<name>/SKILL.md` directories,
- derives description without `skill.toml`,
- parses command metadata from `skill.toml`,
- keeps the long-lived index metadata-only,
- selects skills by keyword overlap,
- injects selected skill instructions into model input without persisting them,
- respects `skills.max_loaded` and `skills.max_instruction_chars`,
- logs `skill_recall`,
- registers `skill.run` when skills are enabled,
- `skill.run` executes a declared local command,
- `skill.run` rejects missing skills and missing commands,
- default permission policy asks before `skill.run`,
- all tests run with `uv run python -m pytest`.

## 10. Future Work

After Milestone 7:

- explicit referenced-file loading with budgets,
- richer skill matching,
- skill change detection,
- per-skill command environment controls,
- MCP skills in the MCP milestone.
