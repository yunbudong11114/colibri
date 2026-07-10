import json
import os
import time

from colibri.transcript import ScopedTranscriptWriter, TranscriptWriter


def test_transcript_writer_writes_jsonl_event(tmp_path):
    path = tmp_path / "session.jsonl"
    writer = TranscriptWriter(path)

    writer.write("user_message", {"text": "hello"})
    writer.close()

    lines = path.read_text(encoding="utf-8").splitlines()
    assert len(lines) == 1

    event = json.loads(lines[0])
    assert event["type"] == "user_message"
    assert event["payload"] == {"text": "hello"}
    assert event["ts"].endswith("Z")


def test_default_transcript_path_uses_colibri_home(monkeypatch, tmp_path):
    monkeypatch.setenv("COLIBRI_HOME", str(tmp_path))

    writer = TranscriptWriter.default()
    writer.write("assistant_message", {"text": "ok"})
    writer.close()

    files = list((tmp_path / "transcripts").glob("*.jsonl"))
    assert len(files) == 1
    assert json.loads(files[0].read_text(encoding="utf-8").splitlines()[0])["type"] == "assistant_message"


def test_scoped_transcript_writer_injects_metadata_without_closing_base(tmp_path):
    path = tmp_path / "session.jsonl"
    writer = TranscriptWriter(path)
    scoped = ScopedTranscriptWriter(writer, {"channel": "weixin", "sender_id": "user-1"})

    scoped.write("user_message", {"text": "hello"})
    scoped.close()
    writer.write("assistant_message", {"text": "ok"})
    writer.close()

    events = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]
    assert events[0]["payload"] == {"text": "hello", "channel": "weixin", "sender_id": "user-1"}
    assert events[1]["payload"] == {"text": "ok"}


def test_transcript_writer_removes_expired_files_but_preserves_active_file(tmp_path):
    directory = tmp_path / "transcripts"
    directory.mkdir()
    expired = directory / "2026-01-01.jsonl"
    recent = directory / "2026-07-09.jsonl"
    active = directory / "2026-07-10.jsonl"
    expired.write_text("expired", encoding="utf-8")
    recent.write_text("recent", encoding="utf-8")
    old = time.time() - 40 * 86400
    os.utime(expired, (old, old))

    writer = TranscriptWriter(active, retention_days=30)
    writer.close()

    assert not expired.exists()
    assert recent.exists()
    assert active.exists()


def test_transcript_writer_removes_oldest_inactive_files_to_fit_size_limit(tmp_path):
    directory = tmp_path / "transcripts"
    directory.mkdir()
    oldest = directory / "2026-07-08.jsonl"
    newest = directory / "2026-07-09.jsonl"
    active = directory / "2026-07-10.jsonl"
    oldest.write_bytes(b"a" * 10)
    newest.write_bytes(b"b" * 10)
    now = time.time()
    os.utime(oldest, (now - 20, now - 20))
    os.utime(newest, (now - 10, now - 10))

    writer = TranscriptWriter(active, max_total_bytes=15)
    writer.close()

    assert not oldest.exists()
    assert newest.exists()
    assert active.exists()


def test_transcript_writer_throttles_cleanup_during_writes(tmp_path):
    now = [0.0]
    active = tmp_path / "transcripts" / "2026-07-10.jsonl"
    writer = TranscriptWriter(active, retention_days=1, cleanup_interval_seconds=60, time_func=lambda: now[0])
    expired = active.parent / "2026-01-01.jsonl"
    expired.write_text("expired", encoding="utf-8")
    os.utime(expired, (0, 0))

    now[0] = 30
    writer.write("user_message", {"text": "first"})
    assert expired.exists()

    now[0] = 86461
    writer.write("user_message", {"text": "second"})
    writer.close()

    assert not expired.exists()
