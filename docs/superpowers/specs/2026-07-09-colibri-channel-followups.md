# Colibri Channel Follow-up Issues

Date: 2026-07-09

This document records the channel/gateway issue currently selected for cleanup.

## Open Issues

### 1. Gateway sessions do not write transcripts

Status: open

Current behavior:

- `ask` and `repl` create `TranscriptWriter.default()` when `session.transcript = true`.
- `gateway` creates per-user `AgentSession` objects without passing a transcript writer.
- Weixin channel conversations, tool calls, tool results, permission decisions, and context compact events are therefore not written to disk.

Impact:

- Weixin conversations cannot be audited from `~/.colibri/transcripts/YYYY-MM-DD.jsonl`.
- Permission and tool-call debugging is harder in channel mode.
- Channel behavior differs from CLI behavior even when `session.transcript = true`.

Expected fix:

- Gateway should honor `session.transcript`.
- Gateway transcript events should include channel metadata such as `channel`, `sender_id`, and possibly `message_id`.
- Transcript writer sharing must be safe with gateway worker threads.
- Keep memory usage bounded; do not keep full channel transcript state in RAM.
