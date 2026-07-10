# Colibri Shared Transcript Restore Design

## Goal

Use Colibri's existing shared JSONL transcripts to restore recent personal
assistant context after REPL, ask, or gateway sessions are recreated. USER.md
and MEMORY.md remain the source of stable long-term memory; no AUTO_MEMORY.md or
per-session persistence database is added.

## Shared History Model

All entry points share ~/.colibri/transcripts/*.jsonl as one chronological
restore source. Gateway metadata such as channel, sender, and session key is
used only to match a user event with its final assistant response. It does not
filter restored history. Completed turns from every source are merged by
completion order and made available to every newly created AgentSession.

The runtime keeps existing in-memory gateway sessions. A session restores the
shared transcript once, immediately before its first submitted user message.
It does not live-refresh while active.

## Event Selection

The loader accepts user_message events with non-empty payload.text and
assistant_message events with non-empty payload.text and
payload.tool_call_count equal to zero.

Intermediate assistant tool-call messages, tool results, permissions, status,
memory context, skills, and malformed JSONL rows are ignored. A source-specific
FIFO pairs user events with final assistant events. Unanswered user events and
assistant events without a preceding user are not restored.

Temporary attachment path blocks beginning with "Attachments saved locally:"
are removed from restored user text. Human-readable image/file placeholders may
remain, but stale /tmp/colibri/media paths must not be reintroduced.

## Bounded Loading

Transcript files are sorted newest first. The loader reads binary tails rather
than entire files and stops after scanning the configured byte budget across
files. A partial first line caused by a tail seek is discarded.

Completed turns are returned in chronological order and bounded by whole turns:

- session.restore_message_limit, default 24 messages;
- session.restore_char_limit, default 24,000 characters;
- session.restore_scan_bytes, default 2 MiB of JSONL input.

The existing model input budget remains the final safety boundary.

## Integration

Add session_history.py with a TranscriptHistoryLoader. CLI and Gateway construct
one loader from AgentConfig and pass its callable into AgentSession.
AgentSession tracks whether restore was attempted and calls the loader at most
once, before appending the first new user message. Loader errors produce an
empty history and do not prevent startup.

Restored messages are not written back to transcript. Only newly submitted
messages continue through the existing TranscriptWriter, preventing duplication
on every restart.

## Transcript Disk Retention

Transcript persistence remains daily JSONL. When a TranscriptWriter opens, and
at most once per 60 seconds while it is active, it cleans direct JSONL children
of the transcript directory:

- delete files older than session.transcript_retention_days, default 30;
- then delete oldest inactive files until total size is at most
  session.transcript_max_total_bytes, default 128 MiB;
- never delete the currently open transcript file;
- ignore stat, list, and unlink failures.

A single active daily file may temporarily exceed the total budget; it is never
unlinked underneath its writer. Setting a retention limit to zero disables that
corresponding cleanup rule.

## Configuration

The session table adds restore_transcript=true, restore_message_limit=24,
restore_char_limit=24000, restore_scan_bytes=2097152,
transcript_retention_days=30, and
transcript_max_total_bytes=134217728.

## Tests

Cover cross-file reverse scanning, malformed and partial lines, completed-turn
selection, tool-call filtering, global metadata merging, attachment path
stripping, whole-turn message and character limits, one-time AgentSession
loading, and transcript age and size cleanup that preserves the active file.
