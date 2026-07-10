# Colibri Steering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add mid-turn steering (skip remaining tools, inject user text, continue) with Weixin Approach-2 ack and lightweight concurrent REPL input, keeping Python and Rust behavior aligned.

**Architecture:** `AgentSession` owns a small thread-safe steering queue and a `permission_pending` flag. After each tool in a batch, poll the queue; on hit, skip remaining tools with a fixed result string, emit status/transcript, optionally call `steer_notifier(ack_text)` for Weixin mid-turn ack, append the steering user message, and continue the model loop. Gateway routes same-session inbound to `session.steer()` while a turn is active instead of starting a second `submit`. Full MessageBus refactor is deferred; this milestone only adds the minimal mid-turn outbound notify hook needed for ack.

**Tech Stack:** Python 3.11+ stdlib (`threading`, `queue`), existing Colibri session/gateway/weixin/console; Rust `colibri-rust` with matching APIs; pytest + `cargo test`.

**Spec:** `docs/superpowers/specs/2026-07-10-colibri-steering-display-design.md`

---

## File map

| File | Responsibility |
|------|----------------|
| `src/colibri/steering.py` (new) | Constants, ack formatter, queue helper |
| `src/colibri/session.py` | Poll/skip/inject loop; `steer()` / turn-active / permission_pending |
| `src/colibri/console.py` | Status line for `steered` transcript events |
| `src/colibri/gateway.py` | Active-turn → `steer()` + notifier wiring |
| `src/colibri/channels/weixin.py` | No API change required beyond existing `send_text` used by notifier |
| `src/colibri/cli.py` | Concurrent line reader during `submit` |
| `tests/unit/test_steering.py` (new) | Formatter + session steer tests |
| `colibri-rust/src/steering.rs` (new) | Rust parity helpers |
| `colibri-rust/src/session.rs` | Same loop semantics |
| `colibri-rust/src/console.rs` / `cli.rs` / `gateway.rs` | Status, REPL, gateway parity |
| `colibri-rust/tests/runtime.rs` | Rust tests + parity mappings |

---

### Task 1: Steering helpers (Python)

**Files:**
- Create: `src/colibri/steering.py`
- Create: `tests/unit/test_steering.py`

- [ ] **Step 1: Write failing formatter tests**

```python
from colibri.steering import SKIPPED_TOOL_RESULT, format_steering_ack

def test_skip_result_constant():
    assert SKIPPED_TOOL_RESULT == "Skipped due to queued user message."

def test_ack_with_short_preview():
    assert format_steering_ack(2, "别用 rm") == "已改方向，跳过剩余 2 个工具\n改：别用 rm"

def test_ack_truncates_preview_at_20_chars():
    text = "一二三四五六七八九十一二三四五六七八九十多余"
    ack = format_steering_ack(1, text)
    assert ack.startswith("已改方向，跳过剩余 1 个工具\n改：")
    preview = ack.split("\n", 1)[1].removeprefix("改：")
    assert preview.endswith("…")
    assert len(preview.rstrip("…")) == 20

def test_ack_omits_preview_when_empty():
    assert format_steering_ack(0, "  ") == "已改方向，跳过剩余 0 个工具"
```

- [ ] **Step 2: Run tests — expect fail (module missing)**

Run: `/opt/homebrew/bin/uv run python -m pytest tests/unit/test_steering.py -v`  
Expected: import error or FAIL

- [ ] **Step 3: Implement helpers**

```python
# src/colibri/steering.py
from __future__ import annotations

SKIPPED_TOOL_RESULT = "Skipped due to queued user message."
STEERING_QUEUE_MAX = 4
STEERING_PREVIEW_CHARS = 20

def format_steering_ack(skipped: int, steering_text: str) -> str:
    line1 = f"已改方向，跳过剩余 {skipped} 个工具"
    stripped = steering_text.strip()
    if not stripped:
        return line1
    preview = stripped[:STEERING_PREVIEW_CHARS]
    if len(stripped) > STEERING_PREVIEW_CHARS:
        preview += "…"
    return f"{line1}\n改：{preview}"
```

