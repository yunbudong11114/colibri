from pathlib import Path
import hashlib
import json

import pytest

from colibri.channels.base import InboundMessage
from colibri.channels.weixin import WeixinApiClient, WeixinChannel, WeixinPermissionPrompter, perform_weixin_auth
from colibri.config import AgentConfig, WeixinChannelConfig
from colibri.gateway import GatewayRunner, GatewaySessionCache
from colibri.media import MediaPart
from colibri.messages import ModelResponse, ToolCall
from colibri.model.fake import FakeModelClient
from colibri.tools.permissions import PermissionRequest, PermissionSubject
from colibri.tools.registry import ToolRegistry


@pytest.fixture(autouse=True)
def isolate_colibri_home(monkeypatch, tmp_path):
    monkeypatch.setenv("COLIBRI_HOME", str(tmp_path / "home"))


class FakeWeixinApi:
    def __init__(self, updates=None):
        self.updates = list(updates or [])
        self.sent = []

    def get_updates(self, get_updates_buf):
        if self.updates:
            return self.updates.pop(0)
        return {"get_updates_buf": get_updates_buf, "msgs": []}

    def send_text(self, to_user_id, context_token, text):
        self.sent.append((to_user_id, context_token, text))
        return {"ret": 0, "errcode": 0}

    def upload_media(self, path, media_type, to_user_id=""):
        self.sent.append(("upload_media", path, media_type, to_user_id))
        return {"media_id": "media-1", "aes_key": "key", "file_size": 5}

    def send_media(self, to_user_id, context_token, media_type, uploaded, filename, caption):
        self.sent.append(("send_media", to_user_id, context_token, media_type, uploaded, filename, caption))
        return {"ret": 0, "errcode": 0}


class FakeChannel:
    def __init__(self, name, messages):
        self.name = name
        self.messages = messages
        self.replies = []

    def run(self, handler, context):
        for message in self.messages:
            self.replies.append(handler(message))

    def send_text(self, recipient_id, text):
        self.replies.append(text)

    def send_media(self, recipient_id, media):
        self.replies.append((recipient_id, media))


class ToolCallModel:
    def __init__(self, tool_name, arguments):
        self.tool_name = tool_name
        self.arguments = arguments
        self.calls = 0

    def complete(self, messages, tools, system, limits):
        self.calls += 1
        if self.calls == 1:
            return ModelResponse(
                text="",
                tool_calls=[ToolCall(id="call_1", name=self.tool_name, arguments=self.arguments)],
            )
        last_tool = next((message.content for message in reversed(messages) if message.role == "tool"), "")
        return ModelResponse(text=f"final: {last_tool}")


def test_weixin_channel_parses_allowed_finished_text_message():
    api = FakeWeixinApi(
        [
            {
                "get_updates_buf": "next",
                "msgs": [
                    {
                        "message_type": 1,
                        "message_state": 2,
                        "from_user_id": "user-1",
                        "message_id": 42,
                        "context_token": "ctx-1",
                        "item_list": [{"type": 1, "text_item": {"text": "hello"}}],
                    }
                ],
            }
        ]
    )
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token", allow_from=["user-1"]), api=api)

    messages = channel.poll_messages()

    assert messages == [InboundMessage(channel="weixin", sender_id="user-1", text="hello", message_id="42")]
    assert channel.get_updates_buf == "next"
    assert channel.context_tokens["user-1"] == "ctx-1"


def test_weixin_channel_ignores_disallowed_sender():
    api = FakeWeixinApi(
        [
            {
                "msgs": [
                    {
                        "message_type": 1,
                        "message_state": 2,
                        "from_user_id": "user-2",
                        "item_list": [{"type": 1, "text_item": {"text": "hello"}}],
                    }
                ]
            }
        ]
    )
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token", allow_from=["user-1"]), api=api)

    assert channel.poll_messages() == []


def test_weixin_channel_sends_text_with_context_token():
    api = FakeWeixinApi()
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token"), api=api)
    channel.context_tokens["user-1"] = "ctx-1"

    channel.send_text("user-1", "reply")

    assert api.sent == [("user-1", "ctx-1", "reply")]


