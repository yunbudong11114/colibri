"""Regression checks for the isolated device entry point and real MCP SDK."""
import asyncio
import json
from contextlib import contextmanager
from pathlib import Path
import os
import socket
import subprocess
import sys
import time

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from worker import App, Settings, mcp_request, pinyin_candidates

@pytest.fixture(autouse=True)
def use_temporary_home(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))


SERVER = """
import sys
from mcp.server.fastmcp import FastMCP

server = FastMCP("fixture", host="127.0.0.1", port=int(sys.argv[2]) if len(sys.argv) > 2 else 0)

@server.tool()
def echo(text: str) -> str:
    return text

@server.resource("fixture://sample")
def sample() -> str:
    return "内容"

@server.prompt()
def greet(name: str) -> str:
    return f"Hello {name}"

server.run(transport=sys.argv[1])
"""


@contextmanager
def fixture_server(tmp_path, transport):
    script = tmp_path / "fixture_server.py"
    script.write_text(SERVER, encoding="utf-8")
    if transport == "stdio":
        yield {"name": "fixture", "command": sys.executable, "args": [str(script), transport]}
        return
    with socket.socket() as reserved:
        reserved.bind(("127.0.0.1", 0))
        port = reserved.getsockname()[1]
    with subprocess.Popen([sys.executable, str(script), transport, str(port)], stderr=subprocess.DEVNULL) as process:
        try:
            for _ in range(150):
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=0.05):
                        break
                except OSError:
                    if process.poll() is not None:
                        pytest.fail("MCP fixture exited")
                    time.sleep(0.05)
            else:
                pytest.fail("MCP fixture did not start")
            yield {"name": "fixture", "url": f"http://127.0.0.1:{port}/mcp"}
        finally:
            process.terminate()
            process.wait(timeout=10)


@pytest.mark.parametrize("transport", ["stdio", "streamable-http"])
def test_mcp_protocol_and_direct_calls(tmp_path, transport):
    with fixture_server(tmp_path, transport) as server:
        tools = asyncio.run(mcp_request(server, "list_tools"))
        assert [tool.name for tool in tools] == ["echo"]
        assert "中文" in str(asyncio.run(mcp_request(server, "call_tool", "echo", {"text": "中文"})))
        assert "内容" in str(asyncio.run(mcp_request(server, "read_resource", "fixture://sample")))
        assert "Hello Mina" in str(asyncio.run(mcp_request(server, "get_prompt", "greet", {"name": "Mina"})))

        app = App(tmp_path / "app")
        app.save("model.provider", "fake")
        app.save("mcp.servers", json.dumps([server]))
        try:
            assert app.ask("你好", pytest.fail) == "fake: 你好"
            tool = app.session.tools.get("mcp.fixture.echo")
            assert tool is not None
            decision = app.session.permission_policy.decide(tool, {"text": "你好"}, None)
            assert decision.allowed
            assert "你好" in app.mcp("fixture", "call_tool", "echo", {"text": "你好"})
        finally:
            app.close()


def test_config_secret_and_concurrent_updates(tmp_path):
    first = Settings(tmp_path)
    second = Settings(tmp_path)
    first.save("model.api_key", "secret-fixture")
    second.save("session.transcript_retention_days", "6")
    assert first.read()["model.api_key"] == "secret-fixture"
    assert first.read()["session.transcript_retention_days"] == 6
    assert "secret-fixture" not in str(first.public())
    assert os.stat(first.path).st_mode & 0o777 == 0o600
    assert "中" in pinyin_candidates("zhong")


def test_repl_uses_the_device_settings(tmp_path):
    worker = Path(__file__).resolve().parents[1] / "worker.py"
    result = subprocess.run(
        [sys.executable, str(worker), "--home", str(tmp_path), "--repl"],
        input="/set model.provider fake\n你好\n/quit\n",
        capture_output=True, text=True, check=True, timeout=10,
    )
    assert "fake: 你好" in result.stdout
    assert Settings(tmp_path).values["model.provider"] == "fake"


