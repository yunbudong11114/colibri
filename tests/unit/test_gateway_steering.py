import pytest

from colibri.channels.base import ChannelContext, InboundMessage
from colibri.channels.weixin import WeixinChannel
from colibri.config import AgentConfig, WeixinChannelConfig
from colibri.gateway import GatewayRunner, GatewaySessionCache
from colibri.model.fake import FakeModelClient
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
        return {"media_id": "media-1", "aes_key": "key", "file_size": 5}

    def send_media(self, to_user_id, context_token, media_type, uploaded, filename, caption):
        return {"ret": 0, "errcode": 0}

    def download_inbound_media(self, item):
        return None


def _text_update(text: str, message_id: str = "msg-1") -> dict:
    return {
        "get_updates_buf": "next",
        "msgs": [
            {
                "message_type": 1,
                "message_state": 2,
                "from_user_id": "user-1",
                "message_id": message_id,
                "context_token": "ctx-1",
                "item_list": [{"type": 1, "text_item": {"text": text}}],
            }
        ],
    }


def test_try_steer_returns_false_when_no_session(tmp_path):
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "allow"}})
    runner = GatewayRunner(
        config=config,
        model=FakeModelClient(),
        registry=ToolRegistry.from_config(config, cwd=tmp_path),
    )

    assert runner.try_steer("weixin", "user-1", "change plan") is False


def test_try_steer_enqueues_when_turn_active(tmp_path):
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "allow"}})
    runner = GatewayRunner(
        config=config,
        model=FakeModelClient(),
        registry=ToolRegistry.from_config(config, cwd=tmp_path),
    )
    session = runner.sessions.get("weixin:user-1", policy=None)
    session._turn_active = True

    assert runner.try_steer("weixin", "user-1", "change plan") is True
    assert session._steering.get_nowait() == "change plan"


def test_get_existing_does_not_create_session(tmp_path):
    config = AgentConfig.default()
    cache = GatewaySessionCache(
        config=config,
        model=FakeModelClient(),
        registry=ToolRegistry.from_config(config, cwd=tmp_path),
        max_sessions=1,
        idle_seconds=0,
    )

    assert cache.get_existing("weixin:user-1") is None
    created = cache.get("weixin:user-1", policy=None)
    assert cache.get_existing("weixin:user-1") is created


def test_take_or_create_keeps_steer_available_while_session_outside_cache(tmp_path):
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "allow"}})
    cache = GatewaySessionCache(
        config=config,
        model=FakeModelClient(),
        registry=ToolRegistry.from_config(config, cwd=tmp_path),
        max_sessions=1,
        idle_seconds=0,
    )
    session = cache.take_or_create("weixin:user-1", policy=None)
    session._turn_active = True

    assert cache.get_existing("weixin:user-1") is session
    assert cache.try_steer("weixin:user-1", "change plan") is True
    assert session._steering.get_nowait() == "change plan"

    cache.put_back("weixin:user-1", session)
    assert cache.get_existing("weixin:user-1") is session


def test_weixin_receive_skips_queue_when_try_steer_true():
    api = FakeWeixinApi([_text_update("steer me")])
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token"), api=api)
    handled = []
    steered = []

    def try_steer(sender_id: str, text: str) -> bool:
        steered.append((sender_id, text))
        return True

    channel.run(
        lambda message: handled.append(message) or "should-not-send",
        ChannelContext(
            stop_requested=lambda: bool(steered),
            try_steer=try_steer,
        ),
    )

    assert steered == [("user-1", "steer me")]
    assert handled == []
    assert api.sent == []


def test_worker_skips_send_on_empty_reply():
    api = FakeWeixinApi([_text_update("hi")])
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token"), api=api)
    handled = []

    channel.run(
        lambda message: handled.append(message) or "",
        ChannelContext(stop_requested=lambda: bool(handled)),
    )

    assert len(handled) == 1
    assert api.sent == []


def test_handle_message_steers_when_turn_active(tmp_path):
    config = AgentConfig.default().with_overrides({"tools": {"default_permission": "allow"}})
    channel = WeixinChannel(WeixinChannelConfig(enabled=True, token="token"), api=FakeWeixinApi())
    runner = GatewayRunner(
        config=config,
        model=FakeModelClient(),
        registry=ToolRegistry.from_config(config, cwd=tmp_path),
    )
    session = runner.sessions.get("weixin:user-1", policy=None)
    session._turn_active = True
    submits = []
    original_submit = session.submit

    def tracking_submit(*args, **kwargs):
        submits.append((args, kwargs))
        return original_submit(*args, **kwargs)

    session.submit = tracking_submit  # type: ignore[method-assign]

    reply = runner.handle_message(
        channel,
        InboundMessage(channel="weixin", sender_id="user-1", text="change plan"),
    )

    assert reply == ""
    assert submits == []
    assert session._steering.get_nowait() == "change plan"
