import base64
from pathlib import Path
import hashlib
import json
import os
import threading

import pytest

import colibri.channels.weixin as weixin_module
from colibri.channels.base import ChannelContext, InboundMessage
from colibri.channels.weixin import (
    WeixinApiClient,
    WeixinChannel,
    WeixinPermissionPrompter,
    _encrypt_aes_ecb,
    perform_weixin_auth,
)
from colibri.config import AgentConfig, WeixinChannelConfig
from colibri.gateway import GatewayRunner, GatewaySessionCache
from colibri.media import MediaPart
from colibri.messages import Message, ModelResponse, ToolCall
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

    def download_inbound_media(self, item):
        if item["type"] == 2:
            return MediaPart(
                type="image",
                path=Path("/tmp/colibri/media/image.png"),
                filename="image.png",
                content_type="image/png",
            )
        filename = item["file_item"]["file_name"]
        return MediaPart(
            type="file",
            path=Path("/tmp/colibri/media") / filename,
            filename=filename,
            content_type="text/plain",
        )


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


def test_weixin_channel_run_dispatches_text_and_image_separately_from_same_poll():
    api = FakeWeixinApi(
        [
            {
                "get_updates_buf": "next",
                "msgs": [
                    {
                        "message_type": 1,
                        "message_state": 2,
                        "from_user_id": "user-1",
                        "message_id": "text-1",
                        "context_token": "ctx-1",
                        "item_list": [{"type": 1, "text_item": {"text": "please inspect"}}],
                    },
                    {
                        "message_type": 1,
                        "message_state": 2,
                        "from_user_id": "user-1",
                        "message_id": "image-1",
                        "context_token": "ctx-1",
                        "item_list": [
                            {
                                "type": 2,
                                "image_item": {"media": {"encrypt_query_param": "download-token"}},
                            }
                        ],
                    },
                ],
            }
        ]
    )
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token"), api=api)
    received = []

    channel.run(
        lambda message: received.append(message) or "done",
        ChannelContext(stop_requested=lambda: bool(received)),
    )

    assert [message.text for message in received] == ["please inspect", "[image: image.png]"]
    assert [message.message_id for message in received] == ["text-1", "image-1"]
    assert received[0].media == []
    assert received[1].media == [
        MediaPart(
            type="image",
            path=Path("/tmp/colibri/media/image.png"),
            filename="image.png",
            content_type="image/png",
        )
    ]
    assert api.sent == [("user-1", "ctx-1", "done"), ("user-1", "ctx-1", "done")]


def test_weixin_channel_run_dispatches_text_and_image_separately_across_polls():
    class CountingApi(FakeWeixinApi):
        def __init__(self, updates):
            super().__init__(updates)
            self.poll_count = 0

        def get_updates(self, get_updates_buf):
            self.poll_count += 1
            return super().get_updates(get_updates_buf)

    api = CountingApi(
        [
            {
                "get_updates_buf": "text",
                "msgs": [
                    {
                        "message_type": 1,
                        "message_state": 2,
                        "from_user_id": "user-1",
                        "message_id": "text-1",
                        "context_token": "ctx-1",
                        "item_list": [{"type": 1, "text_item": {"text": "please inspect"}}],
                    }
                ],
            },
            {
                "get_updates_buf": "image",
                "msgs": [
                    {
                        "message_type": 1,
                        "message_state": 2,
                        "from_user_id": "user-1",
                        "message_id": "image-1",
                        "context_token": "ctx-1",
                        "item_list": [
                            {
                                "type": 2,
                                "image_item": {"media": {"encrypt_query_param": "download-token"}},
                            }
                        ],
                    }
                ],
            },
        ]
    )
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token"), api=api)
    received = []

    channel.run(
        lambda message: received.append(message) or "done",
        ChannelContext(stop_requested=lambda: api.poll_count >= 2 and bool(received)),
    )

    assert [message.text for message in received] == ["please inspect", "[image: image.png]"]
    assert received[0].media == []
    assert received[1].media[0].type == "image"
    assert api.sent == [("user-1", "ctx-1", "done"), ("user-1", "ctx-1", "done")]


