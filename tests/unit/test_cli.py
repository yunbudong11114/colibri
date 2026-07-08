from colibri.cli import main
from colibri.config import AgentConfig
from colibri.model.errors import ModelError


def test_ask_prints_fake_response(capsys):
    exit_code = main(["ask", "status"])

    captured = capsys.readouterr()

    assert exit_code == 0
    assert captured.out.strip() == "fake: status"


def test_ask_prints_status_to_stderr(capsys):
    exit_code = main(["ask", "status"])

    captured = capsys.readouterr()

    assert exit_code == 0
    assert captured.out.strip() == "fake: status"
    assert "[colibri] ready model=fake-colibri-model" in captured.err
    assert "[colibri] thinking" in captured.err


def test_ask_can_disable_status(monkeypatch, capsys):
    config = AgentConfig.default().with_overrides({"console": {"status": False}})
    monkeypatch.setattr("colibri.cli.AgentConfig.load", lambda path: config)

    exit_code = main(["ask", "status"])

    captured = capsys.readouterr()

    assert exit_code == 0
    assert captured.out.strip() == "fake: status"
    assert captured.err == ""


def test_ask_closes_session_transcript(monkeypatch, capsys):
    transcript = FakeTranscript()
    monkeypatch.setattr("colibri.cli.TranscriptWriter.default", lambda: transcript)

    exit_code = main(["ask", "status"])

    assert exit_code == 0
    assert transcript.closed


def test_repl_exits_on_quit(monkeypatch, capsys):
    inputs = iter(["hello", "/quit"])
    monkeypatch.setattr("builtins.input", lambda _: next(inputs))

    exit_code = main(["repl"])

    captured = capsys.readouterr()

    assert exit_code == 0
    assert "fake: hello" in captured.out


def test_repl_exits_on_idle_timeout(capsys):
    config = AgentConfig.default().with_overrides({"session": {"idle_exit_seconds": 1}})
    times = iter([0.0, 2.0])

    exit_code = main(
        ["repl"],
        config_loader=lambda path: config,
        input_func=lambda prompt: "should not be called",
        monotonic_func=lambda: next(times),
    )

    captured = capsys.readouterr()

    assert exit_code == 0
    assert "[colibri] idle_exit seconds=1" in captured.err


def test_diagnostics_prints_key_value_lines(capsys):
    exit_code = main(["diagnostics"])

    captured = capsys.readouterr()

    assert exit_code == 0
    assert "colibri diagnostics" in captured.out
    assert "provider=fake model=fake-colibri-model" in captured.out


def test_main_returns_one_for_expected_model_errors(monkeypatch, capsys):
    class BrokenModel:
        def complete(self, messages, tools, system, limits):
            raise ModelError("boom")

    monkeypatch.setattr("colibri.cli.build_model_client", lambda config: BrokenModel())

    exit_code = main(["ask", "hello"])

    captured = capsys.readouterr()

    assert exit_code == 1
    assert "Model error: boom" in captured.err


class FakeTranscript:
    def __init__(self):
        self.closed = False

    def write(self, event_type, payload):
        return None

    def close(self):
        self.closed = True