- [ ] **Step 4: Run tests — expect PASS**

Run: `/opt/homebrew/bin/uv run python -m pytest tests/unit/test_steering.py -v`

- [ ] **Step 5: Commit** (only if user asked to commit in this session; otherwise leave staged work uncommitted and continue)

---

### Task 2: Session steering loop (Python)

**Files:**
- Modify: `src/colibri/session.py`
- Modify: `tests/unit/test_steering.py`
- Modify: `tests/unit/test_session.py` only if existing tests break

- [ ] **Step 1: Write failing session steer tests**

Use a fake model that returns two tool calls then final text; tools that record execution. After first tool starts/finishes, call `session.steer("change plan")` from the test thread (or enqueue before second tool via a tool side-effect).

```python
def test_steer_skips_remaining_tools_and_injects_user_message(tmp_path):
    # Model1: two tool_calls (a, b); after steer, Model2: final text
    # Assert tool b never ran; tool result for b == SKIPPED_TOOL_RESULT
    # Assert a user message with "change plan" appears after skipped tool messages
    # Assert final answer returned
    ...

def test_steer_rejected_while_permission_pending(tmp_path):
    # Prompter sets/clears via session.begin_permission_wait / end (or policy hook)
    # session.steer("x") returns False while pending
    ...
```

Concrete fake pattern (adapt to existing `SingleToolCallModel` style in `test_session.py`):

```python
class TwoToolsThenText:
    def __init__(self):
        self.calls = 0
    def complete(self, messages, tools, system_prompt, limits):
        self.calls += 1
        if self.calls == 1:
            return ModelResponse(
                text="",
                tool_calls=[
                    ToolCall(id="1", name="files.list", arguments={"path": "."}),
                    ToolCall(id="2", name="files.list", arguments={"path": "."}),
                ],
            )
        return ModelResponse(text="steered-ok")
```

Enqueue steer from a custom tool `run` that calls `session.steer("change plan")` on first invocation, **or** call `steer` between tools by patching `_execute_tool_call` completion — prefer: after first tool in test, manually `session.steer(...)` if the loop polls a queue that was filled before submit via a background thread started at submit time. Simplest reliable approach:

```python
def test_steer_skips_remaining_tools(...):
    session = AgentSession(...)
    # Pre-fill queue so poll after tool 1 sees it
    assert session.steer("change plan") is True  # may return False if turn inactive
```

**API choice (lock this):** `steer(text) -> bool` succeeds only when `turn_active and not permission_pending and queue not full`. Pre-fill before `submit` must **fail** (turn inactive). Therefore tests must enqueue **during** the first tool:

```python
class ListingTool:
    def __init__(self, session_ref):
        self.session_ref = session_ref
        self.runs = 0
    def run(self, args, context):
        self.runs += 1
        if self.runs == 1:
            self.session_ref[0].steer("change plan")
        return ToolResult(ok=True, text="listed")
```

- [ ] **Step 2: Run test — expect FAIL**

Run: `/opt/homebrew/bin/uv run python -m pytest tests/unit/test_steering.py::test_steer_skips_remaining_tools_and_injects_user_message -v`

- [ ] **Step 3: Implement session API and loop**

Add to `AgentSession`:

```python
# fields
_steering: queue.Queue[str]  # maxsize=STEERING_QUEUE_MAX
_turn_active: bool = False
_permission_pending: bool = False
steer_notifier: Callable[[str], None] | None = None

def steer(self, text: str) -> bool:
    cleaned = text.strip()
    if not cleaned or not self._turn_active or self._permission_pending:
        return False
    try:
        self._steering.put_nowait(cleaned)
        return True
    except queue.Full:
        return False

def begin_permission_wait(self) -> None:
    self._permission_pending = True

def end_permission_wait(self) -> None:
    self._permission_pending = False
```

Change `submit` tool loop to:

```python
self._turn_active = True
try:
    for _round_index in range(...):
        model_response = ...
        assistant_text = self._record_assistant_message(model_response)
        if not model_response.tool_calls:
            return self._finish_response(assistant_text)
        calls = list(model_response.tool_calls)
        for index, call in enumerate(calls):
            self._execute_tool_call(call, ...)
            steered = self._drain_one_steering()
            if steered is not None:
                skipped = len(calls) - index - 1
                for skipped_call in calls[index + 1:]:
                    self._record_skipped_tool(skipped_call)
                self._apply_steering(steered, skipped=skipped)
                break  # rebuild model_messages and continue outer round
        model_messages = self._budgeted_model_messages(...)
finally:
    self._turn_active = False
    # clear any leftover queue items for this turn or leave for Continue — spec: clear on turn end
    self._clear_steering_queue()
```

`_apply_steering`:

```python
def _apply_steering(self, text: str, *, skipped: int) -> None:
    self._write_transcript("steered", {"skipped": skipped, "chars": len(text), "text": text[:200]})
    if self.steer_notifier is not None:
        self.steer_notifier(format_steering_ack(skipped, text))
    bounded = self._bound_text(text, self.config.session.model_input_char_limit)
    self.messages.append(Message(role="user", content=bounded))
    self._write_transcript("user_message", {"text": bounded, "media": [], "steering": True})
    self._compact_messages_if_needed()
```

`_record_skipped_tool`: append tool message with `SKIPPED_TOOL_RESULT`, transcript `tool_result` with `ok=False`, `error_type="steered_skip"` (or `ok=True` with skip text — **lock: ok=False, error_type="steered_skip", text=SKIPPED_TOOL_RESULT**).

Wire permission wait: wrap `PermissionPolicy.decide` path so console/weixin prompters call `session.begin_permission_wait()` / `end_permission_wait()` around `confirm()`. Cleanest: pass optional callbacks into prompter or set flags in `AgentSession._execute_tool_call` around `policy.decide`:

```python
self._permission_pending = True
try:
    decision = policy.decide(...)
finally:
    self._permission_pending = False
```

This matches Approach C without changing every prompter.

- [ ] **Step 4: Run steering + session tests — expect PASS**

Run: `/opt/homebrew/bin/uv run python -m pytest tests/unit/test_steering.py tests/unit/test_session.py -v`

- [ ] **Step 5: Commit if requested**

---

### Task 3: Console status for `steered`

**Files:**
- Modify: `src/colibri/console.py`
- Modify: `tests/unit/test_console.py`

- [ ] **Step 1: Failing test**

```python
def test_status_transcript_emits_steered_line():
    status = ConsoleStatusWriter(enabled=True, stream=io.StringIO())
    sink = StatusTranscript(transcript=None, status=status)
    sink.write("steered", {"skipped": 2, "chars": 18})
    assert "[colibri] steered skipped=2 chars=18" in status.stream.getvalue()
```

- [ ] **Step 2: Implement branch in `StatusTranscript._write_status`**

```python
elif event_type == "steered":
    self.status.write("steered", skipped=payload.get("skipped"), chars=payload.get("chars"))
```

- [ ] **Step 3: Run `tests/unit/test_console.py` — PASS**

---

### Task 4: Gateway Weixin mid-turn steer + ack

**Files:**
- Modify: `src/colibri/gateway.py`
- Modify: `src/colibri/channels/weixin.py` (only if helper needed)
- Modify: `tests/unit/test_channels.py` and/or new gateway tests in `tests/unit/test_gateway_process.py` / add `tests/unit/test_gateway_steering.py`

- [ ] **Step 1: Failing test — active turn routes to steer**

Build a fake channel + `GatewayRunner` (or unit-test `handle_message` with a stub session cache). Pattern:

```python
def test_handle_message_steers_when_turn_active():
    # session.turn_active True; steer returns True
    # handle_message returns "" or a fixed ack placeholder and must NOT call submit
    ...
```

Because `handle_message` today always `submit`s and returns final text to Weixin worker (which sends that as the reply), mid-turn steer must:

