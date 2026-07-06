# Colibri Permissions and Transcript Design

Date: 2026-07-06
Status: Approved by user direction
Milestone: 3
Scope: Permission decision skeleton and JSONL transcript logging

## 1. Goal

Milestone 3 makes Colibri's tool execution observable and ready for higher-risk tools.

After this milestone, Colibri should:

- decide whether a tool call is allowed, denied, or needs confirmation,
- allow read-only tools by default,
- provide a headless CLI confirmation path for future risky tools,
- support session-scoped "always allow",
- record compact JSONL transcript events for user input, assistant output, tool calls, tool results, errors, and round limits.

The milestone does not need to add write, network, memory, skills, MCP, or GPIO tools. It prepares the execution layer so those can be added safely later.

## 2. Headless Requirement

All permission and transcript behavior must work on pure Linux servers over SSH.

Rules:

- No GUI dependency.
- No browser dependency.
- No audio, display, notification, or TUI framework dependency.
- Confirmation prompts must work through stdin/stdout.
- Tests must be able to inject non-interactive confirmation behavior.

## 3. Permission Model

Add a small permission layer independent of tool implementation.

Decision values:

- `allow`: run the tool.
- `deny`: block the tool.
- `confirm`: ask the configured prompter.

Default behavior:

- Read-only tools are allowed when `tools.default_permission = "allow_read_confirm_write"`.
- Non-read-only tools require confirmation under `allow_read_confirm_write`.
- `tools.default_permission = "allow"` allows all registered tools.
- `tools.default_permission = "confirm"` confirms all tool calls.
- `tools.default_permission = "deny"` denies all tool calls.

Confirmation choices:

- `yes`: allow this call once.
- `no`: deny this call once.
- `always`: allow this tool name for the rest of the current session.

There is no persistent permission store in this milestone.

## 4. Permission Interfaces

Add `src/colibri/tools/permissions.py`:

```python
@dataclass(frozen=True)
class PermissionRequest:
    tool_name: str
    arguments: dict[str, Any]
    read_only: bool

class PermissionPrompter(Protocol):
    def confirm(self, request: PermissionRequest) -> str: ...

class ConsolePermissionPrompter:
    def confirm(self, request: PermissionRequest) -> str: ...

class PermissionPolicy:
    def check(self, tool: Tool, arguments: dict[str, Any]) -> PermissionDecision: ...
    def confirm(self, request: PermissionRequest) -> bool: ...
```

`PermissionPolicy` should own session-scoped always-allow tool names.

`AgentSession` should accept optional `permission_policy`. If absent, it builds a default policy with a console prompter.

## 5. Tool Registry Changes

`ToolRegistry` should expose:

```python
def get(self, name: str) -> Tool | None: ...
```

`AgentSession` should:

1. resolve the tool,
2. ask the permission policy,
3. run the tool only when allowed,
4. append a denied tool result when blocked.

Unknown tools should still return an `unknown_tool` result.

## 6. Transcript Logging

Add compact JSONL transcript support.

Default path:

```text
~/.colibri/transcripts/YYYY-MM-DD.jsonl
```

Transcript events:

- `user_message`
- `assistant_message`
- `tool_call`
- `tool_result`
- `permission_decision`
- `model_error`
- `round_limit`

Each event should include:

- ISO timestamp in UTC,
- event type,
- compact payload fields.

Do not log API keys. Do not log environment variables. Tool arguments and results may be logged, but result text must be capped to `tools.max_result_chars`.

## 7. Transcript Interfaces

Add `src/colibri/transcript.py`:

```python
class TranscriptWriter:
    @classmethod
    def default(cls) -> "TranscriptWriter": ...
    def write(self, event_type: str, payload: dict[str, Any]) -> None: ...
    def close(self) -> None: ...
```

`AgentSession` should accept optional `transcript`.

Behavior:

- If `config.session.transcript` is false, do not create a default writer.
- If a writer is passed explicitly, use it even in tests.
- Closing the session closes the writer.

## 8. Testing

Required tests:

- read-only tools are allowed under `allow_read_confirm_write`,
- `confirm` default calls a fake prompter,
- `always` grants are session-scoped,
- denied calls produce a tool result and are not executed,
- transcript writer writes valid JSONL,
- session writes user, assistant, tool_call, tool_result, permission_decision, and round_limit events,
- session close closes transcript writer,
- CLI fake path still works,
- all tests run with `uv run python -m pytest`.

## 9. Future Work

After this milestone:

- add `files.write`,
- add `http.fetch`,
- add persistent permission grants if desired,
- add transcript rotation limits,
- add memory tools,
- add skill loading,
- add MCP bridge.
