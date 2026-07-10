import json
from pathlib import Path

from colibri.config import AgentConfig
from colibri.session_history import TranscriptHistoryLoader


def _event(event_type: str, text: str = "", **payload: object) -> str:
    return json.dumps(
        {
            "ts": payload.pop("ts", "2026-07-10T08:00:00+08:00"),
            "type": event_type,
            "payload": {"text": text, **payload},
        },
        ensure_ascii=False,
    )


def _write_events(path: Path, *events: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(events) + "\n", encoding="utf-8")


def _pairs(messages):
    return [(messages[index].content, messages[index + 1].content) for index in range(0, len(messages), 2)]


def test_loader_restores_only_complete_final_turns_and_strips_attachment_paths(tmp_path):
    transcript = tmp_path / "transcripts" / "2026-07-10.jsonl"
    _write_events(
        transcript,
        "not-json",
        _event(
            "user_message",
            "请分析图片\n\nAttachments saved locally:\n1. image: a.png at /tmp/colibri/media/a.png, content_type=image/png",
        ),
        _event("assistant_message", "", tool_call_count=1),
        _event("tool_call", "ignored"),
        _event("assistant_message", "图片内容", tool_call_count=0),
        _event("user_message", "尚未回答"),
        _event("assistant_message", "孤立回答", tool_call_count=0, session_key="other"),
    )

    messages = TranscriptHistoryLoader(tmp_path, message_limit=24, char_limit=24000, scan_bytes=2097152).load()

    assert [(message.role, message.content) for message in messages] == [
        ("user", "请分析图片"),
        ("assistant", "图片内容"),
    ]


def test_loader_pairs_each_source_then_merges_turns_by_completion_order(tmp_path):
    transcript = tmp_path / "transcripts" / "2026-07-10.jsonl"
    _write_events(
        transcript,
        _event("user_message", "微信问题", session_key="weixin:user"),
        _event("user_message", "REPL 问题"),
        _event("assistant_message", "REPL 回答", tool_call_count=0),
        _event("assistant_message", "微信回答", tool_call_count=0, session_key="weixin:user"),
    )

    messages = TranscriptHistoryLoader(tmp_path, message_limit=24, char_limit=24000, scan_bytes=2097152).load()

    assert _pairs(messages) == [("REPL 问题", "REPL 回答"), ("微信问题", "微信回答")]


def test_loader_applies_message_and_character_limits_to_whole_turns(tmp_path):
    transcript = tmp_path / "transcripts" / "2026-07-10.jsonl"
    _write_events(
        transcript,
        _event("user_message", "old-user"),
        _event("assistant_message", "old-answer", tool_call_count=0),
        _event("user_message", "middle-user"),
        _event("assistant_message", "middle-answer", tool_call_count=0),
        _event("user_message", "new-user"),
        _event("assistant_message", "new-answer", tool_call_count=0),
    )

    by_messages = TranscriptHistoryLoader(tmp_path, message_limit=4, char_limit=24000, scan_bytes=2097152).load()
    by_chars = TranscriptHistoryLoader(tmp_path, message_limit=24, char_limit=21, scan_bytes=2097152).load()

    assert _pairs(by_messages) == [("middle-user", "middle-answer"), ("new-user", "new-answer")]
    assert _pairs(by_chars) == [("new-user", "new-answer")]


def test_loader_reads_newest_file_tails_within_scan_budget(tmp_path):
    old_file = tmp_path / "transcripts" / "2026-07-09.jsonl"
    new_file = tmp_path / "transcripts" / "2026-07-10.jsonl"
    _write_events(
        old_file,
        _event("user_message", "old-user"),
        _event("assistant_message", "old-answer", tool_call_count=0),
    )
    _write_events(
        new_file,
        _event("user_message", "discarded-user"),
        _event("assistant_message", "discarded-answer", tool_call_count=0),
        _event("user_message", "new-user"),
        _event("assistant_message", "new-answer", tool_call_count=0),
    )
    scan_bytes = len((_event("user_message", "new-user") + "\n" + _event("assistant_message", "new-answer", tool_call_count=0) + "\n").encode()) + 8

    messages = TranscriptHistoryLoader(tmp_path, message_limit=24, char_limit=24000, scan_bytes=scan_bytes).load()

    assert _pairs(messages) == [("new-user", "new-answer")]


def test_default_loader_uses_colibri_home_and_session_config(monkeypatch, tmp_path):
    monkeypatch.setenv("COLIBRI_HOME", str(tmp_path))
    _write_events(
        tmp_path / "transcripts" / "2026-07-10.jsonl",
        _event("user_message", "previous"),
        _event("assistant_message", "answer", tool_call_count=0),
    )
    config = AgentConfig.default().session

    messages = TranscriptHistoryLoader.default(config).load()

    assert _pairs(messages) == [("previous", "answer")]
