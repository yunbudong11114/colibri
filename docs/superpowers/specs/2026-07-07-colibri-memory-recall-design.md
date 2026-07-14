# Colibri Model-Driven File Memory Design

Date: 2026-07-07
Updated: 2026-07-14
Status: Approved by user
Milestone: 5
Scope: File-backed long-term memory, model-driven memory lookup, and bounded context injection

## 1. Goal

Milestone 5 gives Colibri a Claude Code inspired but CardputerZero-friendly memory system.

After this milestone, Colibri should:

- keep always-on short memory in `SOUL.md`, `USER.md`, and `MEMORY.md`,
- keep detailed searchable memories in `topics/*.md`,
- keep topic discovery metadata in `INDEX.md`,
- inject only bounded always-on memory automatically,
- let the model decide when to search, read, or write detailed memory through tools,
- constrain all memory files with simple Markdown formats and strict character budgets,
- avoid embeddings, vector databases, SQLite, background daemons, and third-party packages.

This replaces the earlier deterministic keyword-overlap auto-recall design. Colibri should no longer decide topic relevance itself.

## 2. Directory Layout

Default memory root:

```text
~/.colibri/memory/
  SOUL.md
  USER.md
  MEMORY.md
  INDEX.md
  topics/
    system-info.md
    colibri-design.md
```

File roles:

| File | Role | Loaded automatically |
| --- | --- | --- |
| `SOUL.md` | Colibri persona, principles, expression style, and self-constraints, max 1000 chars | yes |
| `USER.md` | User profile, preferences, collaboration style, max 1000 chars | yes |
| `MEMORY.md` | Stable long-term facts and project-level context, max 2000 chars | yes |
| `INDEX.md` | Short manifest of topic files | no, read through tools |
| `topics/*.md` | Detailed topic memories | no, read through tools |

`SOUL.md`, `USER.md`, and `MEMORY.md` must stay short. `INDEX.md` should stay a manifest, not a content dump.

When Colibri runs with memory enabled and the memory root does not exist, or the memory root exists but contains no files, Colibri should bootstrap a sample layout:

```text
~/.colibri/memory/
  MEMORY.md
  SOUL.md
  USER.md
  INDEX.md
  topics/
    sample.md
```

Bootstrap must never overwrite existing files. Sample files must follow the same frontmatter format as normal memory files and explain:

- what the file is for,
- when the user or model should update that file,
- that memory changes should rewrite or consolidate the corresponding file instead of repeatedly appending duplicate notes.
- that the first real write should replace the sample file content instead of preserving the example text.

The sample `INDEX.md` entry should be shaped for `memory.search`, which performs a simple case-insensitive substring match over whole `INDEX.md` lines. The text after `:` should therefore contain multiple searchable keywords, aliases, and short description words, for example:

```markdown
- [sample](topics/sample.md): sample 示例 topic 详细记忆 写法 维护 memory search index
```

This bootstrap is allowed to happen during memory context loading, before reading `SOUL.md`, `USER.md`, and `MEMORY.md`.

## 3. Headless Requirement

Memory must run on pure Linux servers over SSH.

Rules:

- Use only Python standard library APIs.
- Do not keep all topic files resident in memory.
- Do not require GUI, browser, audio, display, notification, or TUI frameworks.
- Keep startup memory loading deterministic and testable without network access.
- Let existing model/tool loops handle model-driven search and write decisions.

## 4. Configuration

`MemoryConfig` remains the owner of memory settings:

```python
root: Path = ~/.colibri/memory
enabled: bool = True
max_search_results: int = 5
max_recall_topics: int = 3
max_recall_chars: int = 6000
```

`max_recall_topics` is retained for config compatibility and future model-assisted selectors, but the built-in deterministic auto-topic recall no longer uses it.

`max_recall_chars` becomes the total character budget for automatically injected always-on memory.

TOML override:

```toml
[memory]
enabled = true
root = "~/.colibri/memory"
max_search_results = 5
max_recall_topics = 3
max_recall_chars = 6000
```

If `memory.enabled = false`, no automatic memory context is injected. Explicit memory tools remain available when `"memory"` is enabled under `[tools]`.

## 5. Memory File Format

All memory files may use lightweight YAML-style frontmatter followed by Markdown:

