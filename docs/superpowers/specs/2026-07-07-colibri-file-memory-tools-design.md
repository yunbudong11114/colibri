# Colibri File Memory Tools Design

Date: 2026-07-07
Status: Approved by user direction
Milestone: 4
Scope: File-based memory tools

## 1. Goal

Milestone 4 gives Colibri a small persistent memory surface backed by plain Markdown files.

After this milestone, Colibri should expose tools that let the model:

- list memory topics,
- read a memory topic,
- search the memory index by keyword,
- append a memory note to a topic file.

This milestone intentionally does not implement automatic memory retrieval or prompt injection. The model may use the tools explicitly through the existing tool loop.

## 2. Headless Requirement

Memory tools must work on pure Linux servers over SSH.

Rules:

- No GUI dependency.
- No browser dependency.
- No audio, display, notification, or TUI dependency.
- Use only Python standard library APIs.
- Keep memory files on disk, not in process memory.

## 3. Storage Layout

Use `~/.colibri/memory` by default:

```text
memory/
  MEMORY.md
  USER.md
  INDEX.md
  topics/
    system-info.md
    colibri-design.md
```

`MEMORY.md` and `USER.md` are short always-on files. `INDEX.md` is the topic manifest. Topic files live under `topics/`.

The current `SkillsConfig` and `FilesConfig` stay unchanged. Add:

```python
@dataclass(frozen=True)
class MemoryConfig:
    root: Path = expand_user_path("~/.colibri/memory")
    max_search_results: int = 5
```

`AgentConfig` should include `memory: MemoryConfig`, and TOML overrides should accept:

```toml
[memory]
root = "~/.colibri/memory"
max_search_results = 5
```

## 4. Memory File Names

Topic names are simple identifiers:

- lowercase or uppercase ASCII letters,
- digits,
- `_` and `-`,
- no slashes,
- no spaces,
- no `.` segments.

The topic `system-info` maps to:

```text
<memory.root>/topics/system-info.md
```

Tools accept either `file` (`MEMORY.md`, `USER.md`, `INDEX.md`, or `topics/<topic>.md`) or a compatibility `topic` shorthand. Invalid names return `invalid_arguments`.

## 5. Tools

Add `src/colibri/tools/builtin/memory.py`.

### `memory.list`

Read-only. Lists existing built-in memory files and topics discovered from `topics/*.md`.

Result format:

```text
MEMORY.md
USER.md
INDEX.md
topics/system-info.md
```

If the memory directory does not exist, return an empty successful result.

### `memory.read`

Read-only. Reads `MEMORY.md`, `USER.md`, `INDEX.md`, or a topic file.

Arguments:

```json
{"file": "INDEX.md"}
```

The compatibility form `{"topic": "system-info"}` reads `topics/system-info.md`. Missing files return `not_found`.

### `memory.search`

Read-only. Searches `INDEX.md` lines with simple case-insensitive substring matching. It does not search always-on files or topic contents directly.

Arguments:

```json
{"query": "wifi"}
```

Return up to `memory.max_search_results` matches. Each line uses:

```text
INDEX.md:3: line text
topics/system-info.md:8: line text
```

Empty or missing query returns `invalid_arguments`.

### `memory.write`

Not read-only. Appends to or replaces a memory file.

Arguments:

```json
{"file": "topics/system-info.md", "mode": "append", "content": "- Router admin page is http://192.168.1.1"}
```

Behavior:

- Create parent directories when needed.
- Create the target file when needed.
- `append` adds content at the end of the file.
- `replace` overwrites the whole file.

- Strip surrounding whitespace from content.
- Reject empty content with `invalid_arguments`.
- Cap output with `tools.max_result_chars`.
- The tool description includes the memory file format, write-routing guidance, and concise-memory limits:
  - `USER.md` is for user profile/preferences and should stay under 600 characters.
  - `MEMORY.md` is for short stable facts and should stay under 1800 characters.
  - `INDEX.md` is the searchable manifest used by `memory.search`.
  - `topics/<name>.md` stores detailed topic notes.
  - memory files use frontmatter with `type`, `description`, and `updated`.
- If a topic file is written, the tool result reminds the model to update `INDEX.md`.
- If `USER.md` or `MEMORY.md` exceeds its limit after a write, the tool result reminds the model to summarize/consolidate it and call `memory.write` again with `mode="replace"`.

Because `memory.write` is not read-only, the existing permission policy confirms it under `allow_read_confirm_write`.

## 6. Registry Integration

`ToolRegistry.from_config()` should register memory tools only when `"memory"` is in `tools.enabled`.

The default `tools.enabled` already includes `"memory"`, so memory tools become available by default.

## 7. Transcript Behavior

Memory tool calls already go through `AgentSession`, so Milestone 3 transcript logging should record:

- `tool_call`,
- `permission_decision`,
- `tool_result`.

Do not add a separate memory log.

## 8. Testing

Required tests:

- config loads `[memory]` overrides,
- registry exposes `memory.list`, `memory.read`, `memory.search`, and `memory.write`,
- `memory.list` returns built-in files and sorted topic files,
- `memory.read` reads built-in files, topic shorthand, and rejects invalid names,
- `memory.search` finds `INDEX.md` lines and respects result limits,
- `memory.write` appends or replaces files and creates missing directories,
- `memory.write` exposes format/routing guidance in its description,
- `memory.write` warns about oversized `USER.md` and `MEMORY.md`,
- `memory.write` reminds topic writers to update `INDEX.md`,
- `memory.write` is marked `read_only=False`,
- `AgentSession` requires confirmation for `memory.write` under the default permission policy,
- all tests run with `uv run python -m pytest`.

## 9. Future Work

After this milestone:

- model-assisted memory lookup,
- update `INDEX.md` automatically when new topics are created,
- add structured memory proposals,
- add skill loading,
- add MCP bridge.
