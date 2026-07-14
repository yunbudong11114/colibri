# Colibri Numeric Permission Prompts Design

Date: 2026-07-14

## Goal

Replace user-facing permission choices with numeric choices plus English descriptions across Python, Rust, CLI/repl, and Weixin. Do not accept legacy letter replies. Rejection uses `0`.

## Menu

Shell prompts:

```text
[1] once [2] session-command [3] session-executable [4] user-command [5] user-executable [0] deny
```

File path prompts:

```text
[1] once [2] session-dir [4] user-dir [0] deny
```

Tool prompts:

```text
[1] once [2] session [4] user [0] deny
```

The numbers have stable meanings where possible:

- `1`: allow only this call.
- `2`: allow for this session.
- `3`: shell only, allow the executable for this session.
- `4`: persist an exact user-level grant.
- `5`: shell only, persist the executable as a user-level grant.
- `0`: deny.

File directory grants are shared by file-path tools. A `user-dir` approval for a directory can cover `files.list`, `files.read`, `files.write`, `files.send`, and `image.understand` when they target paths under that directory.

Shell user executable grants are stored as `[shell].executables`. Older `[shell].prefixes` files may be read for migration compatibility, but new writes use `executables` only.

## Channel Formatting

CLI/repl prompts may stay compact and single-line after the subject details.

Weixin prompts should be easier to read in chat and may use a vertical list:

```text
Colibri wants to run shell.run.
shell: git status

choose:
1. once
2. session-command
3. session-executable
4. user-command
5. user-executable
0. deny
```

The Weixin formatting can differ from repl, but the numeric meanings must be identical.

## Compatibility

Legacy letter replies are intentionally not accepted. This keeps REPL and channel behavior simple and prevents old `p`/project-level mental models from surviving in the permission UI. Colibri reads the first whitespace-delimited token; if that token is not an available numeric choice, the reply is treated as `0` deny.

## Implementation

- Add small choice helpers in Python and Rust so console/repl and Weixin share the same numeric meanings.
- Update Python and Rust console prompters to print the numeric menus.
- Update Python and Rust Weixin prompts to print readable vertical numeric menus.
- Update README permission sections and Weixin channel documentation.
- Add tests that assert numeric replies map to the intended decisions, legacy letter replies deny, and Weixin prompt text contains a readable numeric menu.

## Testing

- Python targeted tests for permissions, channels, and README-facing prompt behavior.
- Rust runtime tests for permission policy numeric replies and Weixin numeric reply parsing.
- Full Python and Rust test suites after implementation.