def test_weixin_channel_run_keeps_two_text_messages_separate():
    api = FakeWeixinApi(
        [
            {
                "get_updates_buf": "next",
                "msgs": [
                    {
                        "message_type": 1,
                        "message_state": 2,
                        "from_user_id": "user-1",
                        "message_id": "text-1",
                        "context_token": "ctx-1",
                        "item_list": [{"type": 1, "text_item": {"text": "first"}}],
                    },
                    {
                        "message_type": 1,
                        "message_state": 2,
                        "from_user_id": "user-1",
                        "message_id": "text-2",
                        "context_token": "ctx-1",
                        "item_list": [{"type": 1, "text_item": {"text": "second"}}],
                    },
                ],
            }
        ]
    )
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token"), api=api)
    received = []

    channel.run(
        lambda message: received.append(message) or message.text,
        ChannelContext(stop_requested=lambda: len(received) == 2),
    )

    assert [message.text for message in received] == ["first", "second"]
    assert [sent[2] for sent in api.sent] == ["first", "second"]


def test_weixin_channel_routes_permission_reply_to_active_waiter():
    prompt_sent = threading.Event()

    class PermissionReplyApi(FakeWeixinApi):
        def __init__(self):
            super().__init__()
            self.calls = 0

        def get_updates(self, get_updates_buf):
            self.calls += 1
            if self.calls == 1:
                return {
                    "get_updates_buf": "request",
                    "msgs": [
                        {
                            "message_type": 1,
                            "message_state": 2,
                            "from_user_id": "user-1",
                            "context_token": "ctx-1",
                            "item_list": [{"type": 1, "text_item": {"text": "run it"}}],
                        }
                    ],
                }
            if self.calls == 2:
                assert prompt_sent.wait(timeout=1)
                return {
                    "get_updates_buf": "permission",
                    "msgs": [
                        {
                            "message_type": 1,
                            "message_state": 2,
                            "from_user_id": "user-1",
                            "context_token": "ctx-1",
                            "item_list": [{"type": 1, "text_item": {"text": "1"}}],
                        }
                    ],
                }
            return {"get_updates_buf": get_updates_buf, "msgs": []}

        def send_text(self, to_user_id, context_token, text):
            result = super().send_text(to_user_id, context_token, text)
            if "Colibri wants to run" in text:
                prompt_sent.set()
            return result

    api = PermissionReplyApi()
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token"), api=api)
    choices = []
    request = PermissionRequest(
        tool_name="shell.run",
        arguments={"command": "pwd"},
        read_only=False,
        subject=PermissionSubject(kind="shell", tool_name="shell.run", shell_command="pwd"),
    )

    def handler(message):
        choices.append(WeixinPermissionPrompter(channel, "user-1", timeout_seconds=1).confirm(request))
        return "done"

    channel.run(handler, ChannelContext(stop_requested=lambda: bool(choices)))

    assert choices == ["1"]
    assert len(api.sent) == 2


def test_weixin_channel_worker_failure_does_not_deadlock_when_queue_is_full():
    messages = []
    for index in range(10):
        messages.append(
            {
                "message_type": 1,
                "message_state": 2,
                "from_user_id": "user-1",
                "message_id": f"message-{index}",
                "context_token": "ctx-1",
                "item_list": [
                    {"type": 1, "text_item": {"text": f"inspect {index}"}},
                    {
                        "type": 2,
                        "image_item": {"media": {"encrypt_query_param": f"download-{index}"}},
                    },
                ],
            }
        )
    api = FakeWeixinApi([{"get_updates_buf": "next", "msgs": messages}])
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token"), api=api)
    errors = []

    def run_channel():
        try:
            channel.run(
                lambda _message: (_ for _ in ()).throw(RuntimeError("worker failed")),
                ChannelContext(),
            )
        except BaseException as error:
            errors.append(error)

    thread = threading.Thread(target=run_channel, daemon=True)
    thread.start()
    thread.join(timeout=1)

    assert not thread.is_alive()
    assert len(errors) == 1
    assert isinstance(errors[0], RuntimeError)
    assert str(errors[0]) == "worker failed"


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


def test_weixin_channel_parses_inbound_file_media_message():
    api = FakeWeixinApi(
        [
            {
                "get_updates_buf": "next",
                "msgs": [
                    {
                        "message_type": 1,
                        "message_state": 2,
                        "from_user_id": "user-1",
                        "message_id": 43,
                        "context_token": "ctx-1",
                        "item_list": [
                            {
                                "type": 4,
                                "file_item": {
                                    "file_name": "report.txt",
                                    "media": {"encrypt_query_param": "download-token"},
                                },
                            }
                        ],
                    }
                ],
            }
        ]
    )
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token"), api=api)

    messages = channel.poll_messages()

    assert len(messages) == 1
    assert messages[0].text == "[file: report.txt]"
    assert messages[0].media == [
        MediaPart(type="file", path=Path("/tmp/colibri/media/report.txt"), filename="report.txt", content_type="text/plain")
    ]