1. Call `session.steer(text)`.
2. If True: return **empty string** (or a sentinel) and rely on `steer_notifier` to send the Approach-2 ack immediately — **worker must not send a duplicate final reply for this inbound**.
3. If False (idle): normal `submit` and return `response.text`.

Update Weixin worker (`weixin.py` around handler reply): if handler returns `""`, **do not** call `send_text` for the reply (notifier already sent ack). Document this contract.

```python
# gateway.handle_message
if session.is_turn_active():  # public property
    if session.steer(message.text):
        return ""
# else fall through to submit
session.steer_notifier = lambda ack, ch=channel, recipient=message.sender_id: ch.send_text(recipient, ack)
response = session.submit(...)
return response.text
```

Set `steer_notifier` when creating/getting the session for this message (each handle_message), so ack uses the current recipient.

**Race:** single Weixin worker means `submit` blocks the worker; a second message cannot be handled by the same worker until submit returns — **this breaks Weixin steering** unless receive loop can deliver into `steer()` without going through the worker.

**Required plumbing (minimal item-1):**

In `WeixinChannel.run` receive loop, when a message is not a permission waiter reply:

1. If gateway exposes `try_steer(sender_id, text) -> bool` and it returns True, **do not** enqueue work; optionally nothing else (ack comes from notifier inside session on the worker thread after current tool).
2. Else `_publish_work` as today.

Implement `GatewayRunner.try_steer(channel_name, sender_id, text) -> bool` that looks up session by key and calls `session.steer(text)`.

Wire channel to call this — cleanest: pass `try_steer` on `ChannelContext` or set an attribute on the channel before `run`.

```python
# ChannelContext addition
try_steer: Callable[[str, str], bool] | None = None  # (sender_id, text) -> bool
```

Receive loop:

```python
if self._deliver_text_waiter(message):
    continue
if context.try_steer is not None and message.text.strip():
    if context.try_steer(message.sender_id, message.text):
        continue
self._publish_work(message)
```

Notifier still runs on the worker thread inside `submit` when the queue is drained after a tool — correct.

- [ ] **Step 2: Implement ChannelContext.try_steer + Weixin receive branch + GatewayRunner.try_steer + steer_notifier on submit path**

- [ ] **Step 3: Tests for try_steer true skips queue; ack formatter used by notifier**

- [ ] **Step 4: Run channel + gateway related tests — PASS**

---

### Task 5: Concurrent REPL steering (Python)

**Files:**
- Modify: `src/colibri/cli.py`
- Modify: `src/colibri/repl_input.py` (add non-blocking or background line pump if needed)
- Modify: `tests/unit/test_cli.py`

- [ ] **Step 1: Failing test with fake input pump**

```python
def test_repl_forwards_lines_to_steer_during_submit(monkeypatch):
    # Inject a session whose submit blocks until steer is called, and an input source
    # that yields a steering line while submit runs
    ...
```

Implementation approach (CM0-friendly, no extra prompt while busy):

```python
# During submit in _run_repl:
stop = threading.Event()

def _pump():
    while not stop.is_set():
        line = try_read_repl_line_nonblocking(...)  # or read with short timeout
        if line is None:
            continue
        if not session.steer(line):
            if session.is_permission_pending():
                status.write("permission_pending")
            # else ignore (turn ended / queue full)

thread = threading.Thread(target=_pump, daemon=True)
thread.start()
try:
    status.write("thinking")
    print(format_answer_for_console(session.submit(user_text).text, ...))
finally:
    stop.set()
    thread.join(timeout=1)
```

Add `try_read_repl_line` with short select timeout in `repl_input.py` for TTY; if not TTY, skip starting the pump (Weixin remains primary).

Permission Approach C: `steer` returns False while `_permission_pending`; pump may emit `[colibri] permission_pending` at most once per wait (debounce) to avoid spam.

- [ ] **Step 2: Implement pump + tests — PASS**

- [ ] **Step 3: Manual note in plan completion: on device, type a line during a long tool without waiting for `colibri>`**

