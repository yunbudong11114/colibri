from cardputer_agent.cli import main


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