def test_weixin_channel_sends_media_with_context_token(tmp_path):
    path = tmp_path / "report.txt"
    path.write_text("hello", encoding="utf-8")
    api = FakeWeixinApi()
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token"), api=api)
    channel.context_tokens["user-1"] = "ctx-1"

    channel.send_media(
        "user-1",
        MediaPart(
            type="file",
            path=path,
            filename="report.txt",
            content_type="text/plain",
            caption="请看",
        ),
    )

    assert api.sent == [
        ("upload_media", path, "file", "user-1"),
        (
            "send_media",
            "user-1",
            "ctx-1",
            "file",
            {"media_id": "media-1", "aes_key": "key", "file_size": 5},
            "report.txt",
            "请看",
        ),
    ]


def test_weixin_api_upload_media_encrypts_and_uploads_file(monkeypatch, tmp_path):
    path = tmp_path / "report.txt"
    path.write_bytes(b"hello")
    posts = []

    class FakeHTTPResponse:
        def __init__(self, body=b"{}", headers=None):
            self.body = body
            self.headers = headers or {}

        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, traceback):
            return False

        def read(self):
            return self.body

        def getheader(self, name, default=None):
            return self.headers.get(name, default)

    def fake_urlopen(request, timeout):
        if request.full_url.endswith("/ilink/bot/getuploadurl"):
            body = json.loads(request.data.decode("utf-8"))
            posts.append(("getuploadurl", body))
            return FakeHTTPResponse(
                json.dumps({"ret": 0, "errcode": 0, "upload_full_url": "https://cdn.example/upload"}).encode(
                    "utf-8"
                )
            )
        if request.full_url == "https://cdn.example/upload":
            posts.append(("cdn_upload", request.data, dict(request.header_items())))
            return FakeHTTPResponse(headers={"X-Encrypted-Param": "download-param"})
        raise AssertionError(request.full_url)

    monkeypatch.setattr("colibri.channels.weixin.urllib.request.urlopen", fake_urlopen)
    monkeypatch.setattr("colibri.channels.weixin.secrets.token_bytes", lambda n: b"\x01" * n)
    monkeypatch.setattr("colibri.channels.weixin.secrets.token_hex", lambda n: "f" * (n * 2))
    api = WeixinApiClient(base_url="https://weixin.example/", token="token")

    uploaded = api.upload_media(path, "file", to_user_id="user-1")

    getuploadurl = posts[0][1]
    assert getuploadurl["filekey"] == "f" * 32
    assert getuploadurl["media_type"] == 3
    assert getuploadurl["to_user_id"] == "user-1"
    assert getuploadurl["rawsize"] == 5
    assert getuploadurl["rawfilemd5"] == hashlib.md5(b"hello").hexdigest()
    assert getuploadurl["filesize"] == 16
    assert getuploadurl["no_need_thumb"] is True
    assert getuploadurl["aeskey"] == "01" * 16
    cdn_payload = posts[1][1]
    assert cdn_payload != b"hello"
    assert len(cdn_payload) == 16
    assert posts[1][2]["Content-type"] == "application/octet-stream"
    assert uploaded["download_param"] == "download-param"
    assert uploaded["aes_key"] == "01" * 16
    assert uploaded["file_size"] == 5
    assert uploaded["cipher_size"] == 16
    assert uploaded["filename"] == "report.txt"