def test_weixin_channel_parses_inbound_image_media_message():
    api = FakeWeixinApi(
        [
            {
                "msgs": [
                    {
                        "message_type": 1,
                        "message_state": 2,
                        "from_user_id": "user-1",
                        "item_list": [
                            {
                                "type": 2,
                                "image_item": {"media": {"encrypt_query_param": "download-token"}},
                            }
                        ],
                    }
                ],
            }
        ]
    )
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token"), api=api)

    messages = channel.poll_messages()

    assert len(messages) == 1
    assert messages[0].text == "[image: image.png]"
    assert messages[0].media == [
        MediaPart(type="image", path=Path("/tmp/colibri/media/image.png"), filename="image.png", content_type="image/png")
    ]


def test_weixin_channel_keeps_text_when_inbound_media_download_fails():
    class FailingMediaApi(FakeWeixinApi):
        def download_inbound_media(self, item):
            raise RuntimeError("download failed")

    api = FailingMediaApi(
        [
            {
                "msgs": [
                    {
                        "message_type": 1,
                        "message_state": 2,
                        "from_user_id": "user-1",
                        "item_list": [
                            {"type": 1, "text_item": {"text": "hello"}},
                            {
                                "type": 4,
                                "file_item": {
                                    "file_name": "report.txt",
                                    "media": {"encrypt_query_param": "download-token"},
                                },
                            },
                        ],
                    }
                ],
            }
        ]
    )
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token"), api=api)

    messages = channel.poll_messages()

    assert messages == [InboundMessage(channel="weixin", sender_id="user-1", text="hello")]


def test_weixin_api_download_inbound_file_decrypts_and_stores_media(monkeypatch, tmp_path):
    plaintext = b"hello"
    key = b"\x01" * 16
    ciphertext = _encrypt_aes_ecb(plaintext, key)
    downloads = []

    class FakeHTTPResponse:
        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, traceback):
            return False

        def read(self):
            return ciphertext

    def fake_urlopen(request, timeout):
        downloads.append(request.full_url)
        return FakeHTTPResponse()

    monkeypatch.setattr("colibri.channels.weixin.urllib.request.urlopen", fake_urlopen)
    monkeypatch.setattr("colibri.channels.weixin.secrets.token_hex", lambda n: "a" * (n * 2))
    monkeypatch.setattr("colibri.channels.weixin.MEDIA_TEMP_DIR", tmp_path / "media")
    api = WeixinApiClient(base_url="https://weixin.example/", token="token")
    item = {
        "type": 4,
        "file_item": {
            "file_name": "report.txt",
            "media": {
                "encrypt_query_param": "download-token",
                "aes_key": base64.b64encode(("01" * 16).encode("ascii")).decode("ascii"),
            },
        },
    }

    media = api.download_inbound_media(item)

    assert downloads == [
        "https://novac2c.cdn.weixin.qq.com/c2c/download?encrypted_query_param=download-token"
    ]
    expected_path = tmp_path / "media" / ("weixin-inbound-" + "a" * 16 + ".txt")
    assert media == MediaPart(type="file", path=expected_path, filename="report.txt", content_type="text/plain")
    assert media.path.read_bytes() == plaintext


def test_cleanup_media_directory_removes_expired_files(tmp_path):
    old_file = tmp_path / "old.png"
    new_file = tmp_path / "new.png"
    old_file.write_bytes(b"old")
    new_file.write_bytes(b"new")
    os.utime(old_file, (10, 10))
    os.utime(new_file, (90, 90))

    weixin_module._cleanup_media_directory(
        tmp_path,
        now=100,
        retention_seconds=50,
        max_total_bytes=100,
    )

    assert not old_file.exists()
    assert new_file.exists()


