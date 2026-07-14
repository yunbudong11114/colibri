# Colibri Gateway Thin Bus Design

Date: 2026-07-14
Status: Revision 2 implemented; release verification pending
Scope: Channel/gateway inbound-outbound shape, channel registration, permissions, concurrency, media pipeline
Non-goals: cron, heartbeat, new chat channels, MCP, web dashboard

## 1. Goal

Move Colibri gateway toward this shape (without cron/heartbeat):

```text
[Weixin poll]──轻量信封──► InboundRouter ─► per-session queue ─► AgentSession
                                                      │
                                                      ▼
                                           OutboundDispatcher
                                                      │
                                               Channel.send_*
```

Preserve: permissions, transcript, steering, immediate dispatch, Python/Rust parity, CM0 friendliness.

## 2. Problems to fix

1. **Global single worker** — unrelated senders block each other.
2. **Inbound media downloaded on poll thread** — blocks steer + permission waiters.
3. **Python session stays in cache during `submit`** — idle eviction can close an in-flight session; Rust already uses take/put + SteerHandle.
4. **Weixin special-cases in gateway** — `isinstance(WeixinChannel)` / Rust logic in `cli.rs` blocks a second channel.
5. **Outbound is ad-hoc closures** — steer ack / final reply / media / permission prompts call Weixin APIs directly from session wiring.

## 3. Non-Goals

- cron / heartbeat / inbound task injection
- Telegram or other new channels in this change (registry must allow them later)
- ZeroClaw-style async orchestrator, feature-gated channel zoo
- Changing Weixin API semantics or removing steering
- Implementing a second production chat service. A fake adapter proves the extension boundary.

## 4. Core types

### 4.1 InboundEnvelope

Channel-agnostic inbound work item:

```text
channel: str
sender_id: str
text: str
message_id: str
media_refs: list[MediaRef]   # not fully downloaded bytes
context: map                 # e.g. weixin context_token (channel-private)
```

`MediaRef` is enough to download later (Weixin: item metadata / CDN params / type / filename). Poll must not call CDN download.

Legacy `InboundMessage` with resolved `media: list[MediaPart]` remains the **agent-facing** type after the worker resolves refs.

### 4.2 OutboundEnvelope

```text
channel: str
recipient_id: str
kind: text | ack | media | permission_prompt
text: str                    # for text/ack/permission_prompt
media: MediaPart | None      # for media
```

Session and steering never call `WeixinChannel.send_*` directly. They emit outbound envelopes (or use an `OutboundSink` callback that only enqueues).

### 4.3 Session key

Unchanged: `"{channel}:{sender_id}"`.

## 5. Runtime components

### 5.1 Channel adapter (Weixin)

Responsibilities:

- Long-poll `getupdates`
- Parse to **InboundEnvelope** (store context_token; attach media **refs** only)
- Permission text-waiter routing (unchanged)
- Offer text-only envelopes to `try_steer` before enqueue
- Implement `send_text` / `send_media` for the outbound dispatcher
- Provide `PermissionPrompter` factory for this channel (no gateway `isinstance`)

Rust exposes a transport-neutral `GatewayChannel` trait from `channel.rs`:

```text
name(&self) -> &str
poll_once(&self) -> Result<Vec<InboundEnvelope>, String>
resolve_inbound_media(&self, &mut InboundEnvelope) -> Result<(), String>
outbound_for(&self, &InboundEnvelope) -> Result<Arc<dyn OutboundSink>, String>
permission_prompter(outbound, session_key, waiters) -> Option<Box<dyn PermissionPrompter>>
```

`WeixinGatewayChannel` implements that trait in `weixin.rs` and owns its
poll cursor. `channel_registry.rs` is the only production composition root:
it checks config and returns enabled `Arc<dyn GatewayChannel>` instances.
The generic gateway receives that registry and never imports Weixin API
functions, constructs a Weixin sink, or matches a channel name. The registry
type is a `BTreeMap<String, Arc<dyn GatewayChannel>>`; duplicate names and an
envelope whose `channel` differs from its adapter name are rejected.

Python keeps the equivalent `Channel` protocol and `channels.registry`
composition root. The protocol methods and lifecycle must remain behaviorally
aligned with Rust even when language-specific signatures differ.

### 5.2 InboundRouter

- Accepts envelopes from any channel adapter
- Enqueues onto **per-session** bounded queues
- Global bound: sum of pending envelopes ≤ `gateway.max_pending_inbound` (default 8, same order as today)
- If a session queue is full or global bound hit: fail-fast / drop with status log (same spirit as today’s backpressure; do not deadlock poll)

### 5.3 Turn scheduler

- At most `gateway.max_concurrent_turns` active `submit` calls (default **1** for CM0)
- When a turn slot frees, pick the next envelope from a non-empty session queue (fair round-robin across sessions that have work)
- **Same session never has two concurrent submits** (steering handles mid-turn messages)

Default `max_concurrent_turns = 1` keeps RSS predictable on Cardputer while still giving **per-session ordering** and a clean path to raise to 2 later.

The router tracks both pending and active work. `idle` means **both** counts
are zero. A finite channel poller may end in tests or embedding scenarios;
gateway shutdown must wait for router idle before closing sessions. A worker
that already acquired an item is active even though `pending_len == 0`.

### 5.4 Session owner (take / put)

Both Python and Rust:

1. `take_or_create(key)` — remove session from idle-eviction set; register `SteerHandle`
2. Resolve `media_refs` → `MediaPart` (channel helper)
3. Bind outbound sink + permission prompter for this turn
4. `submit(...)`
5. `put_back(key)` + `touch(key)`