def test_weixin_api_send_media_uses_uploaded_metadata(monkeypatch):
    posts = []

    class FakeHTTPResponse:
        def __init__(self, body=b'{"ret":0,"errcode":0}'):
            self.body = body

        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, traceback):
            return False

        def read(self):
            return self.body

    def fake_urlopen(request, timeout):
        posts.append(json.loads(request.data.decode("utf-8")))
        return FakeHTTPResponse()

    monkeypatch.setattr("colibri.channels.weixin.urllib.request.urlopen", fake_urlopen)
    api = WeixinApiClient(base_url="https://weixin.example/", token="token")

    api.send_media(
        "user-1",
        "ctx-1",
        "file",
        {
            "download_param": "download-param",
            "aes_key": "01" * 16,
            "file_size": 5,
            "cipher_size": 16,
        },
        "report.txt",
        "请看",
    )

    assert posts[0]["msg"]["item_list"][0] == {"type": 1, "text_item": {"text": "请看"}}
    item = posts[1]["msg"]["item_list"][0]
    assert item["type"] == 4
    assert item["file_item"]["file_name"] == "report.txt"
    assert item["file_item"]["len"] == "5"
    assert item["file_item"]["media"] == {
        "encrypt_query_param": "download-param",
        "aes_key": "MDEwMTAxMDEwMTAxMDEwMTAxMDEwMTAxMDEwMTAxMDE=",
        "encrypt_type": 1,
    }


def test_weixin_permission_prompter_sends_prompt_and_maps_reply():
    api = FakeWeixinApi(
        [
            {
                "msgs": [
                    {
                        "message_type": 1,
                        "message_state": 2,
                        "from_user_id": "user-1",
                        "item_list": [{"type": 1, "text_item": {"text": "p"}}],
                    }
                ]
            }
        ]
    )
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token"), api=api)
    request = PermissionRequest(
        tool_name="shell.run",
        arguments={"command": "pwd"},
        read_only=False,
        subject=PermissionSubject(kind="shell", tool_name="shell.run", shell_command="pwd"),
    )

    choice = WeixinPermissionPrompter(channel, "user-1", timeout_seconds=1).confirm(request)

    assert choice == "p"
    assert api.sent[0][0] == "user-1"
    assert "shell.run" in api.sent[0][2]
    assert "pwd" in api.sent[0][2]


def test_weixin_permission_prompt_uses_absolute_file_path_and_summarizes_content(tmp_path):
    api = FakeWeixinApi(
        [
            {
                "msgs": [
                    {
                        "message_type": 1,
                        "message_state": 2,
                        "from_user_id": "user-1",
                        "item_list": [{"type": 1, "text_item": {"text": "y"}}],
                    }
                ]
            }
        ]
    )
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token"), api=api)
    content = 'print("Hello, World!")\n'
    request = PermissionRequest(
        tool_name="files.write",
        arguments={"path": "hello_world.py", "content": content},
        read_only=False,
        subject=PermissionSubject(
            kind="file_path",
            tool_name="files.write",
            file_path=str((tmp_path / "hello_world.py").resolve()),
            file_root=str(tmp_path.resolve()),
            read_only=False,
        ),
    )

    choice = WeixinPermissionPrompter(channel, "user-1", timeout_seconds=1).confirm(request)

    prompt = api.sent[0][2]
    assert choice == "y"
    assert f"path: {(tmp_path / 'hello_world.py').resolve()}" in prompt
    assert "content:" in prompt
    assert "chars" in prompt
    assert "bytes" in prompt
    assert content not in prompt


def test_gateway_session_cache_reuses_and_evicts_oldest(tmp_path):
    config = AgentConfig.default()
    registry = ToolRegistry.from_config(config, cwd=tmp_path)
    times = iter([0.0, 1.0, 2.0])
    cache = GatewaySessionCache(
        config=config,
        model=FakeModelClient(),
        registry=registry,
        max_sessions=1,
        idle_seconds=0,
        monotonic_func=lambda: next(times),
    )

    first = cache.get("weixin:user-1", policy=None)
    second = cache.get("weixin:user-1", policy=None)
    third = cache.get("weixin:user-2", policy=None)

    assert first is second
    assert third is not first


def test_gateway_runner_handles_message_with_weixin_permission_policy(tmp_path):
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "allow"}})
    api = FakeWeixinApi()
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token"), api=api)
    runner = GatewayRunner(
        config=config,
        model=FakeModelClient(),
        registry=ToolRegistry.from_config(config, cwd=tmp_path),
    )

    reply = runner.handle_message(channel, InboundMessage(channel="weixin", sender_id="user-1", text="hi"))

    assert reply == "fake: hi"