def test_cleanup_media_directory_removes_oldest_files_to_fit_budget(tmp_path):
    oldest = tmp_path / "oldest.bin"
    middle = tmp_path / "middle.bin"
    newest = tmp_path / "newest.bin"
    for path, modified in ((oldest, 10), (middle, 20), (newest, 30)):
        path.write_bytes(b"xxxx")
        os.utime(path, (modified, modified))

    weixin_module._cleanup_media_directory(
        tmp_path,
        now=40,
        retention_seconds=100,
        max_total_bytes=8,
    )

    assert not oldest.exists()
    assert middle.exists()
    assert newest.exists()


def test_cleanup_media_directory_ignores_delete_errors(monkeypatch, tmp_path):
    blocked = tmp_path / "blocked.bin"
    removable = tmp_path / "removable.bin"
    blocked.write_bytes(b"x")
    removable.write_bytes(b"x")
    os.utime(blocked, (10, 10))
    os.utime(removable, (10, 10))
    original_unlink = Path.unlink

    def selective_unlink(path, *args, **kwargs):
        if path == blocked:
            raise OSError("busy")
        return original_unlink(path, *args, **kwargs)

    monkeypatch.setattr(Path, "unlink", selective_unlink)

    weixin_module._cleanup_media_directory(
        tmp_path,
        now=100,
        retention_seconds=50,
        max_total_bytes=100,
    )

    assert blocked.exists()
    assert not removable.exists()


def test_write_inbound_media_reserves_space_for_new_file(monkeypatch, tmp_path):
    existing = tmp_path / "existing.bin"
    existing.write_bytes(b"12345678")
    os.utime(existing, (10, 10))
    monkeypatch.setattr(weixin_module, "MEDIA_TEMP_DIR", tmp_path)
    monkeypatch.setattr(weixin_module, "MEDIA_MAX_TOTAL_BYTES", 8)
    monkeypatch.setattr(weixin_module, "MEDIA_RETENTION_SECONDS", 1_000_000)
    monkeypatch.setattr(weixin_module, "_last_media_cleanup_at", 0.0)
    monkeypatch.setattr(weixin_module.time, "time", lambda: 100.0)

    written = weixin_module._write_inbound_media_file("new.bin", b"abcd")

    assert written.read_bytes() == b"abcd"
    assert not existing.exists()
    assert sum(path.stat().st_size for path in tmp_path.iterdir()) <= 8


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
                        "item_list": [{"type": 1, "text_item": {"text": "5"}}],
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

    assert choice == "5"
    assert api.sent[0][0] == "user-1"
    assert "shell.run" in api.sent[0][2]
    assert "pwd" in api.sent[0][2]
    assert "1. once" in api.sent[0][2]
    assert "5. user-executable" in api.sent[0][2]


def test_weixin_permission_prompt_uses_absolute_file_path_and_summarizes_content(tmp_path):
    api = FakeWeixinApi(
        [
            {
                "msgs": [
                    {
                        "message_type": 1,
                        "message_state": 2,
                        "from_user_id": "user-1",
                        "item_list": [{"type": 1, "text_item": {"text": "1"}}],
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
    assert choice == "1"
    assert "4. user-dir" in prompt
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


def test_gateway_session_cache_passes_shared_history_loader_to_new_sessions(tmp_path):
    config = AgentConfig.default()
    cache = GatewaySessionCache(
        config=config,
        model=FakeModelClient(),
        registry=ToolRegistry.from_config(config, cwd=tmp_path),
        max_sessions=1,
        idle_seconds=0,
        history_loader=lambda: [
            Message(role="user", content="previous"),
            Message(role="assistant", content="previous answer"),
        ],
    )

    session = cache.get("weixin:user-1", policy=None)
    session.submit("current")

    assert [message.content for message in session.messages[:3]] == ["previous", "previous answer", "current"]


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


def test_gateway_runner_passes_inbound_media_paths_to_session(tmp_path):
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "allow"}})
    channel = FakeChannel("weixin", [])
    media_path = tmp_path / "photo.png"
    runner = GatewayRunner(
        config=config,
        model=FakeModelClient(),
        registry=ToolRegistry.from_config(config, cwd=tmp_path),
    )

    reply = runner.handle_message(
        channel,
        InboundMessage(
            channel="weixin",
            sender_id="user-1",
            text="[image: photo.png]",
            media=[MediaPart(type="image", path=media_path, filename="photo.png", content_type="image/png")],
        ),
    )

    assert "Attachments saved locally:" in reply
    assert f"image: photo.png at {media_path}, content_type=image/png" in reply


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
