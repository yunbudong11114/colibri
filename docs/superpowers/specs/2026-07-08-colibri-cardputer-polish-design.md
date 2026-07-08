# Colibri CardputerZero Polish Design

Date: 2026-07-08
Status: Implemented
Milestone: 9
Scope: Console status, idle timeout, diagnostics, and systemd examples

## 1. Goal

Milestone 9 improves Colibri's operation on small, headless CardputerZero-class Linux servers. It focuses on plain terminal visibility, automatic idle exit, lightweight diagnostics, and deployment examples.

After this milestone, Colibri should:

- print optional one-line console status messages suitable for SSH, serial consoles, and small screens,
- exit the REPL after a configured idle timeout,
- provide a dependency-free diagnostics command,
- ship a conservative systemd service example,
- keep voice wake out of this milestone.

## 2. Non-Goals

Milestone 9 must not implement:

- a full-screen TUI,
- colors, cursor movement, progress bars, or spinners,
- background daemon mode,
- HTTP, socket, or serial server modes,
- voice wake or audio dependencies,
- automatic systemd installation.

## 3. Console Status

Console status is for human and journal visibility, not for machine protocol output.

Rules:

- Status lines go to `stderr`.
- Model answers stay on `stdout`.
- Every status line is one plain ASCII line.
- Prefix every line with `[colibri]`.
- Do not use colors, emoji, cursor movement, or terminal control codes.
- Do not print API keys, full prompts, full tool results, or full model messages.
- Allow users to disable status with `console.status = false`.

Events:

```text
[colibri] ready model=ZHIPU/GLM-5.2
[colibri] thinking
[colibri] memory topics=devices,preferences
[colibri] skill skills=release
[colibri] tool files.read wait_permission
[colibri] tool files.read ok chars=1284
[colibri] compact mode=model dropped=4 summary_chars=2310
[colibri] model_error type=ModelError
[colibri] idle_exit seconds=300
```

Implementation approach:

- Add a small `ConsoleStatusWriter`.
- Wrap transcript event writes in the CLI so session events can produce status without making `AgentSession` depend on the console.
- Print `ready` after CLI session construction.
- Print `thinking` before a model turn starts.
- Convert selected transcript events into status lines.

## 4. Idle Timeout

Idle timeout applies to interactive `repl`, not to single `ask`.

Rules:

- Use existing `session.idle_exit_seconds`.
- `idle_exit_seconds <= 0` disables idle exit.
- If no user input arrives before the timeout, print `idle_exit` status and exit `0`.
- Close transcript through the existing `finally` path.
- Do not pair REPL idle timeout with a `Restart=always` systemd service.

Implementation approach:

- Add a timeout-aware input helper for REPL.
- Prefer `select.select()` on POSIX stdin when available.
- On interactive TTYs, use a tiny raw-mode line editor that reads UTF-8 characters, handles backspace, and redraws the full prompt line.
- Clear and redraw the whole prompt line on edits so wide CJK characters cannot leave terminal ghost cells.
- Treat terminal escape sequences as controls, never as printable input. In particular, consume arrow-key sequences such as `ESC [ A` and `ESC [ B` so the terminal does not interpret replayed escape bytes as cursor movement during redraw.
- Keep a small in-memory REPL history for the current process. Up/down arrows navigate submitted non-empty prompts; history is not persisted to disk in this milestone.
- Avoid Python's built-in `input()` and readline/libedit by default because CJK input and deletion can leave ghost characters or submit an empty buffer on some macOS terminals.
- In raw TTY mode, read bytes with unbuffered `os.read(fd, 1)` after `select.select()` on the same fd. Do not mix fd-level readiness with `TextIO` or `BufferedReader` reads, because Python buffering can hold the remaining bytes of a UTF-8 CJK character and delay display until the next key press.
- Fall back to blocking `stdin.readline()` if the stream is not selectable.
- Keep tests isolated by allowing a fake input function.

## 5. Low-Memory Diagnostics

Diagnostics is a CLI command:

```bash
colibri diagnostics
```

It prints plain key-value lines to `stdout`.

Include:

- Python version,
- platform,
- provider and model,
- enabled tools,
- memory root and whether it exists,
- number of configured skill dirs,
- number of discovered local skills,
- transcript enabled,
- RSS memory when available,
- configured context limits.

RSS rules:

- On Linux, read `/proc/self/status` and parse `VmRSS`.
- Else, use `resource.getrusage()` when available.
- If unsupported, print `rss_kb=unknown`.

Example:

```text
colibri diagnostics
python=3.12.9 platform=linux
provider=openai_compatible model=ZHIPU/GLM-5.2
tools=shell,files,memory,skills
memory_root=/home/cardputer/.colibri/memory exists=true
skills_dirs=1 skills_found=3
transcript=true rss_kb=28672
recent_message_limit=16 compact_trigger_chars=36000 summary_max_chars=6000
```

## 6. Systemd Example

Ship an example service file at:

```text
deploy/systemd/colibri-repl.service
```

Rules:

- It is an example only; Colibri does not install it.
- Use `Restart=no` for REPL to avoid idle-timeout restart loops.
- Bind it to a concrete TTY if users want interactive REPL under systemd.
- Mention that `Restart=always` is for future daemon/server modes, not REPL.
- Use an environment file for API keys.

Example properties:

```ini
[Service]
Type=simple
Environment=COLIBRI_HOME=/var/lib/colibri
EnvironmentFile=-/etc/colibri/colibri.env
WorkingDirectory=/var/lib/colibri
ExecStart=/usr/bin/python -m colibri.cli --config /etc/colibri/agent.toml repl
Restart=no
```

## 7. Milestone Split

Voice wake is split into Milestone 10.

Milestone 9 implements:

1. console status,
2. idle timeout,
3. diagnostics,
4. systemd service example.

Milestone 10 covers only a future voice wake design spike and must not add runtime code unless explicitly approved later.

## 8. Testing

Required tests:

- status writer prints one-line `[colibri]` messages to `stderr`,
- status can be disabled,
- `ask` keeps answer on `stdout` while status goes to `stderr`,
- transcript events map to concise status lines,
- REPL exits on idle timeout with code `0`,
- `diagnostics` prints expected key-value fields,
- RSS helper returns an integer or `None`,
- config can override `console.status`,
- all tests run with `uv run python -m pytest`.
