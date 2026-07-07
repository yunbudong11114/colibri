# Colibri Memory Recall Design

Date: 2026-07-07
Status: Approved by roadmap
Milestone: 5
Scope: Automatic file memory recall and model context injection

## 1. Goal

Milestone 5 makes Colibri use file-backed memory automatically during model calls.

After this milestone, Colibri should:

- read the memory index from `MEMORY.md`,
- score indexed topic names and descriptions against the current user text and recent messages,
- read the top relevant topic files within a strict character budget,
- inject selected memory as a separate context block before the model call,
- record selected memory references in transcript events.

This milestone does not add embeddings, vector databases, model-based memory selection, memory rewriting, or skill loading.

## 2. Headless Requirement

Memory recall must run on pure Linux servers over SSH.

Rules:

- Use only Python standard library APIs.
- Do not keep all memory files resident in process memory.
- Do not require GUI, browser, audio, display, notification, or TUI frameworks.
- Keep recall deterministic and testable without network access.

## 3. Configuration

Extend `MemoryConfig`:

```python
enabled: bool = True
max_recall_topics: int = 3
max_recall_chars: int = 4000
```

Existing fields remain:

```python
root: Path = ~/.colibri/memory
max_search_results: int = 5
```

TOML overrides should support:

```toml
[memory]
enabled = true
root = "~/.colibri/memory"
max_search_results = 5
max_recall_topics = 3
max_recall_chars = 4000
```

If `memory.enabled = false`, no automatic recall runs, but explicit memory tools still remain available when `"memory"` is in `tools.enabled`.

## 4. Index Format

Recall reads `MEMORY.md` lines that look like Markdown bullets:

```markdown
- devices: Home devices, hostnames, GPIO wiring, network notes.
- preferences: User preferences and recurring constraints.
```

Parsing rules:

- Ignore blank lines and headings.
- Only parse bullet lines beginning with `- `.
- Split at the first `:`.
- The topic name must match the same topic-name rules as memory tools: ASCII letters, digits, `_`, and `-`.
- The text after `:` is the topic description.

Invalid lines are skipped.

## 5. Scoring

Use deterministic keyword overlap.

Input text:

- current user text,
- recent user and assistant messages already in `AgentSession.messages`.

Candidate text:

- topic name,
- topic description.

Tokenization:

- lowercase,
- split on non-alphanumeric ASCII characters,
- ignore tokens shorter than 2 characters.

Score:

- `2` points for topic-name token matches,
- `1` point for description token matches.

Sort by:

1. higher score,
2. topic name alphabetically.

Only topics with score greater than zero are selected.

## 6. Context Injection

Add a focused component:

```python
MemoryRecallResult:
    text: str
    topics: list[str]
    truncated: bool

MemoryRecall:
    recall(user_text: str, messages: list[Message]) -> MemoryRecallResult
```

`AgentSession.submit()` should call recall once per user turn after appending the user message and before the tool loop.

The selected memory block should be injected as a temporary system-style message before the existing conversation messages sent to the model:

```text
Relevant memory:

[devices]
- Router is upstairs.

[preferences]
- Keep answers concise.
```

This message must not be appended to `self.messages`; it is only part of the model input for that submit call.

If no relevant memory is found, do not inject a memory message.

## 7. Budgets

Memory recall must obey:

- `memory.max_recall_topics`,
- `memory.max_recall_chars`,
- `session.compact_trigger_chars` indirectly through existing message bounding.

If selected topic content exceeds `max_recall_chars`, truncate the final memory block with the existing suffix:

```text
...[truncated]
```

## 8. Transcript Behavior

When recall runs, `AgentSession` should write a `memory_recall` transcript event:

```json
{
  "topics": ["devices", "preferences"],
  "truncated": false
}
```

Do not log full memory content in transcript events.

If memory recall is disabled, do not write `memory_recall`.

## 9. Error Handling

Missing memory root, missing `MEMORY.md`, missing topic files, and invalid index lines are non-fatal.

If memory recall cannot read a file because of `OSError`, skip that file.

Recall should never block a user turn with an exception.

## 10. Testing

Required tests:

- config loads `memory.enabled`, `memory.max_recall_topics`, and `memory.max_recall_chars`,
- index parser skips invalid lines,
- recall selects topics by keyword overlap,
- recall obeys topic and character budgets,
- disabled recall injects no memory,
- `AgentSession` sends memory context to the model without persisting it in `session.messages`,
- `AgentSession` logs `memory_recall` with topic names and truncation status,
- all tests run with `uv run python -m pytest`.

## 11. Future Work

After this milestone:

- summary compacting,
- safer shell permission fixes,
- local skill loading,
- MCP bridge,
- structured memory proposals.