```markdown
---
type: user|feedback|project|reference|system
description: One sentence about what this file records.
updated: 2026-07-09
---

# Title

- Fact or preference.
- Why: why it matters.
- How to apply: when future Colibri should use it.
```

The runtime prompt must tell the model this format. The current Python implementation does not parse frontmatter for ranking; it is used as model-readable structure, future-compatible metadata, and a stable convention for writes.

Allowed `type` values:

- `user`: user profile, role, goals, preferences.
- `feedback`: guidance about how Colibri should behave.
- `project`: ongoing project context not derivable from code.
- `reference`: pointers to external systems or where to look.
- `system`: local machine, runtime, deployment, or device facts.

Memory should not store:

- facts easily derived from current source code,
- full transcripts,
- secrets or API keys,
- large logs or command output,
- temporary task state that only matters in the current conversation.

## 6. INDEX.md Format

`INDEX.md` is a short manifest. Each topic entry should be one line:

```markdown
# Memory Index

- [system-info](topics/system-info.md): Current machine, OS, hardware, and runtime environment.
- [colibri-design](topics/colibri-design.md): Colibri project design decisions and milestones.
```

Parsing is intentionally permissive. Tools may return raw `INDEX.md` content; the model decides which linked topic files are worth reading.

## 7. Automatic Context Injection

Add a focused component:

```python
MemoryContextResult:
    text: str
    files: list[str]
    truncated: bool

MemoryContext:
    load() -> MemoryContextResult
```

`AgentSession.submit()` calls `MemoryContext.load()` once per user turn before the model call.

When available, memory is injected as a temporary system-style message before regular conversation messages:

```text
Always-on memory:

[SOUL.md]
...

[USER.md]
...

[MEMORY.md]
...
```

The implementation injects the files in `SOUL.md`, `USER.md`, `MEMORY.md` order.

Bootstrap templates live as packaged Markdown files under `src/colibri/memory_templates/`. Python reads those resources at runtime, and Rust includes the same files at compile time. The template text must not be duplicated in Python and Rust source files.

This message must not be appended to `AgentSession.messages`; it is only part of the model input for that submit call.

If no always-on memory exists, do not inject a memory message.

`MemoryContext` only injects bounded file content. It does not inject memory-writing rules, file format rules, or per-file maintenance instructions. Those rules belong to the `memory.write` tool description and tool result text, so they are only shown when the model is considering or has just performed a memory write.

## 8. Model-Driven Lookup and Writes

Colibri no longer performs deterministic topic selection. Instead:

- `MemoryContext` owns always-on memory loading only,
- `AgentSession` uses the core `SYSTEM_PROMPT` directly and does not build feature-specific prompt variants,
- the `memory.write` tool description tells the model that detailed memory lives under `INDEX.md` and `topics/*.md`,
- `memory.search` searches inside `INDEX.md` only; the model reads matching topic files explicitly with `memory.read`,
- `memory.read` reads one of `SOUL.md`, `USER.md`, `MEMORY.md`, `INDEX.md`, or a topic file,
- `memory.write` writes one of the same file roles using model-supplied content.

Memory usage guidance must not be hard-coded directly into the core `SYSTEM_PROMPT` constant. Keep ownership split:

- `session.py`: core Colibri identity and low-memory behavior only.
- `memory.py`: always-on memory loading and global context budget handling.
- `tools/builtin/memory.py`: concrete memory tool implementation, memory file format guidance, write-routing guidance, topic index maintenance guidance, and short per-file length maintenance prompts.

The model should:

- read/search memory when the user references prior context, preferences, machine facts, project decisions, or asks Colibri to remember/recall something,
- first inspect `INDEX.md` or use `memory.search` for detailed memories,
- decide whether a write belongs in `USER.md`, `MEMORY.md`, or a dedicated `topics/*.md` file,
- update `INDEX.md` whenever creating or materially changing a topic file,
- keep `MEMORY.md` and `USER.md` concise,
- consolidate stale or duplicate entries instead of appending forever.

## 9. Tool Behavior

### `memory.list`

Read-only. Returns:

- built-in memory files that exist: `MEMORY.md`, `USER.md`, `INDEX.md`,
- topic names under `topics/*.md`.

### `memory.read`

Read-only. Accepts:

