# Colibri Documentation

This directory keeps design and maintenance documents. User-facing usage docs live in the repository root:

- `README.md`: English user guide.
- `README.zh-CN.md`: Chinese user guide.

## Specs

The active implementation specs are in `docs/superpowers/specs/`:

- `2026-07-01-colibri-design.md`: overall architecture and roadmap.
- `2026-07-06-colibri-openai-compatible-model-design.md`: OpenAI-compatible model adapter.
- `2026-07-06-colibri-minimum-tool-loop-design.md`: bounded tool loop.
- `2026-07-06-colibri-permissions-transcript-design.md`: permissions and transcript logging.
- `2026-07-07-colibri-file-memory-tools-design.md`: file-backed memory tools.
- `2026-07-07-colibri-memory-recall-design.md`: memory recall injection.
- `2026-07-07-colibri-context-compacting-design.md`: context compacting.
- `2026-07-07-colibri-local-skills-design.md`: local skills (historical; superseded for injection by catalog redesign).
- `2026-07-13-colibri-skills-catalog-design.md`: skill catalog + `skill.read` progressive disclosure.
- `2026-07-08-colibri-dynamic-permissions-design.md`: dynamic permissions and project grants.
- `2026-07-08-colibri-web-search-design.md`: web search tool.
- `2026-07-08-colibri-weixin-gateway-design.md`: gateway and Weixin channel.
- `2026-07-09-colibri-weixin-media-design.md`: Weixin file/image send and receive behavior, including Rust parity requirements.
- `2026-07-09-colibri-channel-followups.md`: selected channel follow-up issue.
- `2026-07-10-colibri-steering-display-design.md`: steering UX on Weixin + lightweight concurrent REPL (Approach 2).

## Plans

Implementation plans are kept in `docs/superpowers/plans/`. They are historical execution aids; prefer the current specs and root README files for the latest behavior.

Recent plan (implemented):

- `2026-07-10-colibri-steering.md`: mid-turn steering + Weixin ack + concurrent REPL (Python/Rust). Spec: `2026-07-10-colibri-steering-display-design.md`.

## Private Reference Notes

Local research notes for Claude Code, PicoClaw, and ZeroClaw are intentionally ignored by git and should not be uploaded. See `.gitignore` entries under `docs/reference/` (including `2026-07-10-picoclaw-zeroclaw-lessons-for-colibri.md`).
