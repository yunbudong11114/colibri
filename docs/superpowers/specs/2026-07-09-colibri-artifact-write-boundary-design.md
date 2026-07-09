# Colibri Artifact Write Boundary Design

Date: 2026-07-09

## Problem

Colibri currently exposes `shell.run` plus read-only file tools. When a user asks the agent to create an artifact, such as an HTML file, the model falls back to shell redirection or heredoc commands. This makes output paths opaque to the permission layer and can lead to files being written into arbitrary locations such as `/home/baidu.html`.

The desired behavior is that generated artifacts are written through a structured file tool, under explicit allowed roots, with shell treated as an execution tool rather than the default file writer.

## Goals

- Give the model a first-class `files.write` tool for artifact creation.
- Keep file writes inside configured file roots or the startup cwd unless the user grants a specific out-of-root directory.
- Detect common shell redirection write attempts and route them through the same path-aware permission model.
- Improve prompts/tool descriptions so the model prefers `files.write` for files.
- Preserve existing read/list behavior and dynamic permission grants.

## Non-Goals

- Do not implement a full shell parser.
- Do not sandbox arbitrary process writes at the OS level in this change.
- Do not change project permission storage format unless required.
- Do not migrate existing user config automatically in this change.

## Design

### Structured Artifact Writes

Add `files.write` as a non-read-only tool with arguments:

- `path`: destination path.
- `content`: UTF-8 text content.

The tool resolves relative paths against the registry cwd and uses the same allowed-root check as `files.read` and `files.list`. Parent directories are created only after the path is authorized. The result reports the written path and byte count.

### Default Artifact Directory

The default file roots are narrowed to Colibri-owned scratch locations:

- `~/.colibri/workspace`
- `/tmp/colibri`

The startup cwd remains an allowed root for existing project workflows, but tool descriptions and the system prompt tell the model to place generated artifacts in the configured workspace rather than using arbitrary absolute paths.

### Shell Write Detection

The permission subject classifier will inspect `shell.run` commands for simple write targets:

- Redirection operators: `>`, `>>`, `1>`, `1>>`, `2>`, `2>>`, `&>`, `&>>`.
- Common writer commands: `tee`, `cat > file`, heredoc with final redirection.

When a write target is detected, the permission subject becomes `file_path` instead of plain `shell`. This lets the existing file-root grant flow ask for path or directory approval. If the path is already inside the workspace/file roots, the permission can be allowed by existing defaults or grants; otherwise it prompts as an out-of-root path.

This is a guardrail, not a complete shell security boundary. Complex commands can still write indirectly; stronger OS sandboxing belongs in a future runtime hardening pass.

### Model Guidance

Update `SYSTEM_PROMPT` and `shell.run` description:

- Use `files.write` for creating or editing files.
- Do not use shell redirection/heredoc for file creation unless the user explicitly asks for a shell command.
- Put generated artifacts in the allowed workspace/current project unless the user provides a path.

### Permission Prompt UX

Permission prompts should prefer human-checkable paths over raw tool arguments.

Rules:

- `files.write` should always classify as a `file_path` permission subject, even when the target is under the startup cwd or configured file roots. This keeps write approval prompts path-oriented.
- `files.read` and `files.list` may keep the existing default read-only allow behavior for paths inside allowed roots; out-of-root paths still classify as `file_path`.
- `file_path` prompts must show the resolved absolute path and, for shell redirection, the original shell command.
- `files.write` prompts must not dump the full `content` argument. They should show content size and at most a short preview.
- `memory.write` is a memory namespace operation, not a filesystem path operation. Its prompt should show `file`/`topic`, `mode`, and content size/preview rather than an absolute path.
- Console and Weixin permission prompts should use the same formatter so channel behavior does not drift.

The transcript may still retain raw tool-call arguments for auditing.

## Testing

- Add tests that `files.write` writes inside an allowed root and rejects outside roots.
- Add tests that out-of-root `files.write` prompts for `file_path` permission.
- Add tests that in-root `files.write` prompts display a resolved absolute path and hide full content.
- Add tests that `memory.write` prompts display memory target and content summary instead of full content.
- Add tests that Weixin permission prompts use the same sanitized formatter.
- Add tests that shell redirection to an out-of-root path produces a `file_path` permission subject.
- Add tests that registry exposes `files.write`.
- Run focused tests for tools, permissions, session, and registry behavior.

## Risks

- Shell write detection is intentionally conservative and may miss complex constructs.
- Classifying shell writes as `file_path` changes permission prompt wording for some shell commands.
- Adding `files.write` may make artifact generation easier, so root restrictions must remain strict.
