# Colibri Steering Display Design

Date: 2026-07-10  
Status: Approved / Implemented  
Scope: How steering is shown and accepted on **Weixin** and **local REPL** (Python + Rust parity).  
Implementation plan: `docs/superpowers/plans/2026-07-10-colibri-steering.md`  
Related: `docs/reference/2026-07-10-picoclaw-zeroclaw-lessons-for-colibri.md` (items 1 / 5); console display `2026-07-10-colibri-console-display-design.md`; Weixin gateway `2026-07-08-colibri-weixin-gateway-design.md`.

## 1. Goal

When the user sends a new message while a tool batch is still running, Colibri can **steer**: skip remaining tools, inject the new user text, and continue the loop.

This document locks the **display and interaction surface** for that behavior on:

- Weixin channel (primary chat UI on phone),
- local REPL on CardputerZero-class console (320×170, no GUI, real TTY).

Core loop mechanics (queue, skip results, Python/Rust parity) must follow the same rules; a separate implementation plan will cover MessageBus decoupling (lesson item 1) and session wiring.

## 2. Non-Goals

- Full-screen TUI (Claude Code–style sticky input + scroll region)
- Weixin interactive buttons / template cards for permissions or steering (iLink has no such controls today)
- Changing permission choice letters (`y/s/e/p/n`) or adding button-based permission UI
- Hardware tools (lesson item 7)
- Echoing full steering text on the local REPL

## 3. Locked Decisions

| Topic | Choice |
|-------|--------|
| Weixin on steer | **Immediate short ack** (Approach A) |
| REPL on steer | **Status line only** — no body echo (Approach A) |
| Milestone local input | **Lightweight concurrent REPL** — type one line while busy; no TUI (Approach B) |
| During permission wait | **Steering disabled** (Approach C) — stdin / Weixin waiter serve permission only |
| Display package | **Approach 2** — Weixin ack includes `skipped=N` and optional short preview; REPL status only |

## 4. Display Spec

### 4.1 Shared status event (REPL + transcript)

When `console.status = true`, emit one stderr line:

```text
[colibri] steered skipped=2
```

Optional field when useful for debugging (still one line, no body):

```text
[colibri] steered skipped=2 chars=18
```

Rules:

- Same `[colibri]` prefix and one-line style as existing tool/compact status.
- Do not print the steering message body on REPL.
- Do not use colors or cursor control.
- Python `StatusTranscript` / Rust CLI status wrapper both emit this event.
- Write a transcript event `steered` with `{skipped, chars, text}` (no full text required in status; transcript stores bounded text for restore/debug — **max 200 chars**).

### 4.2 Weixin ack (Approach 2)

As soon as steering is accepted (after current tool finishes and remaining tools are marked skipped), send **one** short text message to the same user **before** the next model call:

```text
已改方向，跳过剩余 2 个工具
改：别用 rm…
```

Rules:

- Line 1 is required: fixed pattern `已改方向，跳过剩余 N 个工具` (`N` = skipped count; use `0` only if steer happens with no remaining tools — prefer omitting ack only when nothing was skipped **and** no batch was active; see §5).
- Line 2 optional: `改：` + first **20** display characters of steering text (Unicode scalar count), ellipsis `…` if truncated. Omit line 2 if steering text is empty after strip.
- Plain text only; no markdown.
- Ack is **channel UX only** — not injected into model context (the full steering user message is injected separately as a normal user message).
- Do not send a second ack for the same steer event.
- Final assistant answer is still sent as today after the steered turn completes.

### 4.3 Permission prompts (unchanged shape)

Keep current text confirmation:

- REPL: detail lines + blocking `input` with `[y]…[n]`.
- Weixin: multi-line text + text-waiter mapping to `y/s/e/p/n`.

No buttons in this milestone.

## 5. When Steering Is Accepted vs Rejected

### 5.1 Accepted

- An agent turn is active (inside `submit` / tool batch).
- Not waiting on a permission prompt.
- User text is non-empty after strip.
- Queue not full (bound **4** messages per turn).

Then:

