import json

from colibri.transcript import TranscriptWriter


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
