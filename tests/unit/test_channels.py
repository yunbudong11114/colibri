from pathlib import Path

from colibri.channels.base import InboundMessage
from colibri.channels.weixin import WeixinChannel, WeixinPermissionPrompter
from colibri.config import AgentConfig, WeixinChannelConfig
from colibri.gateway import GatewayRunner, GatewaySessionCache
from colibri.model.fake import FakeModelClient
from colibri.tools.permissions import PermissionRequest, PermissionSubject
from colibri.tools.registry import ToolRegistry


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