Idle eviction must not close a taken session. Python must match Rust here.
Once a Rust worker takes a session, it must return the session to the cache on
both successful and failed model submission paths before propagating an error.

### 5.5 OutboundDispatcher

Single outbound path for gateway:

- Serial send per channel connection if required by Weixin (one send at a time is OK)
- Kinds:
  - `ack` — steering short Chinese ack (existing UX)
  - `text` — final assistant reply
  - `media` — tool `files.send` / media results
  - `permission_prompt` — permission UI text

Ordering: for one recipient, preserve enqueue order. Mid-turn ack may interleave before final text by design.

REPL does **not** need this dispatcher; local CLI keeps direct stdout / local prompter.

### 5.6 Permission confirmation (channel-agnostic)

The Weixin numeric reply flow is the shared **text-reply permission** pattern:

1. Format a channel-neutral prompt (`format_channel_permission_prompt`)
2. Send it through `OutboundSink` / `channel.send_text`
3. Wait for the same channel session's next text message (`prompt_for_text` / waiter map)
4. Parse `"0"…"5"` into a grant choice

Any new chat channel that can round-trip plain text should reuse `ChannelTextPermissionPrompter` (Python/Rust). REPL keeps `ConsolePermissionPrompter`. Channels without interactive text may return `None` from `permission_prompter` and fall back to policy defaults / deny.

Permission waiter identity is always the complete session key
`"{channel}:{sender_id}"`, never a bare sender ID. Registration and inbound
delivery must use the same helper. Thus `weixin:user-1` cannot consume a reply
intended for `another:user-1`. Rust centralizes this in a
`ChannelPermissionWaiters` wrapper so callers cannot accidentally choose a
different map key. Python's channel-local waiter map also stores the complete
session key for explicit parity and regression coverage.

## 6. Gateway surface cleanup

`GatewayRunner.handle_message` (or its replacement) must not:

- import/branch on `WeixinChannel` by type
- install Weixin-only closures inline

Instead:

- Channel registry entry: `{ name, build(config), permission_prompter_factory, resolve_media(refs) → parts }`
- Outbound sink obtained from dispatcher bound to `channel` + `recipient_id`

Rust module ownership:

- `channel.rs`: generic envelopes, adapter trait, outbound trait, permission waiters/prompter
- `channel_registry.rs`: config-to-adapter construction only
- `weixin.rs`: Weixin polling, media resolution, outbound implementation
- `gateway.rs`: generic poll scheduling, routing, sessions and turn workers

Adding a production channel may add its module, config and one registry entry;
it must not require a branch or match in `gateway.rs`.

## 7. Config

```toml
[gateway]
enabled_channels = ["weixin"]
max_sessions = 4
session_idle_seconds = 600
max_pending_inbound = 8
max_concurrent_turns = 1
```

- `max_pending_inbound` replaces the hard-coded Weixin `MAX_PENDING_MESSAGES = 8` (behaviorally equivalent default).
- `max_concurrent_turns` is new; default 1.

Reject unknown fields as today. No cron/heartbeat keys.

## 8. Steering and permissions (unchanged rules)

- Text-only inbound for an active same-session turn → steer queue (capacity 4)
- Permission pending → no steer; text waiter consumes reply
- Steer ack via outbound `ack` (Weixin immediate short Chinese ack)
- Empty final reply after steer-only handling does not double-send

## 9. Python / Rust parity matrix

| Behavior | Both must |
|----------|-----------|
| Light poll (no CDN on poll thread) | Yes |
| Per-session inbound queues + global pending bound | Yes |
| `max_concurrent_turns` | Yes |
| take/put + SteerHandle | Yes |
| Outbound kinds text/ack/media/permission_prompt | Yes |
| No Weixin type switch in generic gateway core | Yes |
| Waiter key is `channel:sender_id` | Yes |
| Finite pollers drain pending and active turns | Yes |
| Transcript scoped metadata | Yes (existing) |

## 10. Migration / rollback

- User-visible Weixin chat behavior should stay the same aside from: less poll stall under media; fairer multi-sender latency when `max_concurrent_turns > 1`.
- Config: new keys optional with defaults; removing them restores prior numeric limits.
- Rollback: revert the gateway/channel commits; no data migration (sessions remain in-memory only).

## 11. Acceptance

1. Poll thread does not download Weixin CDN bodies.
2. Two different senders with `max_concurrent_turns = 1` still serialize turns, but each session’s messages stay ordered; raising to 2 allows parallel turns for different keys.
3. Python cannot idle-evict a session mid-submit.
4. Steering ack and final reply both go through outbound dispatcher.
5. Gateway core has no `isinstance(..., WeixinChannel)`.
6. Existing Weixin/steering/permission unit tests updated and green on Python and Rust.
7. No cron/heartbeat code paths.
8. A fake Rust adapter can be registered and exercise inbound, outbound,
   media-resolution and permission paths without editing generic gateway code.
9. Two adapters with the same `sender_id` cannot consume each other's
   permission response.
10. Python `GatewayRunner.run()` does not return or close sessions while an
    acquired turn is still active.

## 12. Implementation phases

**Phase A — correctness + light poll**  
take/put (Python), SteerHandle parity, defer media download to worker, keep single global worker temporarily.

**Phase B — thin bus**  
InboundEnvelope / OutboundDispatcher, remove Weixin branches from gateway core, move Rust worker out of `cli.rs`.

**Phase C — per-session queues + `max_concurrent_turns`**  
Router + scheduler; default concurrent turns = 1.

Ship A→B→C behind one design; each phase stays mergeable and testable.
