# Colibri Dynamic Permissions Design

Date: 2026-07-08
Status: Draft for user review
Milestone: 10
Scope: Claude Code style interactive permission grants for tools and shell commands

## 1. Goal

Colibri's current permissions are too static for an agent workflow. A tool can be visible to the model, but still fail inside the tool implementation because a command or path was not preconfigured. That makes the agent feel stuck: it cannot ask for permission, and the user cannot approve a useful action at the moment it is needed.

This milestone changes permissions from static allowlists into a small interactive grant system:

- tools remain visible to the model when enabled,
- every tool call goes through a unified permission policy before execution,
- missing grants trigger a short stdin/stdout confirmation prompt,
- the user can allow once, allow for the session, allow for the project, or deny,
- project-level shell grants are complete-command grants only,
- hard-deny safety rules remain available for dangerous commands.

The design should feel closer to Claude Code's permission flow while staying small enough for CardputerZero-class devices.

## 2. Non-Goals

This milestone will not implement:

- a graphical or full-screen TUI permission UI,
- a complex permission rule language,
- shell wildcard, regex, prefix, or executable-wide project grants,
- marketplace, plugin, or remote skill installation permissions,
- OS-level privilege escalation such as sudo or sandbox escape,
- network sandbox approval outside Colibri's own tool policy,
- persistent approval sync across machines.

## 3. Headless and CardputerZero Requirements

The permission flow must work on a pure Linux server over SSH or serial console.

Rules:

- Use stdin/stdout only.
- Keep prompts short and readable on small screens.
- Store session grants in memory only.
- Store project grants in a small TOML file.
- Avoid background daemons, databases, and resident watchers.
- Never require a GUI, browser, audio device, notification service, or terminal UI framework.

## 4. Permission Concepts

### 4.1 Grant Scopes

Colibri supports four user decisions:

- `once`: allow this exact tool call once.
- `session`: allow matching calls until the current process exits.
- `project`: persist a matching project-level grant.
- `deny`: deny this call and return a clear tool result to the model.

Session grants are kept in `PermissionPolicy` memory. Project grants are stored under the project root:

```text
.colibri/permissions.toml
```

The implementation should add `.colibri/permissions.toml` to `.gitignore` so personal project permissions are not committed accidentally.

### 4.2 Tool Grants

Tool grants are by tool name.

```toml
[tools]
names = ["files.list", "files.read"]
```

A project grant for `files.read` allows future `files.read` calls in the same project, but it does not allow unrelated tools such as `shell.run` or `memory.write`.

### 4.3 Shell Grants

Shell project grants are complete-command grants only.

```toml
[shell]
commands = ["pwd", "git status"]
```

`git status` allows exactly `git status`. It does not allow `git add`, `git commit`, `git push`, or arbitrary `git ...` commands.

Session shell grants may support both:

- exact command for the current session,
- executable for the current session, if the user explicitly chooses that option.

Project shell grants must not support executable-wide grants in this milestone.

## 5. Tool Execution Flow

The new execution flow is:

```text
Model requests tool call
-> ToolRegistry resolves the tool
-> Tool validates arguments and reports permission subject/risk
-> PermissionPolicy checks hard deny rules
-> PermissionPolicy checks session grants
-> PermissionPolicy checks project grants
-> PermissionPolicy applies default allow rules
-> If still undecided, prompt user
-> If allowed, run tool
-> If denied, return a denial ToolResult to the model
```

Denied calls are not fatal. They are normal feedback to the model. The result text should be explicit, for example:

```text
User denied shell.run: pwd
```

The model can then choose another tool, ask the user for information, or explain the limitation.

## 6. Shell Policy Changes

`shell.allow` should stop acting as a hard allowlist that blocks commands inside `ShellRunTool`.

New shell behavior:

- `shell.run` remains visible when `"shell"` is enabled.
- `ShellRunTool` validates the command string, parses it safely with `shlex.split()`, runs the command with `subprocess.run(argv, shell=False)`, enforces timeout, and bounds output.
- `ShellRunTool` does not reject an otherwise valid command only because it is not in `shell.allow`.
- `shell.deny` remains a hard-deny list for dangerous executables.
- Hard-denied commands are blocked before prompting.

The default hard-deny list remains conservative:

```toml
[shell]
deny = ["rm", "shutdown", "reboot", "mkfs", "dd", "sudo"]
```

Future work may add risk-specific prompts for network or destructive commands, but this milestone keeps hard-deny behavior simple.

## 7. File Workspace and Directory Grants

File tools should follow Claude Code's workspace model instead of prompting for every exact path.

Rules:

- The process startup directory, represented by the tool registry `cwd`, is the default workspace root.
- `files.roots` remains supported and is merged with the startup workspace root as additional automatic read roots.
- If `files.read` or `files.list` targets a path under the startup workspace root or configured `files.roots`, the read-only default policy can allow it without prompting.
- Path checks must use resolved paths. A symlink inside the workspace that points outside the workspace must not become implicitly allowed.
- If `files.read` or `files.list` targets a path outside the workspace roots, the permission subject becomes a `file_path` subject and must be granted or denied dynamically.
- `y` allows that one file tool call without creating a stored grant.
- `s` allows the target's containing directory recursively for the current session.
- `p` stores the target's containing directory recursively in the project permission file.
- A project grant for `files.read` or `files.list` as a tool name still does not grant arbitrary out-of-workspace paths.
- Missing, invalid, or unresolvable paths still return normal tool errors.

