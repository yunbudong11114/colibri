# Colibri Skill Tool Split Design

## Goal

Align skill code organization with the existing memory split:

- `colibri.skills` owns skill indexing, selection, built-in skill metadata, and context injection.
- `colibri.tools.builtin.skills` owns the `skill.run` tool.

This is a structure-only cleanup. It should not change runtime behavior, permission behavior, or the `skill.run` schema.

## Current Shape

Memory already has two layers:

- `src/colibri/memory.py`: automatic memory recall for session context.
- `src/colibri/tools/builtin/memory.py`: model-callable memory tools.

Skills currently mix both layers in `src/colibri/skills.py`:

- `SkillIndex`, `SkillMetadata`, selection, and context loading.
- built-in `create-colibri-skill` metadata.
- `SkillRunTool`.

## New Shape

Keep this in `src/colibri/skills.py`:

- `SkillCommand`
- `SkillMetadata`
- `SkillContext`
- `SkillIndex`
- built-in skill content and selection helpers

Move this to `src/colibri/tools/builtin/skills.py`:

- `SkillRunTool`

`SkillRunTool` will import `SkillIndex` from `colibri.skills`, exactly as other tools depend on their domain helpers.

## Imports

`ToolRegistry` should import `SkillRunTool` from `colibri.tools.builtin`, not from `colibri.skills`.

`colibri.tools.builtin.__init__` should export `SkillRunTool`.

Tests that directly exercise `SkillRunTool` should import it from `colibri.tools.builtin`.

## Verification

- Existing skill index tests still pass.
- Existing `skill.run` tests still pass.
- Existing permission/session tests for `skill.run` still pass.
- Full test suite passes.
