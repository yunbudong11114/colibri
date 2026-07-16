# Colibri Shell Redirection Permission Classification Design

Date: 2026-07-16

## Goal

Keep the existing `shell.run` permission model unchanged while preventing
file-descriptor redirections and `/dev/null` output from being classified as
filesystem writes.

## Existing Permission Behavior

Normal shell commands continue to use the existing shell choices:

```text
[1] once [2] session-command [3] session-executable [4] user-command [5] user-executable [0] deny
```

Commands that write to a real filesystem path continue to use:

```text
[1] once [2] session-dir [4] user-dir [0] deny
```

The numeric choices, persisted grants, session grants, command matching, and
`shell.deny` precedence do not change.

## Classification Rules

The redirection parser must return no filesystem target for:

- File-descriptor duplication such as `2>&1` and `1>&2`.
- File-descriptor closure such as `2>&-`.
- Output discarded through `/dev/null`, including forms such as
  `2>/dev/null` and `> /dev/null`.

The parser must continue returning the target path for real writes:

- Inline redirection such as `>out.txt`, `2>errors.log`, and `>>output.log`.
- Split redirection such as `> out.txt` and `2> errors.log`.
- `tee` output such as `tee /tmp/output.txt`.

Hard-denied executables are checked before any prompt and remain impossible to
authorize through shell or file grants.

## Runtime Parity

Python and Rust must implement the same classification rules and expose the
same permission subject:

- Ignored redirection: `subject_kind = "shell"`.
- Real file target: `subject_kind = "file_path"`.

## Testing

Both runtimes require tests for:

- `2>&1`
- `1>&2`
- `2>&-`
- `2>/dev/null`
- A real inline file redirection remains `file_path`
- A real split file redirection remains `file_path`
- Existing `shell.deny` precedence remains covered
