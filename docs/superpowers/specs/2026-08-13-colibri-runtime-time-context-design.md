# Runtime Time Context Design

## Goal

Give the model reliable awareness of the host's current local date and time while keeping Python and Rust behavior aligned and preserving Colibri's low-memory design.

## Prompt Layout

Normal agent completion requests have two logical regions:

1. A stable prefix containing the identity prompt, summary, memory, skill catalog, tool schema, and prior persisted messages.
2. A dynamic tail on the current user turn containing request-time metadata.

The stable prefix must remain byte-identical when only the clock, channel, sender, or other request metadata changes. Dynamic metadata must never be inserted before stable context.

Before every normal agent completion request, append this transient section to the latest user message in the request-local message copy:

```text
## Current Time
YYYY-MM-DD HH:MM (+HH:MM)
Use this only when needed. Do not mention or repeat the current time unless the user asks or it is relevant.
```

The final instruction is part of the transient runtime metadata. It prevents the
model from treating clock availability as a request to announce the time in every
reply.

- Read the clock again for every model tool round so a long-running turn receives an updated value on its next completion request.
- Use the host's local timezone and numeric UTC offset. Do not hard-code Beijing time and do not add a timezone configuration field.
- Use minute precision. Seconds add prompt churn without improving normal conversation behavior.
- Pass the existing system prompt unchanged to every normal completion request.
- Clone the model messages first, then append the runtime section to the most recent `user` message. This keeps every message before the active user turn reusable as a stable provider-cache prefix.
- Do not append the section to session messages, restored history, transcript payloads, summaries, memory, or skill context.
- Internal summary-compaction and image-understanding requests keep their dedicated prompts and do not receive this context.
- Channel and sender metadata, when present, follow the same rule: they may change the active user message but must not modify the preceding stable prefix.
- The fake model continues to echo and parse only the original user text. It removes the known request-local time suffix before applying its deterministic test behavior.

## Implementation

### Rust

- Add a small `time_context` module.
- Read local time through the existing `libc` dependency using thread-safe platform time conversion.
- Format the UTC offset as `+HH:MM` or `-HH:MM`.
- Fall back to UTC if local time conversion is unavailable.
- Do not spawn `date` and do not add a crate.
- Apply the formatted section only to the cloned completion messages immediately before calling the model.

### Python

- Add an equivalent `time_context` module using `datetime.now().astimezone()`.
- Produce the exact same heading, date/time precision, and UTC-offset format as Rust.
- Apply the formatted section at the same request boundary and to the same latest-user-message target as Rust.

## Tests

- Verify the formatter produces the shared wire format, including positive, negative, and UTC offsets.
- Verify normal session completion receives the unchanged stable system prompt and a transient current-time section on the latest user message.
- Verify the section is absent from persistent session messages.
- Verify that two requests with different times have a byte-identical prefix before the active user message.
- Verify that channel or sender changes affect only the active user turn and do not alter the preceding stable prefix.
- Verify fake-model replies and scripted tool calls do not expose or consume the transient time suffix as user text.
- Keep existing system-prompt and context-compaction behavior unchanged.
- Run the complete Python and Rust test suites before building the release binary.

## Deployment

Build the Rust release binary and install the resulting `colibri` executable to `~/.local/bin/colibri` after all tests pass.