1. Finish the **current** in-flight tool (do not kill mid-syscall unless already cancellable).
2. Mark all **remaining** tools in the batch as skipped with a fixed tool result text (parity with PicoClaw intent): `Skipped due to queued user message.`
3. Emit REPL status + Weixin ack (§4).
4. Inject steering text as a user message into session history.
5. Continue the agent loop (next model call).

### 5.2 Rejected / deferred (Approach C)

While a permission prompt is active:

- **REPL:** input is only for permission choices; non-choice lines are ignored or briefly rejected with a one-line stderr hint such as `[colibri] permission_pending` (no steering enqueue).
- **Weixin:** text-waiter consumes the reply for permission; do **not** treat it as steering. Unrelated messages during permission wait keep existing gateway behavior (do not start a parallel turn).

After permission resolves, steering becomes available again for subsequent tools in the same turn.

### 5.3 Idle session

If no turn is active, a new user message starts a normal `submit` (not steering). Concurrent REPL reader must not treat the next `colibri>` line as steering.

## 6. Surface Behavior

### 6.1 Weixin

```
User: (turn running tools)
User: 别用 rm，改成 ls
Bot:  已改方向，跳过剩余 2 个工具
      改：别用 rm，改成 ls
… (remaining tools skipped; model continues)
Bot:  (final answer)
```

Ingress: while the worker is inside `submit`, a new inbound message for the **same sender session** should be offered to the session steering queue instead of starting a second concurrent `submit`. (Exact wiring with the existing single worker queue is an implementation detail; must preserve one active turn per session.)

### 6.2 Local REPL (lightweight concurrent input)

```
colibri> do something long
[colibri] thinking
[colibri] tool shell.run ok chars=12
(user types while busy)
[colibri] steered skipped=2
(final plain answer)
colibri>
```

Rules:

- During `submit`, keep the ability to read **one line at a time** from the TTY (or a test double) and enqueue as steering.
- No alternate screen, no sticky bottom chrome, no in-app scrollback UI.
- History scrollback remains the terminal/fbcon’s own scrollback.
- Prompt `colibri>` returns only when the turn is idle again.
- Optional: while busy, do not reprint a full prompt; accept raw lines (implementation may show a minimal `>` or nothing — prefer **nothing extra** on CM0 to avoid clutter; document the chosen behavior in the plan).
- Non-TTY / piped stdin: concurrent steering is best-effort or disabled; Weixin remains the primary steer surface.

### 6.3 `ask` command

Single-shot `ask` has no interactive follow-up; steering does not apply unless a future API injects a queue. No display change required.

## 7. Python / Rust Parity

| Behavior | Both runtimes |
|----------|----------------|
| Skip remaining tools + fixed skip result text | same string |
| Status `[colibri] steered skipped=N` | same |
| Weixin ack Chinese template + 20-char preview | same |
| Permission wait blocks steering | same |
| Concurrent REPL line → steer queue | same semantics |
| Ack not in model context; full steer text is | same |

## 8. Config

Minimal for this milestone (optional; defaults match locked UX):

```toml
[session]
# existing keys…

[console]
status = true
plain_answer = true
```

No new config required for Approach 2 defaults. If needed later:

```toml
[session]
steering_ack = true   # Weixin immediate ack; default true
```

Prefer shipping **without** new knobs first; add only if rollback demands it.

## 9. Rollback

- Feature-flag or revert the steering enqueue paths; behavior returns to “queue as next turn” / blocking REPL.
- Weixin ack and status lines disappear with the feature.
- Permission text UI unchanged, so rollback does not break confirms.

## 10. Tests (display / interaction focused)

- Steer mid-batch: remaining tools get skip result; status emits `steered skipped=N`.
- Weixin ack text matches template; preview truncates at 20 chars; ack not present in model-bound user message list as the ack string.
- During permission wait, steering is not enqueued (REPL + Weixin).
- Idle message starts normal submit, not steer.
- Python and Rust parity tests for ack formatter and skip result string.

## 11. Out of Scope Here / Follow-ups

- MessageBus + outbound manager refactor (lesson item 1) — may land in the same implementation plan as plumbing for Weixin ack send-without-ending-turn.
- Claude Code–style TUI — separate milestone if ever needed on larger terminals.
- Weixin buttons — blocked on iLink API.
- Hardware tools — wait for device.
