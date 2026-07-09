# Colibri Lightweight Python Agent Design

Date: 2026-07-01
Status: Design approved for route 1
Target device: M5Stack CardputerZero, Raspberry Pi Compute Module 0 class Linux device, about 512MB RAM

## 1. Goal

Build a personal lightweight AI agent runtime in Python for CardputerZero.

The first version should run as a single local process, call a cloud LLM API, execute local tools, load user-configured skills, optionally connect to MCP servers, keep small file-based memory, and preserve Claude Code's core agent loop without inheriting its desktop-scale complexity.

The agent is for personal use, so the default posture can be more permissive than a team product. Even so, shell, file mutation, network, GPIO, and MCP calls still need clear permission boundaries because the device can touch real local files and hardware.

## 2. Non-Goals

- Do not run a large local LLM on CardputerZero.
- Do not build a multi-agent swarm or subagent system in the first version.
- Do not keep many MCP servers resident in memory.
- Do not implement browser automation, IDE/LSP integration, notebook support, or remote collaboration.
- Do not copy Claude Code's full transcript, hook, feature flag, marketplace, or streaming tool executor complexity.
- Do not require a desktop UI. The primary interface is CLI/TUI-friendly and later can integrate with the small screen, keyboard, microphone, speaker, and systemd.
- Do not make any core milestone depend on a graphical environment. Colibri must remain runnable on headless Linux servers over plain SSH using CLI/stdin/stdout.

## 3. Hardware and Runtime Constraints

CardputerZero should be treated as a low-memory Linux appliance:

- CPU: Raspberry Pi Zero 2 / CM0 class Arm Cortex-A53, about 1GHz.
- RAM: about 512MB, shared by OS, Python, network, audio, and any tools.
- Storage: microSD, so writes should be batched and logs rotated.
- Display: very small screen; status text should be short.
- Input: hardware keyboard first, voice later.
- Audio: microphone and speaker are useful but should be optional modules.
- Connectivity: Wi-Fi/Ethernet available, but cloud API latency and offline states are normal.
- Expansion: GPIO/I2C/SPI/UART can control real hardware, so hardware tools need permission labels.
- Headless operation: the same runtime must work on pure Linux servers without a display server, browser, desktop session, audio device, or hardware screen. Visual, audio, or CardputerZero-specific integrations must be optional layers around the CLI runtime.

The target resident memory for the first version should be conservative:

| Component | Target |
|---|---:|
| Python process baseline | 35-70MB RSS |
| Loaded config, skills, memory index | less than 10MB |
| Conversation state | less than 20MB |
| Active tool output buffers | less than 8MB |
| Optional MCP child process | one at a time by default |
| Voice wake stack | disabled by default |

The agent should prefer low dependency count, lazy imports, bounded buffers, and simple file formats.

## 4. Claude Code Ideas to Keep and Cut

The design borrows concepts from Claude Code but intentionally reduces them.

| Claude Code concept | Lightweight Python version |
|---|---|
| `QueryEngine` conversation shell | `AgentSession` with messages, summary, usage, transcript, idle timeout |
| Recursive `query.ts` agent loop | Bounded `while tool_round < max_tool_rounds` |
| Tool interface with schema and permissions | Keep `ToolSpec`, JSON schema, permission tags, result budgets |
| Streaming tool executor | Cut in v1; execute tools after one full model response |
| Tool result budgeting | Keep strict per-tool and per-turn truncation |
| Microcompact/autocompact/reactive compact | Replace with recent N messages + summary compact |
| `MEMORY.md` plus topic files | Keep as first-class memory system |
| Skills | Keep local `skills/<name>/SKILL.md` with optional scripts |
| MCP | Keep bridge/client, but start servers lazily |
| Transcript | Keep compact JSONL logs with rotation |
| Hooks, feature gates, subagents | Cut in v1 |

The essential loop remains:

```text
user input
  -> build prompt context
  -> call model
  -> if assistant response has tool calls:
       validate tool args
       check permissions
       execute tools
       append tool results
       continue next model round
     else:
       return assistant text
```

## 5. Package Layout

The proposed repository layout:

```text
colibri/
  pyproject.toml
  README.md
  configs/
    agent.example.toml
  docs/
    superpowers/specs/
  src/
    colibri/
      __init__.py
      cli.py
      config.py
      session.py
      loop.py
      messages.py
      model/
        __init__.py
        base.py
        openai_compatible.py
      tools/
        __init__.py
        base.py
        registry.py
        permissions.py
        builtin/
          shell.py
          files.py
          http.py
          memory.py
          gpio.py
      skills/
        __init__.py
        loader.py
        skill_tool.py
      mcp/
        __init__.py
        client.py
        bridge.py
      memory/
        __init__.py
        store.py
        recall.py
        compact.py
      ui/
        __init__.py
        console.py
        status.py
      util/
        limits.py
        jsonl.py
        paths.py
        text.py
  tests/
    unit/
```

User data should live outside the package:

```text
~/.colibri/
  config.toml
  skills/
    weather/
      SKILL.md
      skill.toml
      run.py
    home/
      SKILL.md
  memory/
    MEMORY.md
    topics/
      preferences.md
      devices.md
      routines.md
  transcripts/
    2026-07-01.jsonl
  cache/
```

This split keeps code immutable and user customization easy to back up.

## 6. Configuration

Use TOML because it is human-editable and available in Python 3.11 via `tomllib`.

When the CLI starts without `--config`, Colibri should try `~/.colibri/config.toml` first and fall back to built-in defaults if the file does not exist. An explicit `--config` path always wins over the default user config file.

Example:

```toml
[model]
provider = "openai_compatible"
base_url = "https://api.openai.com/v1"
model = "gpt-4.1-mini"
api_key = ""
timeout_seconds = 60
max_output_tokens = 16384

[session]
max_tool_rounds = 32
recent_message_limit = 96
compact_trigger_chars = 24000
summary_max_chars = 24000
idle_exit_enabled = false
idle_exit_seconds = 300
transcript = true

[tools]
enabled = ["shell", "files", "web", "memory", "skills", "mcp"]
default_permission = "allow_read_confirm_write"
max_result_chars = 32000
max_shell_seconds = 30

[shell]
deny = ["rm", "shutdown", "reboot", "mkfs", "dd", "sudo"]

[files]
roots = ["~/notes", "~/.colibri", "/tmp"]

[skills]
dirs = ["~/.colibri/skills"]
max_loaded = 3
max_instruction_chars = 8000

[console]
status = true

[gateway]
enabled_channels = ["weixin"]
max_sessions = 4
session_idle_seconds = 600

[channels.weixin]
enabled = false
token = ""
base_url = "https://ilinkai.weixin.qq.com/"
allow_from = []
poll_timeout_seconds = 35
auth_timeout_seconds = 300

[web_search]
engine = "baidu"
api_key = ""
endpoint = "https://qianfan.baidubce.com/v2/ai_search/web_search"
max_results = 10
timeout_seconds = 10

[mcp]
enabled = true
startup = "lazy"
max_active_servers = 1

[[mcp.servers]]
name = "home"
transport = "stdio"
command = "python"
args = ["-m", "home_mcp_server"]
permission = "confirm"
idle_ttl_seconds = 60
```

On CardputerZero, the default config should favor small outputs and short timeouts. Desktop development can override these limits.

## 7. Core Components

### 7.1 `AgentSession`

`AgentSession` owns the runtime state for one conversation:

- `messages`: compacted list of model-facing messages.
- `summary`: rolling compact summary of older conversation.
- `usage`: approximate token/character usage.
- `loaded_skills`: skill metadata chosen for the current turn.
- `memory_refs`: memory files injected into the current turn.
- `transcript_writer`: optional JSONL recorder.
- `started_at` and `last_activity_at`: idle timeout support.

It should expose:

```python
class AgentSession:
    def submit(self, user_text: str) -> "AgentResponse": ...
    def reset(self) -> None: ...
    def compact_if_needed(self) -> None: ...
    def close(self) -> None: ...
```

The session should not know how each tool works. It only coordinates model calls, context shaping, tool execution, compacting, and logs.

### 7.2 Agent Loop

The loop is deliberately bounded:

```python
for round_index in range(config.session.max_tool_rounds):
    context = context_builder.build(session, user_input)
    assistant = model.complete(context, tool_specs)
    session.append_assistant(assistant)

    tool_calls = assistant.tool_calls
    if not tool_calls:
        return final_text(assistant)

    for call in tool_calls:
        result = tool_runner.run(call)
        session.append_tool_result(call.id, result)

    session.compact_if_needed()

return round_limit_report(max_tool_rounds, recent_tool_results)
```

First version executes tool calls sequentially. A later version can parallelize read-only tools, but sequential execution is simpler, lower memory, and easier to reason about on a small device.