- `file`: one of `MEMORY.md`, `USER.md`, `INDEX.md`, or `topics/<name>.md`,
- or `topic`: compatibility shorthand for `topics/<topic>.md`.

Invalid paths, path traversal, and non-Markdown files are rejected.

### `memory.search`

Read-only. Searches `INDEX.md` lines with case-insensitive substring matching. It does not search `MEMORY.md`, `USER.md`, or topic file contents directly.

This keeps search cheap and forces the model to use the manifest first. If the model needs detailed content, it should call `memory.read` on the linked topic file.

Input:

```json
{"query": "router"}
```

Return up to `memory.max_search_results` matches:

```text
INDEX.md:3: - [system-info](topics/system-info.md): Current machine, OS, hardware, and runtime environment.
```

### `memory.write`

Write tool. Accepts:

- `file`: target file,
- `content`: complete replacement content,
- `mode`: `replace` or `append`.

`append` writes content at the end of the file with a separating newline.

`replace` overwrites the file. The model is responsible for preserving valid frontmatter and keeping files concise.

The `memory.write` tool description must include:

- the tool function: append to or replace an allowed memory file,
- allowed targets: `SOUL.md`, `USER.md`, `MEMORY.md`, `INDEX.md`, or `topics/<name>.md`,
- frontmatter format:

```markdown
---
type: soul|user|feedback|project|reference|system
description: one-line description
updated: YYYY-MM-DD
---
```

When writing `topics/<name>.md`, the model must also update `INDEX.md` so future searches can discover the topic. The tool should remind the model about this in the result text for topic writes, but the model owns the actual index entry wording.

When `memory.write` detects that a write leaves `SOUL.md` over 1000 characters, `USER.md` over 1000 characters, or `MEMORY.md` over 2000 characters, the tool should still complete the write, but return an additional maintenance prompt telling the model to summarize/consolidate the file and call `memory.write` again with `mode="replace"`.

Writes are permission-gated by the existing dynamic permission system.

## 10. Budgets

Automatic memory injection must obey `memory.max_recall_chars`.

Suggested per-file split:

- `SOUL.md`: max 1000 characters,
- `USER.md`: max 1000 characters,
- `MEMORY.md`: max 2000 characters,
- if one file is missing or short, the total message still obeys `memory.max_recall_chars`.

Topic files are never injected automatically. Tool results already obey `tools.max_result_chars`.

If injected memory exceeds the budget, truncate with:

```text
...[truncated]
```

## 11. Transcript Behavior

When always-on memory is injected, `AgentSession` writes a `memory_context` transcript event:

```json
{
  "files": ["SOUL.md", "USER.md", "MEMORY.md"],
  "truncated": false
}
```

Do not log full memory content in transcript events.

When `memory.enabled = false`, do not write `memory_context`.

## 12. Error Handling

Missing memory root, missing files, invalid frontmatter, and unreadable files are non-fatal.

Memory read/search tools return ordinary tool errors for invalid user/model arguments.

Memory context loading should never block a user turn with an exception.

## 13. Testing

Required tests:

- automatic context loads `MEMORY.md` and `USER.md`,
- automatic context bootstraps sample memory files when the memory root is absent or empty,
- automatic context does not overwrite existing memory files,
- automatic context ignores `INDEX.md` and `topics/*.md`,
- automatic context obeys `memory.max_recall_chars`,
- automatic context does not inject memory write guidance or per-file maintenance warnings,
- disabled memory injects no automatic memory,
- session sends memory context to the model without persisting it in `session.messages`,
- session logs `memory_context` with file names and truncation status,
- `memory.list` returns built-in files and topics,
- `memory.read` reads built-in files and topic shorthand,
- `memory.search` searches `INDEX.md` only,
- `memory.write` supports append and replace while rejecting traversal,
- `memory.write` description contains memory file format and routing guidance,
- `memory.write` result warns when `USER.md` or `MEMORY.md` exceeds its character limit,
- `memory.write` result reminds the model to update `INDEX.md` for topic writes,
- all tests run with `uv run python -m pytest`.

## 14. Future Work

Future memory improvements may add:

- optional model side-query selection similar to Claude Code,
- optional SQLite FTS for larger memory stores,
- file-size warnings for oversized always-on memory files,
- automatic memory consolidation commands,
- channel/session-specific memory roots.