def test_voice_transcription_uses_its_own_endpoint_and_key(tmp_path, monkeypatch):
    from http.server import BaseHTTPRequestHandler, HTTPServer
    import threading

    capture = []

    class SpeechHandler(BaseHTTPRequestHandler):
        def do_POST(self):
            body = self.rfile.read(int(self.headers["Content-Length"]))
            capture.append((self.path, self.headers.get("Authorization"), body))
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps({"text": "你好"}).encode())

    arecord = tmp_path / "arecord"
    arecord.write_text(f"""#!{sys.executable}
import sys
import wave
with wave.open(sys.argv[-1], "wb") as audio:
    audio.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
    audio.writeframes(b"fixture0")
""", encoding="utf-8")
    arecord.chmod(0o755)
    monkeypatch.setenv("PATH", f"{tmp_path}{os.pathsep}{os.environ['PATH']}")
    with HTTPServer(("127.0.0.1", 0), SpeechHandler) as server:
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        try:
            app = App(tmp_path / "app")
            app.save("model.api_key", "never-forward")
            app.save("speech.base_url", f"http://127.0.0.1:{server.server_port}/v1")
            app.start_voice()
            _, path, process = app.recording
            process.wait(timeout=5)
            assert app.finish_voice(cancel=True) == ""
            assert not path.exists()
            assert capture == []
            app.start_voice()
            _, path, process = app.recording
            process.wait(timeout=5)
            malformed = bytearray(path.read_bytes())
            malformed[4:8] = (1_920_036).to_bytes(4, "little")
            malformed[40:44] = (1_920_000).to_bytes(4, "little")
            path.write_bytes(malformed)
            assert app.finish_voice() == "你好"
            assert not path.exists()
        finally:
            server.shutdown()
            thread.join(timeout=5)

    [(path, authorization, body)] = capture
    assert path == "/v1/audio/transcriptions"
    assert authorization is None
    assert b"RIFF" + (44).to_bytes(4, "little") + b"WAVE" in body
    assert b"data" + (8).to_bytes(4, "little") + b"fixture0" in body
    assert b"whisper-1" in body


def test_voice_reports_missing_openai_key_before_recording(tmp_path, monkeypatch):
    monkeypatch.delenv("COLIBRI_API_KEY", raising=False)
    app = App(tmp_path)
    with pytest.raises(ValueError, match="缺少语音 API 密钥"):
        app.start_voice()
    assert app.recording is None


@pytest.mark.parametrize("failure,message", [
    ("timeout", "转写连接或响应超时"),
    ("auth", "转写服务拒绝访问"),
])
def test_voice_reports_service_failures(tmp_path, monkeypatch, failure, message):
    import httpx

    app = App(tmp_path)
    app.save("speech.base_url", "https://speech.example.com/v1")
    recording = tmp_path / "speech.wav"
    recording.write_bytes(b"fixture")

    def handle(request):
        if failure == "timeout":
            raise httpx.ConnectTimeout("fixture", request=request)
        return httpx.Response(401)

    client = httpx.AsyncClient
    monkeypatch.setattr(httpx, "AsyncClient", lambda **kwargs: client(
        transport=httpx.MockTransport(handle), **kwargs
    ))
    with pytest.raises(ValueError, match=message):
        app.transcribe(recording)


def test_offline_voice_prefers_local_model_without_a_key(tmp_path, monkeypatch):
    root = tmp_path / "speech"
    root.mkdir()
    (root / "model.int8.onnx").touch()
    (root / "tokens.txt").touch()
    engine = root / "sherpa-onnx"
    engine.write_text("#!/bin/sh\nprintf '%s\\n' '{\"text\":\"你好\"}'\n", encoding="utf-8")
    engine.chmod(0o755)
    monkeypatch.setenv("COLIBRI_OFFLINE_SPEECH", str(root))
    app = App(tmp_path / "settings")
    assert app.offline_speech() == root
    assert app.transcribe(tmp_path / "speech.wav") == "你好"
    app.save("speech.base_url", "https://speech.example.com/v1")
    assert app.offline_speech() is None
    app.save("speech.backend", "offline")
    assert app.offline_speech() == root


