# Colibri Runtime Hardening Design

## Scope

This change hardens four existing Python runtime areas without adding new
features or reorganizing unrelated modules:

1. preserve OpenAI-compatible tool-call history as atomic message groups;
2. guarantee that the Weixin receive loop can stop after worker failure;
3. bound temporary Weixin media retention on disk;
4. remove runtime configuration and helpers that have no implementation or
   caller.

The current image understanding, Weixin complementary-message batching,
permission behavior, memory format, and gateway session model remain unchanged.

## Atomic Tool-Call History

An assistant message containing one or more tool calls and every immediately
following tool-result message form one logical message group. A normal user,
assistant, system, or standalone tool message forms a one-message group.

Both history operations must consume these groups instead of individual
messages:

- context compaction retains the newest groups whose total message count fits
  `session.recent_message_limit`; a group is retained whole even when that makes
  the retained message count slightly exceed the configured limit;
- model-input budgeting drops the oldest droppable group while preserving all
  system groups and the group containing the latest user message.

The latest user group is inserted before retained recent groups when it would
otherwise be outside the recent window. Duplicate insertion is not allowed.
This guarantees that a tool result is never sent without its originating
assistant tool call.

Character accounting includes message role and content as before. Expanding it
to serialized tool schemas and arguments is outside this focused change.

## Weixin Worker Shutdown

The receive loop and worker keep the current bounded queue and serialized agent
execution. A shared stop event records worker termination or channel shutdown.

- Worker failure stores the original exception, sets the stop event, and exits.
- Queue publication uses short timed puts and checks the stop event between
  attempts, so producers cannot block forever after worker termination.
- Finalization sets the stop event, attempts a non-blocking stop-sentinel
  insertion, and joins the worker with a bounded timeout.
- The receive loop re-raises the worker exception. Normal external shutdown does
  not manufacture an error.

Inbound messages use the same interruptible publisher and therefore cannot
remain blocked on a full queue after shutdown.

## Temporary Media Cleanup

Weixin inbound media remains under `/tmp/colibri/media`. Cleanup uses no
background thread.

- Retention age: 24 hours.
- Total directory budget: 256 MiB.
- Maintenance cleanup interval: at most once per 60 seconds per process when no
  file is being written.
- Cleanup runs when a Weixin channel starts. Before every inbound-media write,
  capacity cleanup reserves space for the incoming byte length.
- Files older than the retention age are removed first.
- If remaining regular files exceed the budget, oldest files are removed until
  the total is within the budget.
- Files that disappear, cannot be stat'ed, or cannot be deleted are skipped.
- Cleanup failure must not prevent message handling.

Only regular files directly under the media directory are managed. Directories
and unrelated paths are never traversed or removed.

## Redundant Runtime Surface

MCP remains a deferred milestone, but the Python runtime must not advertise it
as active. Remove `mcp` from the default tool list, remove `McpConfig` and the
`AgentConfig.mcp` field, and remove the `[mcp]` example section. Unknown TOML
sections continue to be ignored, so an existing user configuration containing
`[mcp]` still loads.

Remove `memory.max_recall_topics`, which has no runtime behavior, from the
configuration model and examples. Remove the unused Weixin `_text_from_items`
helper. Deferred MCP intent remains documented in milestone documentation
rather than exposed as live configuration.

## Tests

Add regression coverage for:

- compaction retaining complete assistant/tool groups at the recent boundary;
- character budgeting dropping complete groups and retaining the latest user;
- worker failure with a full queue returning instead of deadlocking;
- age-based and size-based media cleanup;
- cleanup errors being non-fatal;
- default and loaded configuration no longer exposing MCP or
  `max_recall_topics`.

The complete unit suite and `git diff --check` must pass.
