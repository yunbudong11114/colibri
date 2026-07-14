# Colibri SOUL Memory Design

Date: 2026-07-14
Status: Approved by user direction
Scope: Always-on memory roles, SOUL.md bootstrap, limits, tools, and context injection

## 1. Goal

Colibri's always-on memory now has three short Markdown files:

- `SOUL.md`: Colibri's stable persona, operating principles, tone, and self-constraints.
- `USER.md`: the user's stable preferences, collaboration style, and profile.
- `MEMORY.md`: stable project, device, environment, and long-term facts.

Detailed memories still live in `topics/*.md`, with `INDEX.md` as the searchable manifest. The goal is to give the model a tiny but expressive identity/preferences/facts layer without adding background review, embeddings, databases, or higher memory use.

## 2. Limits

Always-on file limits are strict guidance used by context loading and `memory.write` feedback:

| File | Limit | Role |
| --- | ---: | --- |
| `SOUL.md` | 400 chars | Persona, principles, expression style, and self-constraints |
| `USER.md` | 400 chars | User profile, stable preferences, and collaboration style |
| `MEMORY.md` | 1200 chars | Stable project/device/environment facts |

The automatic context loader truncates each file independently to its file limit, then applies `memory.max_recall_chars` as the total always-on context budget.

## 3. Bootstrap

When memory is enabled and the memory root is missing or empty, Colibri bootstraps:

```text
~/.colibri/memory/
  SOUL.md
  USER.md
  MEMORY.md
  INDEX.md
  topics/
    sample.md
```

Bootstrap must never overwrite existing user files. A non-empty legacy memory directory continues to be treated as user-owned; users or the model may create `SOUL.md` later with `memory.write`.

Sample files should be real examples, not hidden runtime prompts. They must explain when each file should be rewritten or consolidated, and that the first real write may replace the sample content.

## 4. Context Injection

`MemoryContext.load()` injects the always-on files in this order:

```text
Always-on memory:

[SOUL.md]
...

[USER.md]
...

[MEMORY.md]
...
```

Only files that exist and have non-empty content are included. The injected memory message is temporary model input for the current turn and is not appended to persisted session history.

## 5. Tools

`memory.list`, `memory.read`, and `memory.write` must treat `SOUL.md` as a built-in memory file alongside `USER.md`, `MEMORY.md`, and `INDEX.md`.

`memory.write` guidance should route:

- persona, principles, tone, and durable behavior constraints to `SOUL.md`;
- user preferences and collaboration style to `USER.md`;
- project, device, environment, and stable operational facts to `MEMORY.md`;
- detailed or bulky notes to `topics/<name>.md`, with `INDEX.md` updated afterward.

If `SOUL.md`, `USER.md`, or `MEMORY.md` exceeds its limit after a write, the tool result reminds the model to consolidate and replace that file.

## 6. Python/Rust Parity

Python and Rust must keep the same:

- bootstrap file set and sample intent,
- context injection order,
- per-file limits,
- memory tool accepted built-in filenames,
- oversize warning text shape,
- tests for bootstrap, truncation, read/list/write, and session memory-context events.