@pytest.mark.parametrize("decoded, expected", [
    ("把记忆同步到纳斯", "把记忆同步到 NAS"),
    ("问问纳斯", "问问纳斯"),
    ("打开 MCP 设置", "打开 MCP 设置"),
    ("", ""),
])
def test_ncnn_offline_voice(tmp_path, monkeypatch, decoded, expected):
    engine = tmp_path / "sherpa-ncnn"
    engine.write_text(f"#!/bin/sh\nprintf 'text: %s\\n' '{decoded}'\n", encoding="utf-8")
    engine.chmod(0o755)
    (tmp_path / "tokens.txt").touch()
    for component in ("encoder", "decoder", "joiner"):
        for extension in ("param", "bin"):
            (tmp_path / f"{component}.ncnn.{extension}").touch()
    monkeypatch.setenv("COLIBRI_OFFLINE_SPEECH", str(tmp_path))
    app = App(tmp_path / "settings")
    app.save("speech.backend", "offline")
    assert app.transcribe(tmp_path / "speech.wav") == expected


def test_vision_settings_keep_toml_defaults_and_save_explicit_overrides(tmp_path):
    config = tmp_path / "config.toml"
    config.write_text('[vision]\nmodel = "legacy-vl"\napi_key = "vision-test-secret"\n'
                      'base_url = "https://vision.example/v1"\n', encoding="utf-8")
    settings = Settings(tmp_path)
    settings.save("model.model", "chat-model")
    public = {field["key"]: field for field in settings.public()}
    assert public["vision.model"]["value"] == "legacy-vl"
    assert public["vision.api_key"]["value"] == "•••"
    assert "vision-test-secret" not in str(public)
    assert "vision.model" not in json.loads(settings.path.read_text())

    settings.save("vision.model", "new-vl")
    settings.save("vision.base_url", "")
    settings.save("vision.timeout_seconds", "20")
    reloaded = Settings(tmp_path)
    vision = reloaded.agent_config().vision
    assert (vision.model, vision.base_url, vision.timeout_seconds) == ("new-vl", "", 20)
    assert vision.api_key == "vision-test-secret"
    assert reloaded.agent_config().model.model == "chat-model"
    with pytest.raises(ValueError, match="大于"):
        settings.save("vision.max_image_bytes", "0")


def test_model_form_saves_atomically_and_keeps_key_on_same_address(tmp_path):
    settings = Settings(tmp_path)
    settings.save_model("https://openrouter.ai/api/v1/chat/completions", "openrouter/auto", "fixture-key")
    settings.save_model("https://openrouter.ai/api/v1/", "new/model", None)
    assert settings.values["model.api_key"] == "fixture-key"
    assert settings.values["model.model"] == "new/model"
    with pytest.raises(ValueError, match="API Key"):
        settings.save_model("https://api.deepseek.com", "deepseek-flash", None)
    assert Settings(tmp_path).values["model.base_url"] == "https://openrouter.ai/api/v1"
    settings.save_model("http://192.168.1.4:11434/v1", "local/model", None)
    assert settings.values["model.api_key"] == ""
    assert "fixture-key" not in str(settings.public())


def test_custom_model_sends_no_authorization_when_key_blank(tmp_path, monkeypatch):
    import urllib.request

    app = App(tmp_path)
    app.config.save_model("http://localhost:11434/v1", "local/model", "")
    monkeypatch.setenv("COLIBRI_API_KEY", "unrelated-environment-key")

    class Reply:
        def __enter__(self):
            return self

        def __exit__(self, *_):
            return False

        def read(self):
            return b'{"choices":[{"message":{"content":"ok"}}]}'

    def fetch(request, timeout):
        assert request.full_url == "http://localhost:11434/v1/chat/completions"
        assert not request.has_header("Authorization")
        return Reply()

    monkeypatch.setattr(urllib.request, "urlopen", fetch)
    try:
        assert app.ask("hello", pytest.fail) == "ok"
    finally:
        app.close()


def test_voice_uses_board_microphone_channel_and_limited_gain(tmp_path):
    from array import array
    import tempfile
    import wave

    app = App(tmp_path)
    temp = tempfile.TemporaryDirectory()
    recording = Path(temp.name) / "speech.wav"
    with wave.open(str(recording), "wb") as output:
        output.setparams((2, 2, 16000, 0, "NONE", "not compressed"))
        output.writeframes(array("h", [1000, 5, -500, -5]).tobytes())
    app.recording = (temp, recording, subprocess.Popen(["true"]))

    def inspect(normalized):
        with wave.open(str(normalized), "rb") as audio:
            assert audio.getnchannels() == 1
            assert audio.getnframes() == 2
            assert array("h", audio.readframes(2)).tolist() == [8000, -4000]
        return "测试"

    app.transcribe = inspect
    assert app.finish_voice() == "测试"
    assert not recording.exists()