---

### Task 6: Rust helpers + session parity

**Files:**
- Create: `colibri-rust/src/steering.rs`
- Modify: `colibri-rust/src/lib.rs` (`pub mod steering;`)
- Modify: `colibri-rust/src/session.rs`
- Modify: `colibri-rust/src/console.rs`
- Modify: `colibri-rust/tests/runtime.rs`
- Modify: `colibri-rust/tests/parity.rs` mappings if required

- [ ] **Step 1: Port `format_steering_ack` + `SKIPPED_TOOL_RESULT` with unit tests in `runtime.rs`**

```rust
pub const SKIPPED_TOOL_RESULT: &str = "Skipped due to queued user message.";
pub fn format_steering_ack(skipped: usize, steering_text: &str) -> String { ... }
```

Unicode: use the same scalar-char truncation as Python (`chars().take(20)`).

- [ ] **Step 2: Add `SteeringState` with `Mutex<VecDeque<String>>`, `turn_active`, `permission_pending`, `steer_notifier: Option<Arc<dyn Fn(String) + Send + Sync>>`**

- [ ] **Step 3: Mirror Python tool-batch poll/skip/inject in `submit_inner`**

- [ ] **Step 4: Status mapping for transcript event `steered`**

- [ ] **Step 5: `cargo test --manifest-path colibri-rust/Cargo.toml` — PASS**

---

### Task 7: Rust gateway + CLI parity

**Files:**
- Modify: `colibri-rust/src/gateway.rs`
- Modify: `colibri-rust/src/weixin.rs` / channel run loop equivalent
- Modify: `colibri-rust/src/cli.rs`
- Modify: `colibri-rust/src/repl_input.rs` if present

- [ ] **Step 1: `try_steer` on gateway; Weixin poll path calls it before enqueue**

- [ ] **Step 2: Set notifier to send Weixin text ack**

- [ ] **Step 3: REPL background pump during submit (TTY only), same status lines**

- [ ] **Step 4: Empty handler reply must not double-send (match Python worker contract)**

- [ ] **Step 5: `cargo test` + `/opt/homebrew/bin/uv run python -m pytest` — PASS**

---

### Task 8: Docs and design cross-links

**Files:**
- Modify: `docs/superpowers/specs/2026-07-10-colibri-steering-display-design.md` (fill queue bound = 4; transcript text bound = 200)
- Modify: `docs/README.md` (link plan)
- Modify: `README.zh-CN.md` / `README.md` — short note under gateway/REPL that mid-turn Weixin messages steer and local TTY can type while busy

- [x] **Step 1: Update spec §5 queue bound and transcript bound to match code**

- [x] **Step 2: Add plan link under Plans in `docs/README.md`**

- [x] **Step 3: User-facing one-liner in Chinese + English README**

- [x] **Step 4: Full test suite**

```bash
/opt/homebrew/bin/uv run python -m pytest
cargo test --manifest-path colibri-rust/Cargo.toml
```

---

## Spec coverage checklist

| Spec item | Task |
|-----------|------|
| Skip remaining tools + fixed skip string | 2, 6 |
| Status `[colibri] steered skipped=N` | 3, 6 |
| Weixin ack template + 20-char preview | 1, 4, 6 |
| Ack not in model context; full text injected | 2 |
| Permission wait blocks steering | 2, 5 |
| Concurrent REPL line → steer | 5, 7 |
| Idle message → normal submit | 4 (`try_steer` false) |
| Python/Rust parity | 6, 7 |
| No full MessageBus (minimal notify only) | 4 (notifier + try_steer) |
| No TUI / no buttons | honored throughout |

## Placeholder scan

Queue max **4**, preview **20**, transcript text bound **200**, skip `error_type=steered_skip` — all locked above.

---

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-10-colibri-steering.md`.

**Two execution options:**

1. **Subagent-Driven (recommended)** — fresh subagent per task, review between tasks  
2. **Inline Execution** — execute tasks in this session with checkpoints  

Which approach?
