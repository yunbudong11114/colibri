# Colibri Minimum Tool Loop Design

Date: 2026-07-06
Status: Approved by user direction
Milestone: 2B
Scope: Minimal bounded tool loop with read-only built-in tools

## 1. Goal

Extend the completed OpenAI-compatible model milestone into a minimal working agent loop.

After this milestone, `AgentSession.submit()` should be able to:

1. send user messages and tool schemas to the configured model,
2. receive model tool calls,
3. execute known built-in read-only tools through a registry,
4. append tool results to the conversation, and
5. call the model again until it returns final text or reaches `max_tool_rounds`.

This makes Colibri a real small agent while keeping the implementation safe enough for a CardputerZero-class Linux device.

## 2. Non-Goals

- Do not implement file writes.
- Do not implement network tools.
- Do not implement memory, skills, MCP, GPIO, or transcript logging.
- Do not implement persistent permission grants.
- Do not run arbitrary shell commands.
- Do not implement parallel tool execution.
- Do not implement JSON Schema validation beyond simple required-field/type checks needed by built-in tools.

## 3. Architecture

Keep four boundaries clear:

- `ModelClient`: calls a model and returns `ModelResponse`.
- `AgentSession`: owns messages and coordinates the bounded loop.
- `ToolRegistry`: exposes tool schemas and resolves tool names.
- Individual tools: validate arguments, enforce local safety rules, and return bounded results.

The model adapter must not execute tools. The tools must not call the model. `AgentSession` is the only coordinator.

```text
user input
  -> AgentSession appends user message
  -> AgentSession sends messages + tool schemas to ModelClient
  -> ModelClient returns text and/or tool_calls
  -> AgentSession executes tool_calls through ToolRegistry
  -> AgentSession appends tool result messages
  -> repeat until final text or max_tool_rounds
```

## 4. Message Shape

The existing `Message` dataclass should gain optional fields so model adapters can represent assistant tool calls and tool results without a second message hierarchy:

```python
@dataclass(frozen=True)
class Message:
    role: str
    content: str
    tool_call_id: str | None = None
    tool_calls: list[ToolCall] = field(default_factory=list)
```

Rules:

- User messages use `role="user"`.
- Assistant text messages use `role="assistant"`.
- Assistant tool-call messages use `role="assistant"` with `tool_calls`.
- Tool results use `role="tool"` and `tool_call_id`.

`OpenAICompatibleModelClient` should serialize these fields to the OpenAI-compatible chat message shape.

## 5. Tool Interfaces

Add `src/colibri/tools/base.py`:

```python
@dataclass(frozen=True)
class ToolSpec:
    name: str
    description: str
    input_schema: dict[str, Any]
    read_only: bool = True

@dataclass(frozen=True)
class ToolResult:
    ok: bool
    text: str
    error_type: str | None = None
    truncated: bool = False

class Tool(Protocol):
    spec: ToolSpec
    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult: ...
```

`ToolContext` should carry:

- `config: AgentConfig`
- `cwd: Path`

Tool output must be capped to `config.tools.max_result_chars`.

## 6. Registry

Add `ToolRegistry`:

```python
class ToolRegistry:
    def from_config(config: AgentConfig, cwd: Path | None = None) -> "ToolRegistry": ...
    def specs(self) -> list[dict]: ...
    def run(self, call: ToolCall, context: ToolContext) -> ToolResult: ...
```

Rules:

- Only include tools listed in `config.tools.enabled`.
- Unknown tool names return a failed `ToolResult`.
- Tool specs should use the OpenAI-compatible `{"type": "function", "function": ...}` shape.

## 7. Built-In Tools

### 7.1 `files.list`

Arguments:

```json
{"path": "string"}
```

Behavior:

- Path must be under one of `config.files.roots`.
- Return sorted child names with `/` suffix for directories.
- Do not recurse.
- Return failed `ToolResult` for missing paths, non-directories, or disallowed paths.

### 7.2 `files.read`

Arguments:

```json
{"path": "string"}
```

Behavior:

- Path must be under one of `config.files.roots`.
- Path must be a file.
- Read as UTF-8 with replacement for invalid bytes.
- Cap output to `config.tools.max_result_chars`.

### 7.3 `shell.run`

Arguments:

```json
{"command": "string"}
```

Behavior:

- Historical milestone behavior: only run commands whose first token or full command appears in `config.shell.allow`.
- Reject commands whose first token appears in `config.shell.deny`.
- Historical milestone behavior ran with `shell=False` using `shlex.split()`.
- Use `config.tools.max_shell_seconds` as timeout.
- Combine stdout and stderr in the result text.
- Cap output to `config.tools.max_result_chars`.

For this milestone, `shell.run` is still conservative and intended for read-like commands such as `ls`, `cat`, `sed`, `rg`, and `git status`.

Current Colibri behavior is defined by later milestones: `shell.allow` has been removed, ungranted shell commands prompt for approval, `shell.deny` remains the hard-deny list, and execution uses the platform shell after checking each unquoted compound command segment.

## 8. Session Loop

`AgentSession` should accept an optional `ToolRegistry`.

Default behavior should preserve old tests:

- If no registry is passed, build one from config.
- If tool execution is not needed, one model call should still work as before.

Loop behavior:

```python
for round_index in range(config.session.max_tool_rounds):
    response = model.complete(messages, registry.specs(), SYSTEM_PROMPT, limits)
    append assistant message
    if not response.tool_calls:
        return response text
    for call in response.tool_calls:
        result = registry.run(call, context)
        append tool result message
return a concise round-limit report with:

- the configured `max_tool_rounds`,
- the most recent tool call names,
- the most recent tool result summaries,
- a short note that the user can continue the task or raise `session.max_tool_rounds`.
```

The max-round message should be concise and deterministic for tests. It must not be a bare fixed string because that hides useful progress and makes debugging tool loops painful.

## 9. Testing

Required tests:

- Tool registry exposes enabled built-in tool schemas.
- Registry rejects unknown tools.
- `files.list` lists allowed directories and rejects disallowed paths.
- `files.read` reads allowed files and truncates large output.
- `shell.run` executes allowlisted commands, rejects denied commands, and times out slow commands.
- `AgentSession` executes a model-requested tool and sends the tool result into the next model call.
- `AgentSession` stops at `max_tool_rounds`.
- Existing fake model CLI behavior remains unchanged.
- OpenAI-compatible adapter serializes tool result messages.

No tests should call real model APIs.

## 10. Validation

Minimum validation:

```bash
uv run python -m pytest
uv run python -m colibri.cli ask "hello colibri"
```

Optional real model validation remains manual because it needs a user-provided API key:

```bash
export COLIBRI_API_KEY="..."
uv run python -m colibri.cli --config configs/glm.example.toml ask "list files in /tmp if you need a tool"
```

## 11. Future Work

After this milestone:

- add confirmation prompts for write/risky tools,
- add `files.write`,
- add transcript JSONL,
- add memory tools,
- add skill loading,
- add MCP bridge,
- add CardputerZero UI confirmations.