def test_board_recording_status_requires_audio_frames(tmp_path):
    from types import SimpleNamespace

    wav = tmp_path / "mic.wav"
    wav.write_bytes(b"x" * 48)
    App.wait_for_microphone(wav, SimpleNamespace(poll=lambda: None))
    wav.write_bytes(b"x" * 44)
    with pytest.raises(ValueError, match="未开始采样"):
        App.wait_for_microphone(wav, SimpleNamespace(poll=lambda: 1))


def test_memory_editor_and_session_use_same_files(tmp_path):
    from colibri.messages import ModelResponse

    app = App(tmp_path / "state")
    root = tmp_path / "mem"
    app.save("memory.root", str(root))
    app.save("model.provider", "fake")
    assert app.config.agent_config().memory.enabled
    assert Settings(tmp_path / "fresh").agent_config().memory.root == tmp_path / ".colibri/memory"

    old = app.read_memory("USER.md")
    assert old["revision"] is None
    saved = app.save_memory("USER.md", "# 偏好\n记得称我为测试用户。\n", old["revision"])
    assert app.read_memory("USER.md") == saved
    app.save_memory("topics/project.md", "# Project\n", None)
    assert app.memory_topics() == ["topics/project.md"]
    with pytest.raises(ValueError, match="无效"):
        app.read_memory("topics/../USER.md")
    (root / "USER.md").write_text("外部更新", encoding="utf-8")
    with pytest.raises(ValueError, match="在别处修改"):
        app.save_memory("USER.md", "过期草稿", saved["revision"])
    app.save_memory("USER.md", saved["text"], app.read_memory("USER.md")["revision"])
    (root / "MEMORY.md").symlink_to(tmp_path / "outside.md")
    with pytest.raises(ValueError, match="符号链接"):
        app.read_memory("MEMORY.md")

    prompts = []

    class Model:
        def complete(self, messages, tools, system, limits):
            prompts.append(("\n".join(m.content for m in messages), {t["function"]["name"] for t in tools}))
            return ModelResponse(text="ok")

    try:
        app.setup()
        app.session.model = Model()
        assert app.ask("你好", pytest.fail) == "ok"
        assert "记得称我为测试用户" in prompts[-1][0]
        assert {"memory.list", "memory.read", "memory.search"} <= prompts[-1][1]
        app.save("memory.enabled", "false")
        app.session.model = Model()
        app.ask("继续", pytest.fail)
        assert "记得称我为测试用户" not in prompts[-1][0]
        assert {"memory.read", "memory.write"}.isdisjoint(prompts[-1][1])
    finally:
        app.close()


def test_chat_memory_write_runs_directly(tmp_path):
    from colibri.messages import ModelResponse, ToolCall

    app = App(tmp_path / "state")
    root = tmp_path / "memory"
    app.save("memory.root", str(root))
    app.save("model.provider", "fake")
    arguments = {"file": "USER.md", "content": "记住这条测试偏好", "mode": "replace"}

    class Model:
        def complete(self, messages, tools, system, limits):
            if messages[-1].role == "tool":
                return ModelResponse(text="done")
            assert "memory.write" in {tool["function"]["name"] for tool in tools}
            return ModelResponse(text="", tool_calls=[ToolCall("write-1", "memory.write", arguments)])

    try:
        app.setup()
        app.session.model = Model()
        assert app.ask("把这条记住", pytest.fail) == "done"
        assert arguments["content"] in (root / "USER.md").read_text()
    finally:
        app.close()


def test_default_home_migrates_without_overwriting_existing_files(tmp_path):
    from worker import migrate_device_home

    legacy = tmp_path / ".local/share/colibri-cardputer"
    home = tmp_path / ".colibri"
    for directory in (legacy, home):
        (directory / "transcripts").mkdir(parents=True)
    (legacy / "settings.json").write_text('{"model.provider":"fake"}', encoding="utf-8")
    (legacy / "transcripts" / "same.jsonl").write_text("legacy", encoding="utf-8")
    (legacy / "transcripts" / "old.jsonl").write_text("old session", encoding="utf-8")
    (home / "transcripts" / "same.jsonl").write_text("original", encoding="utf-8")
    migrate_device_home(legacy, home)
    assert Settings(home).values["model.provider"] == "fake"
    assert (home / "transcripts" / "same.jsonl").read_text() == "original"
    assert (home / "transcripts" / "old.jsonl").read_text() == "old session"
    assert (legacy / "settings.json").exists()
    assert (home / "settings.json").stat().st_mode & 0o777 == 0o600
    assert (home / ".cardputer-layout-v1").exists()