When the loop reaches `max_tool_rounds`, the user-facing response should include a compact summary of what happened instead of only a fixed error string. The report should mention the configured limit, recent tool names, recent tool result snippets, and that the user can continue or increase `session.max_tool_rounds`.

### 7.3 Model Adapter

Use an OpenAI-compatible adapter first because many providers support that shape. Keep the interface narrow:

```python
class ModelClient:
    def complete(
        self,
        messages: list[Message],
        tools: list[ToolSpec],
        system: str,
        limits: ModelLimits,
    ) -> ModelResponse:
        ...
```

The adapter should normalize:

- assistant text
- tool calls
- refusal/error messages
- usage if available
- timeout and retry behavior

Streaming output is optional in v1. The small screen benefits from streaming, but the architecture should first be correct without it.

## 8. Tool System

Tools use a schema-driven interface similar in spirit to Claude Code:

```python
@dataclass
class ToolSpec:
    name: str
    description: str
    input_schema: dict
    permission: PermissionClass
    read_only: bool
    destructive: bool
    max_result_chars: int

class Tool:
    spec: ToolSpec

    def validate(self, args: dict) -> None: ...
    def check_permission(self, args: dict, ctx: ToolContext) -> PermissionDecision: ...
    def run(self, args: dict, ctx: ToolContext) -> ToolResult: ...
```

`ToolResult` should contain:

- `ok: bool`
- `text: str`
- `metadata: dict`
- `truncated: bool`
- `error_type: str | None`

Built-in v1 tools:

| Tool | Purpose | Default permission |
|---|---|---|
| `shell.run` | Run whitelisted commands | confirm unless allowlisted |
| `files.read` | Read files under allowed roots | allow |
| `files.write` | Create/overwrite files under allowed roots | confirm |
| `files.list` | List directory entries | allow |
| `http.fetch` | Simple HTTP GET/POST | confirm by default |
| `memory.read` | Read memory index/topic files | allow |
| `memory.write` | Append or update topic memory | confirm |
| `skill.run` | Invoke configured local skill | depends on skill |
| `mcp.call` | Invoke MCP tool | confirm by server/tool policy |
| `gpio.call` | Placeholder for hardware actions | confirm |

All tool output must be capped. If a command produces too much output, keep the head and tail plus a truncation note. Large full outputs can be written to a temp file only when explicitly configured.

## 9. Permission Model

The agent is personal-use, so permissions should be lightweight but visible.

Permission classes:

- `allow`: run without asking.
- `confirm`: ask user on keyboard/TUI before running.
- `deny`: block.
- `allow_read_confirm_write`: allow read-only actions; confirm writes or hardware/network side effects.

Risk labels:

- `read_only`
- `writes_files`
- `runs_process`
- `network`
- `hardware`
- `secret_access`
- `destructive`

For CardputerZero, confirmation prompts must be short:

```text
Run shell?
rg "foo" ~/notes
[Y]es [n]o [a]lways
```

The "always" choice should be session-scoped by default, not permanently written, unless config explicitly allows persistent approvals.

## 10. Skill System

Skills are primarily local directories. Colibri v1 should not implement skill installation, marketplace discovery, remote downloads, package registries, or plugin distribution.

Colibri may also ship a very small set of built-in guidance skills inside the project. Built-in skills are not read from `skills.dirs`, do not install anything, and do not expose commands unless explicitly designed later. They exist to teach users how to create or structure local Colibri skills.

```text
skills/<name>/
  SKILL.md
  skill.toml        # optional
  run.py            # optional
  scripts/...       # optional
```

`SKILL.md` is the instruction source. `skill.toml` describes metadata and callable scripts:

```toml
name = "weather"
description = "Check local weather and summarize it."
permission = "network"

[[commands]]
name = "forecast"
description = "Fetch forecast for a city"
command = "python"
args = ["run.py"]
input_schema = { type = "object" }
```

Skill loading should be cheap:

1. At startup, seed project built-in skill metadata, then scan configured local skill directories for names, descriptions, `SKILL.md` paths, and command metadata.
2. Keep the long-lived skill index metadata-only; do not retain every full `SKILL.md` body in memory.
3. Do not load every full `SKILL.md` into every prompt.
4. On each user turn, select relevant skills by simple keyword scoring first.
5. Read full local `SKILL.md` content only for selected skills; built-in selected skills read their bundled instruction text from the project.
6. Inject only the top bounded relevant skill instructions.
7. Expose scripts as `skill.run` subcommands only when their skill is enabled.

