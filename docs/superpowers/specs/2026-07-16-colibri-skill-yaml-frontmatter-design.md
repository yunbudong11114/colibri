# Colibri Skill YAML Frontmatter Design

Date: 2026-07-16
Status: Approved in conversation
Scope: Python and Rust runtimes, builtin creation guidance, bundled local skills
Replaces: `skill.toml` metadata and command declarations

## 1. Goal

Make each Colibri skill a single `SKILL.md` file whose YAML frontmatter is the
authoritative source for discovery metadata and runnable commands. Keep the
existing bounded catalog plus on-demand `skill.read` flow, while ensuring the
model prefers a declared `skill.run` command over invoking the same executable
through `shell.run`.

## 2. Skill Layout

The supported layout is:

```text
~/.colibri/skills/<skill-name>/
  SKILL.md
  scripts/...       # optional
```

`skill.toml` is removed from the format and runtime. There is no compatibility
fallback for old `skill.toml` files or frontmatter-free `SKILL.md` files.

## 3. Required YAML Frontmatter

Every `SKILL.md` must begin with a YAML document bounded by `---` lines:

```markdown
---
name: memory-sync
description: >
  Back up and restore Colibri data through NAS WebDAV.
commands:
  - name: upload
    description: Upload local Colibri data to NAS.
    command: bash
    args:
      - scripts/upload.sh
    read_only: false
---

# Memory Sync
```

Fields:

- `name`: required non-empty string and must equal the skill directory name.
- `description`: required non-empty string used in the injected catalog.
- `commands`: optional list.
- `commands[].name`: required non-empty string, unique within the skill.
- `commands[].description`: optional string.
- `commands[].command`: required non-empty executable string.
- `commands[].args`: optional list of strings, default `[]`.
- `commands[].read_only`: optional boolean, default `false`.

Python uses a complete YAML library and Rust uses a complete YAML library.
Parsing behavior and accepted field types must match. Unknown frontmatter
fields are ignored so the format can coexist with external skill metadata.

If frontmatter is missing, invalid, has a mismatched name, or contains an
invalid command, the entire local skill is excluded from the index and catalog.
Scanning continues without failing the user turn.

## 4. Index and Cache

The skill index remains metadata-only. Scanning reads and parses the YAML
frontmatter but does not retain the full Markdown body for local skills.

The scan fingerprint watches `SKILL.md` only. Changes to frontmatter or body
invalidate the cached index through the file modification time.

The builtin `create-colibri-skill` remains in memory, but its bundled content is
a complete `SKILL.md` document with the same required YAML frontmatter. It is
parsed through the same metadata parser instead of maintaining separately
duplicated name and description constants.

## 5. Catalog and Command Discovery

The catalog remains an ephemeral system message, bounded by
`skills.max_catalog` and `skills.max_catalog_chars`.

It uses this global guidance:

```text
Available skills:

Use skill.read to load full instructions. When a skill has a configured command
matching the requested action, use skill.run instead of invoking that command's
underlying executable through shell.run.
```

Each entry includes name, description, command names when present, and path:

```text
- memory-sync: Back up and restore Colibri data through NAS WebDAV. Commands: upload, download [/home/user/.colibri/skills/memory-sync]
```

Skills without commands omit the `Commands:` portion.

## 6. `skill.read`

`skill.read` continues to load the complete bounded `SKILL.md`. Its returned
header additionally lists configured commands with descriptions:

```text
[memory-sync]
Base directory: /home/user/.colibri/skills/memory-sync
Configured commands:
- upload: Upload local Colibri data to NAS.
- download: Restore Colibri data from NAS.

<complete SKILL.md>
```

The global command-preference rule is not repeated in each skill file or each
`skill.read` result.

## 7. `skill.run`

`skill.run` resolves commands exclusively from parsed YAML frontmatter. It
retains the current execution model:

- execute `command + configured args + model-supplied args` as an argv array;
- use the skill root as the working directory;
- apply the existing timeout and bounded-output behavior;
- reject unknown skills and command names;
- remain a permission-controlled, non-read-only tool at the registry level.

The tool description explicitly states that it should be preferred whenever a
matching configured command exists and that the underlying executable should
not be invoked through `shell.run`.

Per-command `read_only` remains parsed metadata for schema parity but does not
change the current permission policy in this change.

## 8. Builtin Creation Skill

`create-colibri-skill` teaches the new single-file format:

- create `~/.colibri/skills/<name>/SKILL.md`;
- always include valid YAML frontmatter;
- require matching `name` and non-empty `description`;
- put optional commands in frontmatter;
- place reusable implementations under `scripts/`;
- do not create `skill.toml`;
- explain that matching declared commands are executed with `skill.run`.

No new `skill.validate` tool is added. The creation guidance must make the
required structure explicit.

## 9. Migration

All repository-owned and currently installed Colibri skills in scope are
migrated to frontmatter:

- `memory-sync` moves its description and commands into `SKILL.md`;
- its `skill.toml` is deleted;
- test fixtures use frontmatter-only skills.

Existing third-party or user skills are not automatically migrated. A
frontmatter-free skill simply disappears from the catalog until edited.

## 10. Dependencies

- Python adds a full YAML parser dependency.
- Rust adds a full YAML parser dependency.
- Release builds retain the existing LTO, single codegen unit, stripping, and
  abort-on-panic settings.

The implementation records the release binary size before and after the Rust
dependency change for visibility, but no fixed size threshold is imposed.

## 11. Documentation and Parity

Update English and Chinese README documentation, configuration examples where
needed, Python tests, Rust runtime tests, and the Python/Rust parity map.

Python and Rust must match on:

- valid and invalid frontmatter behavior;
- field defaults and type validation;
- name-directory matching;
- command parsing;
- catalog text;
- `skill.read` command summaries;
- `skill.run` resolution and execution.

## 12. Acceptance Criteria

- `skill.toml` is no longer read or documented.
- Only valid frontmatter-based skills enter the catalog.
- Catalog entries expose configured command names.
- The global prompt and `skill.run` description direct the model to prefer
  matching commands over `shell.run`.
- `skill.read` exposes configured command descriptions and full instructions.
- The builtin creation skill uses and teaches the new format.
- `memory-sync` uses one `SKILL.md` and continues to support upload/download.
- Python and Rust focused tests and full test suites pass.