def test_capability_catalog_uses_metadata_without_starting_servers(tmp_path):
    app = App(tmp_path)
    app.save("mcp.servers", json.dumps([
        {"name": "local", "command": "should-not-start", "description": "Local tools"},
        {"name": "remote", "url": "https://example.com/mcp", "token": "secret"},
    ]))
    skill = tmp_path / "skills" / "example" / "SKILL.md"
    skill.parent.mkdir(parents=True)
    skill.write_text(
        "---\nname: example\ndescription: Short description\n"
        "commands:\n  - name: test\n    command: /bin/false\n---\nPrivate body\n",
        encoding="utf-8",
    )
    assert app.catalog("mcp") == [
        {"name": "local", "description": "Local tools"},
        {"name": "remote", "description": "Streamable HTTP · 未填写说明"},
    ]
    assert {"name": "example", "description": "Short description"} in app.catalog("skills")

def test_cardputer_builtin_tools_and_transcript_access(tmp_path):
    from colibri.tools.base import ToolContext

    app = App(tmp_path)
    app.save("model.provider", "fake")
    app.save("session.transcript", "false")
    transcript = tmp_path / "transcripts" / "example.jsonl"
    transcript.parent.mkdir()
    transcript.write_text("transcript fixture", encoding="utf-8")
    skill = tmp_path / "skills" / "example" / "SKILL.md"
    skill.parent.mkdir(parents=True)
    skill.write_text(
        "---\nname: example\ndescription: Fixture\ncommands:\n"
        "  - name: hello\n    command: echo\n    args: [skill fixture]\n---\n",
        encoding="utf-8",
    )
    try:
        app.ask("你好", pytest.fail)
        registry = app.session.tools
        expected = {
            "files.list", "files.read", "files.write", "files.send", "shell.run",
            "memory.list", "memory.read", "memory.search", "memory.write", "web.search",
            "image.understand", "skill.read", "skill.run", "hardware.probe",
            "hardware.devices", "serial.read", "serial.write", "gpio.read", "gpio.write",
            "i2c.scan", "i2c.read", "i2c.write", "spi.transfer",
        }
        assert {tool["function"]["name"] for tool in registry.specs()} == expected
        context = ToolContext(config=app.session.config, cwd=registry.cwd)
        for name, arguments, expected_text in [
            ("files.list", {"path": "transcripts"}, "example.jsonl"),
            ("files.read", {"path": str(transcript)}, "transcript fixture"),
            ("shell.run", {"command": "ls transcripts"}, "example.jsonl"),
            ("skill.run", {"skill": "example", "command": "hello"}, "skill fixture"),
        ]:
            tool = registry.get(name)
            assert app.session.permission_policy.decide(tool, arguments, context).allowed
            result = tool.run(arguments, context)
            assert result.ok and expected_text in result.text
    finally:
        app.close()


def test_missing_model_key_points_to_model_settings(tmp_path, monkeypatch):
    from colibri.config import ConfigError
    from worker import error_message

    monkeypatch.delenv("COLIBRI_API_KEY", raising=False)
    app = App(tmp_path)
    with pytest.raises(ConfigError) as failure:
        app.ask("你好", pytest.fail)
    message = error_message(failure.value)
    assert "模型 API Key" in message and "设置 → 模型" in message


def test_model_status_detects_missing_config_without_opening_a_session(tmp_path, monkeypatch):
    monkeypatch.delenv("COLIBRI_API_KEY", raising=False)
    app = App(tmp_path)
    assert "模型 API Key" in app.model_hint()
    assert app.session is None
    assert not (tmp_path / "transcripts").exists()
    app.save("model.provider", "fake")
    assert app.model_hint() == ""


def test_cardputer_search_does_not_prompt_for_read_only_query():
    from colibri.tools.builtin.web import WebSearchTool
    from worker import CardputerToolPolicy

    assert CardputerToolPolicy().decide(WebSearchTool(), {"query": "test"}, None).allowed