The first built-in skill is `create-colibri-skill`. It triggers on requests about creating, writing, adding, or designing Colibri skills, and guides the assistant to create a local `~/.colibri/skills/<name>/SKILL.md` skill with optional `skill.toml` command metadata.

This avoids Claude Code's richer but heavier skill machinery while preserving the user's ability to configure local capabilities. Users can add skills by placing files in configured directories; Colibri will not install, update, or fetch remote skill packages in v1.

## 11. MCP Support

MCP should be supported as a bridge, not as many resident servers.

First version:

- Read MCP server definitions from config.
- Support stdio transport first.
- Start MCP server lazily when a tool/resource is needed.
- Stop idle server after `idle_ttl_seconds`.
- Limit active MCP servers to 1 by default.
- Cache tool schemas for the session.
- Treat MCP tools as permissioned external tools.

Optional second transport:

- HTTP/SSE bridge to a home server or desktop.

This lets CardputerZero act as the personal agent terminal while heavier integrations run elsewhere.

## 12. Memory

Use file-based memory:

```text
memory/
  MEMORY.md
  topics/
    preferences.md
    devices.md
    routines.md
```

`MEMORY.md` is the index:

```markdown
# Memory Index

- preferences: User preferences, tone, recurring constraints.
- devices: Home devices, hostnames, GPIO wiring, network notes.
- routines: Common workflows and reminders.
```

Recall algorithm v1:

1. Load `MEMORY.md` index.
2. Score topic names and descriptions with keyword overlap against current user input and recent messages.
3. Read top 1-3 topic files within a strict character budget.
4. Inject memory as a separate context block.

Memory writes should not silently rewrite large files. Prefer append-style proposed updates:

```text
memory.write(topic="devices", mode="append", text="...")
```

The user confirms memory writes by default.

## 13. Context Management and Compacting

CardputerZero should not keep a desktop-scale transcript in RAM. The model context builder should combine:

1. Static system prompt.
2. Device/runtime constraints.
3. Current rolling summary.
4. Recent conversation messages.
5. Relevant memory snippets.
6. Relevant skill instructions.
7. Tool specs.
8. Current user input.

The first compact strategy:

- Keep the last `recent_message_limit` messages.
- Maintain a rolling `summary` for older messages.
- Trigger compact when estimated context chars exceed `compact_trigger_chars`.
- Summarize old messages into at most `summary_max_chars`.
- Drop or shrink old tool results aggressively.

Tool result budget:

- Per tool result default: 12,000 chars.
- Per model turn total tool results: 24,000 chars.
- Old tool results in compacted history become one-line summaries.

Summary compact should be model-assisted when network is available:

```text
Summarize the older conversation for continuing an agent session.
Preserve user goals, decisions, file paths, tool results, open tasks,
memory changes, device constraints, and unresolved errors.
```

Colibri should use a Claude Code style compact prompt rather than an unconstrained one-line summary. The model is asked for plain text containing an `<analysis>` scratchpad and a `<summary>` section. Colibri strips the analysis block, converts the summary wrapper into a readable `Summary:` header, and then bounds the stored summary. This is a prompt-level format constraint, not a JSON schema requirement.

Fallback compact when model compact fails:

- Keep user requests and assistant final decisions.
- Replace tool outputs with metadata summaries.
- Keep file paths and command names.

This is less capable than Claude Code's multi-layer compaction, but it is robust and cheap.

## 14. Transcript and Logs

Use JSONL with rotation:

```json
{"type":"user","time":"...","text":"..."}
{"type":"assistant","time":"...","text":"...","tool_calls":[...]}
{"type":"tool_result","time":"...","tool":"shell.run","ok":true,"chars":1200}
```

Rules:

- Store transcripts on disk, not fully in memory.
- Rotate by day or file size.
- Redact API keys and obvious secrets.
- Do not store huge tool output inline unless configured.

## 15. CLI and Device UX

Initial interface:

```bash
colibri
colibri ask "turn off the desk light"
colibri repl
colibri skill list
colibri memory search "wifi"
```

Interactive REPL should show compact status:

```text
colibri> check my notes for the camera pinout
thinking...
tool files.read ~/.colibri/memory/topics/devices.md ok
...
```

Later CardputerZero UI:

- Small status line for model/tool/network state.
- Keyboard confirmation prompts.
- Idle timeout exit to save battery.
- Optional systemd service for wake mode.

## 16. Voice Wake and Idle Exit

Voice is not part of v1 implementation, but the architecture should reserve hooks:

- `WakeController`: waits for keyboard, button, or wake word.
- `InputProvider`: text input now, voice transcript later.
- `OutputProvider`: console now, speaker/TTS later.
- `IdleManager`: exits or sleeps after no activity.

Recommended behavior:

- Default process exits after `idle_exit_seconds`.
- Wake-word service, if used, should be a separate tiny process.
- The full agent starts only after wake, then exits when idle.

This avoids burning RAM and battery all day.

## 17. Error Handling

Common errors should degrade gracefully:

- Model timeout: show short error, keep session state.
- Network offline: allow local tools and memory, skip cloud call.
- Tool denied: append a tool result explaining denial.
- Tool timeout: kill subprocess, return partial output if available.
- MCP server failed: mark server unavailable for this turn.
- Compact failed: use fallback compact and lower recent message limit.
- Skill parse error: skip that skill and show warning in diagnostics.

The model should see structured tool errors so it can recover instead of repeating the same call blindly.

## 18. Performance Strategy

Primary bottlenecks:

- RAM pressure from Python imports, conversation history, tool outputs, MCP child processes.
- CPU and latency from model JSON parsing, subprocess output, and compacting.
- microSD wear from transcripts and frequent memory writes.
- Battery drain from Wi-Fi, audio wake, and long-running child processes.

Mitigations:

- Lazy import optional modules.
- Start MCP servers only on demand.
- Execute tools sequentially in v1.
- Cap all strings entering the session.
- Store transcripts on disk with rotation.
- Keep only recent messages in RAM.
- Use keyword memory/skill retrieval before considering model-based retrieval.
- Disable voice by default.
- Use timeouts for shell, HTTP, MCP, and model calls.
- Prefer Python standard library where reasonable.

Recommended dependency posture for v1:

- Required runtime dependencies: none beyond the Python standard library if practical.
- Built in-house for v1: OpenAI-compatible HTTP calls through `urllib.request`, small tool schema validator, JSONL logging, and minimal JSON-RPC stdio MCP client.
- Optional later: `rich` for nicer console output, `httpx` for streaming/proxy ergonomics, official MCP SDK if measured memory use is acceptable.
- Avoid: vector databases, heavy TUI frameworks, always-on async service frameworks, large audio ML packages, and broad validation frameworks on the device.

## 19. Testing Plan

Unit tests:

- Config loading and path expansion.
- Tool schema validation.
- Shell allow/deny matching.
- File root permission checks.
- Skill scanning and relevance scoring.
- Memory index parsing and topic recall.
- Tool result truncation.
- Compact fallback behavior.
- Agent loop stops at max tool rounds.

Integration tests:

- Fake model asks for a file read tool, then returns final answer.
- Fake model asks for denied shell command and recovers.
- Fake skill with `SKILL.md` is selected and invoked.
- Fake MCP server schema is loaded lazily.
- Transcript JSONL is written and rotated.

Device tests on CardputerZero:

- Cold start time.
- Idle RSS after startup.
- RSS after 10-turn conversation.
- Shell/file tool latency.
- Wi-Fi model call behavior.
- microSD write volume for transcripts.
- Idle timeout exit.

## 20. Current Implementation Roadmap

This roadmap reflects the actual implementation sequence as of 2026-07-07. Earlier drafts grouped skills, memory, permissions, transcripts, and compacting differently; the project now treats each milestone as one complete, testable slice.

Milestone 1: local CLI skeleton

- `pyproject.toml`
- config loader
- console REPL
- message types
- fake model for tests
- basic `AgentSession`

Status: complete.

Primary plan/spec:

- `docs/superpowers/plans/2026-07-01-colibri-milestone-1.md`

Milestone 2: real model adapter and minimum tool loop

- OpenAI-compatible model adapter
- model provider factory
- tool message serialization
- tool registry
- bounded agent loop
- read-only file tools: `files.list`, `files.read`
- allowlisted shell tool: `shell.run`
- result truncation

Status: complete.

Primary specs/plans:

- `docs/superpowers/specs/2026-07-06-colibri-openai-compatible-model-design.md`
- `docs/superpowers/plans/2026-07-06-colibri-openai-compatible-model.md`
- `docs/superpowers/specs/2026-07-06-colibri-minimum-tool-loop-design.md`
- `docs/superpowers/plans/2026-07-06-colibri-minimum-tool-loop.md`

Milestone 3: permissions and transcript logging

- permission policy for `allow`, `deny`, `confirm`, and `allow_read_confirm_write`
- headless stdin/stdout confirmation path
- session-scoped `always allow`
- JSONL transcript writer
- session event logging for messages, tool calls, permission decisions, tool results, model errors, and round limits

