from colibri.console import ConsoleStatusWriter, StatusTranscript


def test_status_writer_prints_plain_prefixed_lines(capsys):
    status = ConsoleStatusWriter(enabled=True)

    status.write("ready", model="fake")

    captured = capsys.readouterr()
    assert captured.err.strip() == "[colibri] ready model=fake"
    assert captured.out == ""


def test_status_writer_can_be_disabled(capsys):
    status = ConsoleStatusWriter(enabled=False)

    status.write("ready", model="fake")

    captured = capsys.readouterr()
    assert captured.err == ""


def test_status_transcript_maps_selected_events(capsys):
    transcript = MemoryTranscript()
    status = ConsoleStatusWriter(enabled=True)
    wrapped = StatusTranscript(transcript=transcript, status=status)

    wrapped.write("memory_recall", {"topics": ["devices"], "truncated": False})
    wrapped.write("tool_result", {"name": "files.read", "ok": True, "text": "abcd"})
    wrapped.write("context_compact", {"mode": "model", "dropped_messages": 2, "summary_chars": 120})

    captured = capsys.readouterr()
    assert "[colibri] memory topics=devices" in captured.err
    assert "[colibri] tool files.read ok chars=4" in captured.err
    assert "[colibri] compact mode=model dropped=2 summary_chars=120" in captured.err
    assert [event_type for event_type, _payload in transcript.events] == [
        "memory_recall",
        "tool_result",
        "context_compact",
    ]


class MemoryTranscript:
    def __init__(self):
        self.events = []
        self.closed = False

    def write(self, event_type, payload):
        self.events.append((event_type, payload))

    def close(self):
        self.closed = True
