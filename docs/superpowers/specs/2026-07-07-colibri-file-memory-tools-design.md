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
- search the memory index and topic files by keyword,
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
  topics/
    preferences.md
    devices.md
    routines.md
```

`MEMORY.md` is the index. Topic files live under `topics/`.

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

## 4. Topic Names

Topic names are simple identifiers:

- lowercase or uppercase ASCII letters,
- digits,
- `_` and `-`,
- no slashes,
- no spaces,
- no `.` segments.

The topic `devices` maps to:

```text
<memory.root>/topics/devices.md
```

Invalid topic names return `invalid_arguments`.

## 5. Tools

Add `src/colibri/tools/builtin/memory.py`.

### `memory.list`

Read-only. Lists topics discovered from `topics/*.md`.

Result format:

```text
devices
preferences
```

If the memory directory does not exist, return an empty successful result.

### `memory.read`

Read-only. Reads a topic file by name.

Arguments:

```json
{"topic": "devices"}
```

Missing topic files return `not_found`.

### `memory.search`

Read-only. Searches `MEMORY.md` and topic files with simple case-insensitive substring matching.

Arguments:

```json
{"query": "wifi"}
```

Return up to `memory.max_search_results` matches. Each line uses:

```text
index: line text
devices: line text
```

Empty or missing query returns `invalid_arguments`.

### `memory.write`

Not read-only. Appends text to a topic file.

Arguments:

```json
{"topic": "devices", "text": "Router admin page is http://192.168.1.1"}
```

Behavior:

- Create `memory.root/topics` when needed.
- Create the topic file when needed.
- Append one Markdown bullet:

```markdown
- Router admin page is http://192.168.1.1
```

- Strip surrounding whitespace from text.
- Reject empty text with `invalid_arguments`.
- Cap output with `tools.max_result_chars`.

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
- `memory.list` returns sorted topic names,
- `memory.read` reads an existing topic and rejects invalid names,
- `memory.search` finds index and topic lines and respects result limits,
- `memory.write` appends a bullet and creates missing directories,
- `memory.write` is marked `read_only=False`,
- `AgentSession` requires confirmation for `memory.write` under the default permission policy,
- all tests run with `uv run python -m pytest`.

## 9. Future Work

After this milestone:

- inject relevant memory into model context,
- update `MEMORY.md` automatically when new topics are created,
- add structured memory proposals,
- add skill loading,
- add MCP bridge.
