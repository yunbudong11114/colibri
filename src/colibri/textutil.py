from __future__ import annotations

_TRUNCATED_SUFFIX = "\n...[truncated]"


def bound_text(text: str, max_chars: int) -> str:
    if len(text) <= max_chars:
        return text
    keep = max(0, max_chars - len(_TRUNCATED_SUFFIX))
    return text[:keep] + _TRUNCATED_SUFFIX
