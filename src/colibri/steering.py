from __future__ import annotations

SKIPPED_TOOL_RESULT = "Skipped due to queued user message."
STEERING_QUEUE_MAX = 4
STEERING_PREVIEW_CHARS = 20


def format_steering_ack(skipped: int, steering_text: str) -> str:
    line1 = f"已改方向，跳过剩余 {skipped} 个工具"
    stripped = steering_text.strip()
    if not stripped:
        return line1
    preview = stripped[:STEERING_PREVIEW_CHARS]
    if len(stripped) > STEERING_PREVIEW_CHARS:
        preview += "…"
    return f"{line1}\n改：{preview}"
