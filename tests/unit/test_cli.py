from io import StringIO

import pytest

from colibri.cli import ReplLineEditor, main, read_escape_sequence, read_repl_line, read_tty_byte, write_raw_tty_newline
from colibri.config import AgentConfig
from colibri.model.errors import ModelError


@pytest.fixture(autouse=True)
def isolate_user_config(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path / "home"))


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
    assert "[colibri] ready model=fake-colibri-model\n" in captured.err
    assert "tools=" not in captured.err
    assert "memory=" not in captured.err
    assert "skills=" not in captured.err
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


def test_repl_exits_on_quit(capsys):
    inputs = iter(["hello", "/quit"])

    exit_code = main(["repl"], input_func=lambda _: next(inputs))

    captured = capsys.readouterr()

    assert exit_code == 0
    assert "fake: hello" in captured.out


def test_repl_exits_on_idle_timeout(capsys):
    config = AgentConfig.default().with_overrides({"session": {"idle_exit_enabled": True, "idle_exit_seconds": 1}})
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


def test_repl_idle_timeout_is_disabled_by_default(capsys):
    inputs = iter(["/quit"])
    times = iter([0.0, 999.0])

    exit_code = main(
        ["repl"],
        input_func=lambda prompt: next(inputs),
        monotonic_func=lambda: next(times),
    )

    captured = capsys.readouterr()

    assert exit_code == 0
    assert "idle_exit" not in captured.err


def test_parser_accepts_gateway_command(monkeypatch):
    called = []

    class FakeGateway:
        def __init__(self, config, model):
            called.append((config, model))

        def run(self):
            called.append("run")

    monkeypatch.setattr("colibri.cli.GatewayRunner", FakeGateway)

    exit_code = main(["gateway"])

    assert exit_code == 0
    assert called[-1] == "run"


def test_auth_weixin_saves_token_without_printing_secret(monkeypatch, tmp_path, capsys):
    class AuthResult:
        token = "secret-token"
        user_id = "user-1"
        account_id = "account-1"
        base_url = "https://redirect.weixin.test/"

    config_path = tmp_path / "config.toml"
    config_path.write_text(
        """
[model]
provider = "fake"
""".strip(),
        encoding="utf-8",
    )
    monkeypatch.setattr("colibri.cli.perform_weixin_auth", lambda base_url, timeout_seconds: AuthResult())

    exit_code = main(["--config", str(config_path), "auth", "weixin"])

    captured = capsys.readouterr()
    saved = config_path.read_text(encoding="utf-8")
    assert exit_code == 0
    assert "secret-token" not in captured.out
    assert "Config updated:" in captured.out
    assert "[channels.weixin]" in saved
    assert "enabled = true" in saved
    assert 'token = "secret-token"' in saved
    assert 'base_url = "https://redirect.weixin.test/"' in saved


def test_read_repl_line_reads_unicode_from_plain_stream():
    stdin = StringIO("我有我\n")
    stdout = StringIO()

    text = read_repl_line("colibri> ", timeout_seconds=0, stdin=stdin, stdout=stdout)

    assert text == "我有我"
    assert stdout.getvalue() == "colibri> "


def test_repl_line_editor_backspace_removes_cjk_and_redraws_line():
    stdout = StringIO()
    editor = ReplLineEditor("colibri> ", stdout)

    editor.start()
    editor.feed_text("尿尿是豆阿斯顿")
    editor.backspace()
    editor.backspace()
    editor.feed_text("斯顿")

    assert editor.text == "尿尿是豆阿斯顿"
    output = stdout.getvalue()
    assert "\r\x1b[2Kcolibri> 尿尿是豆阿斯" in output
    assert output.endswith("\r\x1b[2Kcolibri> 尿尿是豆阿斯顿")


def test_repl_line_editor_up_and_down_navigate_history_without_printing_escape_text():
    stdout = StringIO()
    editor = ReplLineEditor("colibri> ", stdout, history=["first", "第二个问题"])

    editor.start()
    editor.feed_text("draft")
    editor.history_previous()
    editor.history_previous()
    editor.history_next()
    editor.history_next()

    assert editor.text == "draft"
    output = stdout.getvalue()
    assert "\x1b[A" not in output
    assert "\x1b[B" not in output
    assert "\r\x1b[2Kcolibri> 第二个问题" in output
    assert "\r\x1b[2Kcolibri> first" in output
    assert output.endswith("\r\x1b[2Kcolibri> draft")


def test_read_tty_byte_uses_unbuffered_fd_read(monkeypatch):
    calls = []

    def fake_read(fd, size):
        calls.append((fd, size))
        return b"\xe6"

    monkeypatch.setattr("colibri.cli.os.read", fake_read)

    assert read_tty_byte(12) == b"\xe6"
    assert calls == [(12, 1)]


def test_read_escape_sequence_consumes_arrow_key_bytes(monkeypatch):
    available = [True, True]
    bytes_to_read = [b"[", b"A"]

    def fake_select(reads, writes, errors, timeout):
        return (reads, writes, errors) if available.pop(0) else ([], writes, errors)

    def fake_read(fd):
        return bytes_to_read.pop(0)

    monkeypatch.setattr("colibri.cli.select.select", fake_select)
    monkeypatch.setattr("colibri.cli.read_tty_byte", fake_read)

    assert read_escape_sequence(12) == b"\x1b[A"


def test_write_raw_tty_newline_returns_cursor_to_column_zero():
    stdout = StringIO()

    write_raw_tty_newline(stdout)

    assert stdout.getvalue() == "\r\n"


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
