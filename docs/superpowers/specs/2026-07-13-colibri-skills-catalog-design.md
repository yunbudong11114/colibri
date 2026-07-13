# Colibri Skills Catalog Redesign

Date: 2026-07-13
Status: Approved (conversation)
Replaces: keyword match + full `SKILL.md` injection in `2026-07-07-colibri-local-skills-design.md` §4–5
Scope: Python + Rust parity

## 1. Goal

Change skill progressive disclosure from:

> Colibri keyword-matches skills and injects full `SKILL.md` into the prompt

to:

> Colibri injects a bounded skill **catalog**; the model calls `skill.read` when it needs full instructions

This matches PicoClaw’s `BuildSkillsSummary` + on-demand load and ZeroClaw’s Compact mode + `read_skill`, while staying smaller than either project’s marketplace / multi-root skill systems.

## 2. Non-Goals

- Skill install, marketplace, registry, remote fetch
- Multiple skill roots (workspace / global / builtin dirs beyond the one builtin guidance skill)
- Compact/Full dual injection modes (Colibri is Compact-only)
- Recursive loading of skill `references/` via a dedicated tool (model may use `files.read` later; out of scope)
- Changing `skill.run` permission policy (still confirm-by-default)

## 3. Locked path layout

Do **not** rename to singular `~/.colibri/skill`. Existing users, README, tests, and defaults already use plural:

```text
~/.colibri/skills/<skill-name>/SKILL.md      # required
~/.colibri/skills/<skill-name>/skill.toml    # optional
~/.colibri/skills/<skill-name>/scripts/      # optional
```

Builtin `create-colibri-skill` is **not** scanned from this directory. It remains in-memory guidance only.

## 4. Config

### 4.1 Public fields

```toml
[skills]
# Single user skill directory. Default: ~/.colibri/skills
dir = "~/.colibri/skills"
# Max entries in the prompt catalog (local + builtin).
max_catalog = 32
# Max total characters for the catalog system message.
max_catalog_chars = 4000
# Max characters returned by skill.read for one skill body.
max_instruction_chars = 8000
```

### 4.2 Removed / rejected

| Old | Behavior |
|-----|----------|
| `skills.dirs` (list) | `ConfigError`: use `skills.dir` (single directory) |
| `skills.max_loaded` | `ConfigError`: use `skills.max_catalog` |

Runtime exposes **one** directory only. No multi-directory merge.

### 4.3 Defaults rationale

- `max_catalog=32`: enough for a personal device; still bounded
- `max_catalog_chars=4000`: catalog itself must not bloat context
- `max_instruction_chars=8000`: same order as today, applied to `skill.read` output

## 5. Prompt injection

### 5.1 Catalog only

Each model turn that loads skills injects an ephemeral system message (not persisted in session history), same placement as today (after memory, before conversation):

```text
Available skills (use skill.read with name when needed):

- create-colibri-skill: Guide creating ... [builtin]
- release-notes: Write release notes from git history [/home/user/.colibri/skills/release-notes]
```

Rules:

1. Include builtin `create-colibri-skill` first when present in the index.
2. Then local skills from `skills.dir`, sorted by name.
3. Cap at `max_catalog` entries.
4. If rendered catalog exceeds `max_catalog_chars`, truncate with `...[truncated]` (prefer dropping trailing entries before mid-line chops when practical; bound_text is acceptable).
5. **Do not** inject any `SKILL.md` body into the system prompt.
6. **Do not** keyword-score or auto-select skills per user turn.

### 5.2 Empty catalog

If there are no local skills and builtin is somehow disabled (should not happen), inject nothing (no empty “Available skills” header).

Builtin is always in the index → catalog is almost always non-empty. If only builtin remains after caps, still inject it.

### 5.3 Transcript / console

- Transcript event: `skill_catalog` with `{skills: [names...], truncated: bool}`
- Replace prior `skill_recall` emission for this path
- Console may show `[colibri] skills catalog=N` (keep short)

