# Colibri User Permissions Design

## Context

Colibri previously exposed persistent grants as project permissions stored in `./.colibri/permissions.toml`. That wording is confusing in REPL and channel confirmation prompts, and the user-facing model should be simpler: one-time, session, or user-level persistent permissions.

Python and Rust must keep matching behavior.

## Design

- Persistent permissions are user-level and stored at `~/.colibri/permissions.toml`.
- REPL and channel prompts use numeric choices only:
  - shell: `[1] once [2] session-command [3] session-executable [4] user-command [5] user-executable [0] deny`
  - file: `[1] once [2] session-dir [4] user-dir [0] deny`
  - tool: `[1] once [2] session [4] user [0] deny`
- `4` saves the same subject as the old persistent project option:
  - shell: exact command in `[shell].commands`
  - file: directory/root in `[files].roots`
  - tool: tool name in `[tools].names`
- `5` applies only to shell subjects and saves the parsed shell executable in `[shell].executables`.
- `5 user-executable` and `3 session-executable` intentionally use the same granularity:
  - `3` allows the executable for the current session only.
  - `5` allows the executable persistently at user level.
- Existing `[shell].prefixes` is read only as a legacy compatibility alias. New writes use `[shell].executables`.
- Shell executable matching rejects commands with dangerous shell features before checking executable grants.
- Internal code should use executable terminology instead of the old prefix terminology.

## Verification

- Python unit tests cover user command, user executable, user file root, store path, and numeric prompt choices.
- Rust runtime tests cover matching behavior and persistence.
- Existing parity tests must be updated to the new test names and continue to pass.