Status: complete.

Primary spec/plan:

- `docs/superpowers/specs/2026-07-06-colibri-permissions-transcript-design.md`
- `docs/superpowers/plans/2026-07-06-colibri-permissions-transcript.md`

Milestone 4: file-backed memory tools

- `[memory]` config
- `memory.list`
- `memory.read`
- `memory.search`
- `memory.write`
- permission confirmation for memory writes

Status: complete.

Primary spec/plan:

- `docs/superpowers/specs/2026-07-07-colibri-file-memory-tools-design.md`
- `docs/superpowers/plans/2026-07-07-colibri-file-memory-tools.md`

Milestone 5: memory recall injection

- load `MEMORY.md` index within strict character limits
- score topic names and descriptions against user input and recent messages
- read top 1-3 relevant topic files within a memory context budget
- inject selected memory as a separate context block before model calls
- record selected memory references in transcript payloads

Status: complete.

Primary spec/plan:

- `docs/superpowers/specs/2026-07-07-colibri-memory-recall-design.md`
- `docs/superpowers/plans/2026-07-07-colibri-memory-recall.md`

Milestone 6: context compacting and limits

- recent-message window refinements
- model-assisted summary compact with deterministic fallback
- fallback compact
- tool result budgeting refinements
- configurable context limits

Status: complete.

Primary spec/plan:

- `docs/superpowers/specs/2026-07-07-colibri-context-compacting-design.md`
- `docs/superpowers/plans/2026-07-07-colibri-context-compacting.md`

Milestone 7: local skills

- skill scanner
- skill instruction injection
- simple `skill.run`
- script permission integration
- local filesystem only; no skill install, marketplace, registry, or remote fetch

Status: complete.

Primary spec:

- `docs/superpowers/specs/2026-07-07-colibri-local-skills-design.md`

Milestone 8: MCP bridge

- config-defined stdio MCP server
- lazy startup
- tool schema cache
- `mcp.call`
- idle shutdown

Status: deferred. Do not implement in the current milestone sequence; revisit only if MCP becomes necessary for the target device workflows.

Milestone 9: CardputerZero polish

- small-screen friendly console status
- idle timeout
- low-memory diagnostics
- systemd service example

Status: complete.

Primary spec:

- `docs/superpowers/specs/2026-07-08-colibri-cardputer-polish-design.md`

Milestone 10: optional voice wake design spike

- external wake process boundary
- stdin/socket/local trigger options
- memory and dependency budget
- no runtime code unless explicitly approved later

Status: planned.

## 21. Implementation Defaults

Use these defaults for v1 so implementation does not stall on dependency or policy choices:

- Model HTTP client: use Python standard library `urllib.request` first. Add `httpx` later only if streaming, proxy support, or better timeout handling becomes necessary.
- Tool schema validation: implement a small local subset validator for `type`, `properties`, `required`, `enum`, `items`, and primitive scalar types. Avoid `jsonschema` in v1 to reduce dependency weight.
- MCP client: deferred. If revived later, prefer a minimal JSON-RPC stdio client for tool listing and tool calls, and evaluate the official MCP SDK only after measuring its RSS on CardputerZero.
- Default cloud model: configure an OpenAI-compatible model name in `config.toml`; ship the example with `gpt-4.1-mini` because it is a reasonable low-latency default, but do not hard-code it.
- Persistent permissions: do not persist "always allow" in v1. "Always" approvals are session-scoped only.
- Async model: keep v1 mostly synchronous. Use subprocess timeouts and simple blocking HTTP to reduce mental and runtime overhead.
- Packaging: use a standard `src/` layout and avoid runtime dependency on `uv`, Poetry, or Hatch on the device. Development tooling may use them, but the installed agent should run with plain Python.

## 22. Recommended v1 Cut

This section is the target v1 boundary, not the current implementation state. See Section 20 for the current milestone status.

The smallest useful v1 should include:

- CLI REPL.
- OpenAI-compatible cloud model call.
- Bounded agent loop.
- Shell read-ish commands with allowlist.
- File read/list/write under allowed roots.
- Local skills loaded from `~/.colibri/skills`.
- File memory from `~/.colibri/memory`.
- Recent N messages plus summary compact.
- JSONL transcript.
- Idle timeout.

MCP is intentionally deferred from the current v1 path. Revisit it only when a concrete workflow needs external MCP servers.
