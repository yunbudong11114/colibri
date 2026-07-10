# Colibri Session Structure Optimization Design

## Goal

Reduce the responsibilities visible inside AgentSession.submit without adding
new architectural layers. Preserve all current model, tool, permission,
compaction, media, memory, skill, and transcript behavior.

## Chosen Approach

Keep AgentSession as the application-level conversation coordinator and extract
focused private methods inside session.py. This is preferred over introducing
separate turn-runner or tool-executor classes because the current project is
small, the collaborators are session-specific, and extra objects would add
indirection without reducing memory use.

The alternative of leaving submit unchanged has the least code churn, but it
makes future channel and multimodal changes increasingly difficult to review.
The alternative of splitting orchestration across several files provides
stronger isolation but is premature for the current code size.

## Runtime Dependency Lifecycle

ToolRegistry, PermissionPolicy, and ImageAnalyzer belong to an AgentSession and
must be initialized lazily once, then reused for later turns. Lazy construction
preserves lightweight test setup and avoids creating unused tools for sessions
that never receive a message.

MemoryContext and SkillIndex remain per-turn operations. Their backing files
can change while a session is active, so caching their results would make
memory and local skills stale.

## Submit Flow

AgentSession.submit remains the public synchronous entry point and reads as a
short orchestration sequence:

1. restore shared history once;
2. prepare and persist the new user message;
3. resolve reusable runtime dependencies and per-turn dynamic context;
4. run the bounded model/tool loop;
5. return either the final model response or the existing round-limit result.

Private methods may encapsulate user-message preparation, runtime dependency
resolution, dynamic context loading, model completion, and a single tool-call
execution. Message mutation and transcript event order must remain unchanged.

## Error Handling

History restoration remains non-fatal. If the loader raises, AgentSession must
write a history_restore_error transcript event containing only the exception
type and message, then continue with an empty restored history. It must still
attempt restoration at most once.

Model and tool behavior remains unchanged. Model errors are logged and raised;
permission denials are converted into tool results.

## Memory Constraints

The optimization must not duplicate message buffers or cache memory/skill file
contents. Reusing ToolRegistry and ImageAnalyzer reduces repeated allocations.
Existing bounded copies used for model calls and AgentResponse remain intact.

## Tests

Add tests proving that ToolRegistry and ImageAnalyzer are created once across
multiple submits, history restoration failures are recorded and non-fatal, and
existing transcript event ordering and tool-loop behavior continue to pass the
full suite.
