from datetime import datetime, timedelta, timezone

from colibri.messages import Message
from colibri.time_context import (
    append_time_context_to_latest_user,
    current_time_context,
    user_text_without_time_context,
)


def test_current_time_context_uses_shared_positive_offset_format():
    now = datetime(2026, 8, 13, 9, 5, tzinfo=timezone(timedelta(hours=8)))

    assert current_time_context(now) == "## Current Time\n2026-08-13 09:05 (+08:00)"


def test_current_time_context_formats_negative_and_utc_offsets():
    negative = datetime(2026, 8, 12, 18, 35, tzinfo=timezone(-timedelta(hours=7, minutes=30)))
    utc = datetime(2026, 8, 13, 1, 5, tzinfo=timezone.utc)

    assert current_time_context(negative) == "## Current Time\n2026-08-12 18:35 (-07:30)"
    assert current_time_context(utc) == "## Current Time\n2026-08-13 01:05 (+00:00)"


def test_time_context_only_changes_active_user_tail():
    stable_prefix = [
        Message(role="system", content="summary"),
        Message(role="user", content="previous question"),
        Message(role="assistant", content="previous answer"),
    ]
    first = stable_prefix + [
        Message(role="user", content="[channel=weixin sender=alice]\ncurrent question")
    ]
    second = stable_prefix + [
        Message(role="user", content="[channel=console sender=bob]\ncurrent question")
    ]

    assert append_time_context_to_latest_user(
        first, "## Current Time\n2026-08-20 10:00 (+08:00)"
    )
    assert append_time_context_to_latest_user(
        second, "## Current Time\n2026-08-20 10:01 (+08:00)"
    )

    assert first[: len(stable_prefix)] == stable_prefix
    assert second[: len(stable_prefix)] == stable_prefix
    assert first[-1].content != second[-1].content


def test_user_text_without_time_context_requires_valid_runtime_suffix():
    augmented = "hello\n\n## Current Time\n2026-08-20 10:00 (+08:00)"

    assert user_text_without_time_context(augmented) == "hello"
    assert (
        user_text_without_time_context("hello\n\n## Current Time\nnot-runtime-data")
        == "hello\n\n## Current Time\nnot-runtime-data"
    )