Project file grants are recursive directory grants, not exact file grants. Colibri does not support legacy exact file path grants.

This keeps normal project exploration quiet while preserving a clear trust boundary for reading outside the project.

## 8. Prompt Design

Prompts should be compact.

For shell:

```text
shell: pwd
[y] once [s] session [e] executable-session [p] project [n] deny:
```

For non-shell tools:

```text
tool: files.read {"path":"README.md"}
[y] once [s] session [p] project [n] deny:
```

For out-of-root file paths:

```text
file: files.list /home/user/project
[y] once [s] session-dir [p] project-dir [n] deny:
```

`e` is only valid for shell commands and only creates a session executable grant. If the user enters an unsupported choice, the call is denied by default.

The prompter should continue to work in tests through dependency injection.

## 9. Project Permission Store

Add a small project permission store:

```python
class ProjectPermissionStore:
    @classmethod
    def for_cwd(cls, cwd: Path) -> "ProjectPermissionStore": ...
    def load(self) -> ProjectGrants: ...
    def save(self, grants: ProjectGrants) -> None: ...
```

File format:

```toml
[shell]
commands = ["pwd", "git status"]

[tools]
names = ["files.list", "files.read"]

[files]
roots = ["/home/user/project"]
```

Behavior:

- Missing file means no project grants.
- Missing tables mean empty grants.
- Duplicate entries are ignored.
- Saves should create `.colibri/` when needed.
- Writes should be small and atomic enough for local usage: write a temp file in the same directory, then replace.

## 10. Permission Policy Interfaces

Extend permission concepts around a concrete subject:

```python
@dataclass(frozen=True)
class PermissionSubject:
    kind: Literal["tool", "shell"]
    tool_name: str
    shell_command: str | None = None
    shell_executable: str | None = None
    read_only: bool = False
```

`PermissionPolicy.decide()` should use this subject instead of only `tool.spec.read_only`.

The policy should know:

- default permission mode from config,
- hard-deny shell executables,
- session tool grants,
- session shell command grants,
- session shell executable grants,
- project tool grants,
- session file path grants,
- session file directory grants,
- project file directory grants,
- project shell command grants,
- injected prompter.

## 11. Default Policy

The default policy should be useful but cautious:

- Read-only non-shell tools can still run under `allow_read_confirm_write`.
- Non-read-only tools require a grant or prompt.
- Shell commands require a grant or prompt, even if they are read-like.
- Hard-denied shell executables are blocked without prompting.

This preserves the previous "read-only is easy" behavior for file and memory reads, but treats shell as special because even read-looking shell commands can leak secrets, consume resources, or behave differently across systems.

## 12. Transcript and Console Status

Transcript events should stay compact and explicit.

Extend `permission_decision` payloads with:

- `tool_name`,
- `subject_kind`,
- `decision`,
- `scope`,
- `allowed`,
- `reason`,
- `shell_command` when applicable,
- `file_path` and `file_root` when applicable.

Console status may continue to show:

```text
[colibri] tool shell.run wait_permission
[colibri] tool shell.run ok chars=...
```

Denied calls should show `permission_denied`.

## 13. Testing

Required tests:

- shell command not pre-allowlisted triggers prompt instead of internal rejection,
- `y` allows one exact shell command call,
- `s` allows the same shell command for the current session,
- `e` allows the same shell executable for the current session,
- `p` writes a complete shell command project grant,
- project grant for `git status` does not allow `git push`,
- hard-denied executable blocks without prompting,
- tool project grant allows the named tool only,
- denied tool call returns a denial result and does not execute,
- file roots auto-allow in-root read-only paths,
- out-of-root file paths prompt,
- `s` allows the out-of-root path's containing directory for the current session,
- `p` stores the out-of-root path's containing directory as a project file root grant,
- file tool-name grants do not allow arbitrary out-of-root paths,
- project permission store loads missing files as empty grants,
- project permission store saves deduplicated TOML,
- `.colibri/permissions.toml` is ignored by git,
- all tests run with `uv run python -m pytest`.

## 14. Migration Notes

Existing configs can keep `shell.allow`, but after this milestone it should be treated as legacy or as optional pre-granted session defaults, not as a hard runtime allowlist.

`shell.deny` remains active.

Documentation should explain that command approval is interactive. Users who want fully non-interactive operation can set:

```toml
[tools]
default_permission = "allow"
```

or pre-populate `.colibri/permissions.toml` for exact project commands.

## 15. Future Work

Future milestones may add:

- interactive path-root expansion,
- separate network risk classification,
- stricter destructive command detection beyond executable name,
- command prefix grants for advanced users,
- permission management CLI commands,
- expiry timestamps for project grants,
- MCP tool grant subjects.
