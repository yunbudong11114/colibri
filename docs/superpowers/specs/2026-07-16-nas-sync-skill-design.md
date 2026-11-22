# NAS Sync Skill Design

Date: 2026-07-16

## Goal

Make the user-level `nas-sync` skill compatible with UGREEN UGOS Pro WebDAV
paths while reducing rclone memory use on constrained devices.

## WebDAV Path

UGOS Pro WebDAV exposes shared-folder names rather than physical storage paths.
The NAS shared folder is `Colibri`, so both directions use:

```text
mynas:/Colibri
```

The scripts must not use the internal filesystem path `/volume1/Colibri`.

Before copying files, upload walks every local directory under `~/.colibri` in
parent-before-child order and calls `rclone mkdir` for the corresponding remote
path. The `run/` subtree is pruned just like the file copy exclusion. This
supports arbitrary future skill layouts without hard-coding
`skills/<name>/scripts` or relying on UGOS Pro to create multiple directory
levels during a file upload.

## Resource Limits

Both upload and download use the same low-memory rclone settings:

- `--transfers 1`: process one file transfer at a time.
- `--checkers 2`: limit concurrent metadata checks.
- `--buffer-size 0`: disable the default per-transfer 16 MiB buffer.
- `--multi-thread-streams 0`: disable multi-thread file transfers.
- `--max-backlog 256`: bound queued sync/check objects.
- `--stats 10s --stats-one-line`: provide bounded channel-friendly progress.

The scripts do not use unsupported `--no-create-dir-on-rename`. They also do
not add Nextcloud chunking options because the remote uses `vendor = other` and
the observed failure already occurs in rclone's unchunked update path.

## Shell Compatibility

Both scripts use POSIX `sh` syntax and `#!/bin/sh`. They must work when invoked
as either executable files or through `sh upload.sh` / `sh download.sh`.
Bash-only process substitution, `[[ ... ]]`, `read -d`, `&>` redirection, and
`set -o pipefail` are not used.

## Copy Semantics

- Upload uses `rclone copy` from `~/.colibri` to NAS.
- Download uses `rclone copy` from NAS to `~/.colibri`.
- Files present only at the destination are preserved in both directions.
- Existing files with the same path are updated from the source.
- `run/**` and `permissions.toml.lock` remain excluded.
- Existing rclone remote initialization behavior remains unchanged.

## User-Facing Output

Before invoking rclone, each script prints the source, destination, copy
semantics, exclusions, and low-memory concurrency settings. Completion messages
use `备份完成` and `恢复完成` so the output does not imply destructive mirroring.

## Verification

- Validate both scripts with `sh -n`.
- Verify all configured rclone flags exist in the installed rclone version.
- Run upload through `sh` against a fake rclone executable and assert that
  nested directories are created before the final file copy.
- Run a read-only `rclone lsd mynas:` probe separately before a destructive
  full sync when validating the NAS configuration.

## Cardputer Zero Dependency Installation

The deployed `memory-sync` skill declares `install-rclone` for missing rclone
on Cardputer Zero. Colibri runs it directly when called through skill.run.
The command accepts no arguments and starts a bounded user systemd job. APT
downloads the Debian package from the configured sources, and the job extracts
and installs only the rclone executable in `~/.local/bin`. This works without
sudo and lets the download finish beyond the normal skill execution timeout.
Sync scripts prefer this user path when available. Installation performs no
upload or restore. An existing rclone installation can also be used.