## 6. Tools

### 6.1 `skill.read` (new)

```json
{
  "name": "skill.read",
  "description": "Read the full SKILL.md instructions for a skill listed in the catalog. Prefer this over guessing skill contents.",
  "input_schema": {
    "type": "object",
    "properties": {
      "name": {"type": "string", "description": "Exact skill name from the catalog."}
    },
    "required": ["name"]
  },
  "read_only": true
}
```

Behavior:

1. Resolve by exact name (case-sensitive, matching index keys) against the skill index (builtin + `skills.dir` only).
2. Return body text:
   - Builtin: in-memory content
   - Local: read `SKILL.md` under that skill’s root
3. Prefix a short header:
   ```text
   [skill-name]
   Base directory: <root>

   <body>
   ```
4. Truncate body+header with `max_instruction_chars` (same bound helper as today).
5. Unknown name → error listing available names (bounded).
6. **No arbitrary paths** — only catalog names. Path traversal via `name` rejected by existing name validation / lookup.
7. Permission: `read_only=true` → default policy auto-allows (like `memory.read` / `files.read`).

Registered when `"skills"` is in `tools.enabled`, same gate as `skill.run`.

### 6.2 `skill.run` (unchanged semantics)

- Schema: `skill`, `command`, optional `args` (Rust must accept `args` for parity if Python does)
- Executes declared `skill.toml` commands only
- `read_only=false` → confirm under default policy
- Builtin has no commands → cannot run

### 6.3 System / tool descriptions

Tool description for `skill.read` plus the catalog header line are the only “when to read” guidance. No keyword matcher.

## 7. Builtin `create-colibri-skill`

- Remains in-memory; not scanned from disk
- Always appears in the catalog (subject to caps; listed first so it survives truncation when possible)
- User directory named `create-colibri-skill` is ignored (name reserved)
- Model discovers it via catalog + `skill.read` when the user asks to create a skill
- Remove keyword gate / special scoring (`_is_create_skill_request`, boost score)

## 8. Index / scan

Keep:

- Scan `skills.dir` for `*/SKILL.md`
- Metadata only at scan: name, description, root, commands
- mtime fingerprint cache
- Optional `skill.toml` commands for `skill.run`

Remove:

- `select(user_text, …)` scoring path from the hot session path
- `context_for(user_text, …)` full-body injection

Add:

- `catalog(config) -> SkillContext` building catalog text only
- `read(name, max_chars) -> str` used by `skill.read`

`SkillIndex.scan` takes a single `Path` (or `list` of one for minimal churn — prefer single `Path`).

## 9. Python / Rust parity

| Surface | Requirement |
|---------|-------------|
| Config fields + rejection of `dirs` / `max_loaded` | Identical errors |
| Catalog format | Identical string shape |
| `skill.read` schema + behavior | Identical |
| `skill.run` | Align Rust `args` + timeout/bound if Python has them (parity fix in same change if cheap) |
| Tests | Update Python unit + Rust runtime + parity map |
| README / README.zh-CN | Catalog + `skill.read`; path remains `skills/` |

## 10. Migration

- Existing skills under `~/.colibri/skills/<name>/` need **no** file moves
- Config with `[skills] dirs = [...]` fails load with clear error → change to `dir = "..."`
- Config with `max_loaded` fails → rename to `max_catalog`
- Behavior change: models that relied on auto-injected skill bodies must call `skill.read`

## 11. Acceptance

- Default prompt never contains a local skill’s full `SKILL.md`
- Catalog includes name + description + path (or `[builtin]`)
- `skill.read` returns bounded full instructions by name
- `skill.run` still works with permissions unchanged
- Custom multi-dir config rejected
- Python and Rust tests pass with matching behavior
- README documents catalog + `skill.read`

## 12. Rollout

Single coordinated change across config, skills, tools, session, tests, README. No phased dual-mode.
