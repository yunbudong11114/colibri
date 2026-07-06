from colibri.cli import main
from colibri.model.errors import ModelError


def test_ask_prints_fake_response(capsys):
    exit_code = main(["ask", "status"])

    captured = capsys.readouterr()

    assert exit_code == 0
    assert captured.out.strip() == "fake: status"


def test_repl_exits_on_quit(monkeypatch, capsys):
    inputs = iter(["hello", "/quit"])
    monkeypatch.setattr("builtins.input", lambda _: next(inputs))

    exit_code = main(["repl"])

    captured = capsys.readouterr()

    assert exit_code == 0
    assert "fake: hello" in captured.out


def test_main_returns_one_for_expected_model_errors(monkeypatch, capsys):
    class BrokenModel:
        def complete(self, messages, tools, system, limits):
            raise ModelError("boom")

    monkeypatch.setattr("colibri.cli.build_model_client", lambda config: BrokenModel())

    exit_code = main(["ask", "hello"])

    captured = capsys.readouterr()

    assert exit_code == 1
    assert captured.err.strip() == "Model error: boom"
