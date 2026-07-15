# Colibri Session Permission Cache Design

Date: 2026-07-15

## Goal

Make Python and Rust session-scoped permissions last for the same
`AgentSession` lifetime, while reducing repeated parsing of
`~/.colibri/permissions.toml` without allowing concurrent writers to lose
grants.

## Scope

This change is limited to:

- aligning the Python and Rust session permission lifecycle;
- adding a concurrency-safe cache and merge-save path for user permissions;
- removing obsolete `[shell].prefixes` compatibility.

It does not change prompt choices, permission scopes, default permission
behavior, command parsing, file-root matching, or the public configuration.

## Session Lifecycle

`PermissionPolicy` belongs to `AgentSession` in both runtimes. Session grants
therefore survive multiple `submit` calls handled by the same session and are
discarded when that session is destroyed.

Rust must not store a borrowed REPL or channel prompter inside the persistent
policy. The current submission passes its temporary prompter into permission
evaluation. The policy retains only configuration, cached user grants, and
session grants.

The following scopes share this lifecycle:

- exact shell command;
- shell executable;
- file directory;
- ordinary tool name.

## Cached Reads

Each policy keeps the last successfully loaded `UserGrants` and a file
fingerprint containing file identity, nanosecond modification time, and byte
length. Permission evaluation checks the fingerprint first:

- unchanged fingerprint: use the cached grants without opening or parsing the
  TOML file;
- changed fingerprint: reload and parse the file, then replace the cache;
- missing file: use empty grants and cache the missing state;
- malformed or unreadable file: preserve each runtime's existing error
  behavior; caching must not silently change permission semantics.

The fingerprint is an invalidation hint, not a concurrency primitive. Correct
writes are provided by the merge-save path below.

## Concurrent Merge-Save

Persisting a user grant uses a dedicated sibling lock file. On macOS and Linux,
the writer acquires an exclusive operating-system file lock, then performs the
following steps while holding it:

1. Reload the latest grants from `permissions.toml`.
2. Union the requested grant with the latest grants.
3. Sort and deduplicate all persisted lists.
4. Write a temporary file in the same directory.
5. Atomically replace `permissions.toml`.
6. Release the lock.

The merge operation accepts only a grant delta rather than a complete cached
snapshot. Two AgentSessions or two Colibri processes adding different grants
must retain both grants regardless of save order.

After a successful save, the calling policy refreshes its cached grants and
fingerprint from the merged result. Save failures remain failures and must not
be reported as successful persistence.

## Permission File Format

The supported format is:

```toml
[shell]
commands = []
executables = []

[tools]
names = []

[files]
roots = []
```

`[shell].prefixes` is obsolete. Python and Rust do not read it, migrate it, or
write it. Compatibility helpers and tests referring to that key are removed.

## Cross-Runtime Behavior

Python and Rust must agree on:

- the lifetime of every session grant;
- cache invalidation after an external file change;
- union semantics for concurrent persistent grants;
- sorted, deduplicated output;
- exclusive use of `[shell].executables`.

Implementation details may differ where required by the standard libraries,
but both runtimes use an OS-level lock and atomic same-directory replacement.
No new runtime configuration or third-party dependency is introduced.

## Testing

Python and Rust tests must cover:

- a session grant approved during one submit is reused by a later submit in the
  same `AgentSession`;
- a new `AgentSession` does not inherit session grants;
- unchanged permission files do not require repeated TOML parsing;
- an external file replacement invalidates the cache;
- two stores starting from stale views can persist different grants without
  losing either grant;
- writes remain sorted and deduplicated;
- `[shell].prefixes` is ignored;
- existing numeric prompt and hard-deny behavior remains unchanged.

The complete Python and Rust test suites and Rust release build are required
before completion.
