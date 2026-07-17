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
  - `3` allows every parsed executable in the approved command for the current
    session only.
  - `5` allows every parsed executable in the approved command persistently at
    user level.
  - A pipeline or compound command is auto-allowed only when every parsed
    executable is covered by the corresponding executable grants or an exact
    command-segment grant.
  - This prevents an approval loop where approving `curl ... | head ...` stores
    only `curl`, then asks again forever because `head` can never be stored.
  - `shell.deny` remains stronger than every grant: every executable in every
    compound segment is checked against the deny list before grants are
    considered.
- `[shell].executables` is the only supported executable-grant key. The obsolete
  `[shell].prefixes` key is ignored and its compatibility code is removed.
- Shell executable matching rejects commands with dangerous shell features before checking executable grants.
- Internal code should use executable terminology instead of the old prefix terminology.

## Verification

- Python unit tests cover user command, user executable, user file root, store path, and numeric prompt choices.
- Python and Rust tests cover storing every executable from an approved
  pipeline or compound command and reusing that grant without another prompt.
- Rust runtime tests cover matching behavior and persistence.
- Existing parity tests must be updated to the new test names and continue to pass.