def test_gateway_runner_sends_file_tool_media_through_channel(tmp_path):
    path = tmp_path / "report.txt"
    path.write_text("hello", encoding="utf-8")
    config = AgentConfig.default().with_overrides(
        {
            "files": {"roots": [str(tmp_path)]},
            "tools": {"default_permission": "allow", "max_result_chars": 1000},
            "session": {"max_tool_rounds": 3},
        }
    )
    channel = FakeChannel("weixin", [])
    runner = GatewayRunner(
        config=config,
        model=ToolCallModel("files.send", {"path": "report.txt", "caption": "请看"}),
        registry=ToolRegistry.from_config(config, cwd=tmp_path),
    )

    reply = runner.handle_message(channel, InboundMessage(channel="weixin", sender_id="user-1", text="send"))

    assert reply == "final: Sent file to channel: report.txt"
    assert channel.replies == [
        (
            "user-1",
            MediaPart(
                type="file",
                path=path.resolve(),
                filename="report.txt",
                content_type="text/plain",
                caption="请看",
            ),
        )
    ]


def test_gateway_runner_writes_channel_metadata_to_transcript(tmp_path, monkeypatch):
    monkeypatch.setenv("COLIBRI_HOME", str(tmp_path / "home"))
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "allow"}})
    channel = FakeChannel("weixin", [])
    runner = GatewayRunner(
        config=config,
        model=FakeModelClient(),
        registry=ToolRegistry.from_config(config, cwd=tmp_path),
    )

    reply = runner.handle_message(channel, InboundMessage(channel="weixin", sender_id="user-1", text="hi"))
    runner.sessions.close()

    transcript_files = list((tmp_path / "home" / "transcripts").glob("*.jsonl"))
    assert reply == "fake: hi"
    assert len(transcript_files) == 1
    event = json.loads(transcript_files[0].read_text(encoding="utf-8").splitlines()[0])
    assert event["type"] == "user_message"
    assert event["payload"]["text"] == "hi"
    assert event["payload"]["channel"] == "weixin"
    assert event["payload"]["sender_id"] == "user-1"
    assert event["payload"]["session_key"] == "weixin:user-1"


def test_gateway_runner_runs_all_enabled_channels(tmp_path, monkeypatch):
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "allow"}})
    first = FakeChannel("first", [InboundMessage(channel="first", sender_id="user-1", text="one")])
    second = FakeChannel("second", [InboundMessage(channel="second", sender_id="user-2", text="two")])
    runner = GatewayRunner(
        config=config,
        model=FakeModelClient(),
        registry=ToolRegistry.from_config(config, cwd=tmp_path),
    )
    monkeypatch.setattr(runner, "_build_channels", lambda: [first, second])

    runner.run()

    assert first.replies == ["fake: one"]
    assert second.replies == ["fake: two"]


def test_perform_weixin_auth_prints_terminal_qr(monkeypatch):
    qr_payload = "https://liteapp.weixin.qq.com/q/7GiQu1?qrcode=4b69ff82f873485e97acae885b11437c&bot_type=3"

    class FakeAuthApi:
        def __init__(self, base_url, timeout_seconds):
            pass

        def get_qrcode(self):
            return {
                "qrcode_img_content": qr_payload,
                "qrcode": "qr-1",
            }

        def get_qrcode_status(self, qrcode):
            return {
                "status": "confirmed",
                "bot_token": "token",
                "ilink_bot_id": "bot-1",
                "ilink_user_id": "user-1",
                "baseurl": "https://redirect.weixin.test/",
            }

    lines = []
    monkeypatch.setattr("colibri.channels.weixin.WeixinApiClient", FakeAuthApi)

    result = perform_weixin_auth("https://ilinkai.weixin.qq.com/", timeout_seconds=1, print_func=lines.append)

    output = "\n".join(lines)
    assert result.token == "token"
    assert "Scan this Weixin QR code with WeChat:" in output
    assert "██" in output
    assert "QR payload:" in output
    assert qr_payload in output
