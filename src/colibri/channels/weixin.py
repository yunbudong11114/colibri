from __future__ import annotations

from dataclasses import dataclass
import json
import secrets
import time
from typing import Any, Callable
import urllib.error
import urllib.parse
import urllib.request

from colibri.channels.base import ChannelContext, InboundMessage
from colibri.config import WeixinChannelConfig
from colibri.terminal_qr import render_terminal_qr
from colibri.tools.permissions import PermissionRequest


WEIXIN_CHANNEL_VERSION = "2.1.1"
WEIXIN_ILINK_APP_ID = "bot"
WEIXIN_CLIENT_VERSION = "131329"


class WeixinChannelError(RuntimeError):
    pass


@dataclass(frozen=True)
class WeixinAuthResult:
    token: str
    user_id: str
    account_id: str
    base_url: str


class WeixinApiClient:
    def __init__(self, base_url: str, token: str = "", timeout_seconds: int = 35):
        self.base_url = (base_url or "https://ilinkai.weixin.qq.com/").rstrip("/") + "/"
        self.token = token
        self.timeout_seconds = timeout_seconds

    def get_qrcode(self, bot_type: str = "3") -> dict[str, Any]:
        return self._get("ilink/bot/get_bot_qrcode", {"bot_type": bot_type}, auth=False)

    def get_qrcode_status(self, qrcode: str) -> dict[str, Any]:
        return self._get("ilink/bot/get_qrcode_status", {"qrcode": qrcode}, auth=False)

    def get_updates(self, get_updates_buf: str) -> dict[str, Any]:
        return self._post(
            "ilink/bot/getupdates",
            {
                "get_updates_buf": get_updates_buf,
                "base_info": {"channel_version": WEIXIN_CHANNEL_VERSION},
            },
            auth=True,
            timeout_seconds=self.timeout_seconds + 5,
        )

    def send_text(self, to_user_id: str, context_token: str, text: str) -> dict[str, Any]:
        if not text.strip():
            return {}
        return self._post(
            "ilink/bot/sendmessage",
            {
                "msg": {
                    "to_user_id": to_user_id,
                    "client_id": "colibri-" + secrets.token_hex(8),
                    "message_type": 2,
                    "message_state": 2,
                    "item_list": [
                        {
                            "type": 1,
                            "text_item": {"text": text},
                        }
                    ],
                    "context_token": context_token,
                },
                "base_info": {"channel_version": WEIXIN_CHANNEL_VERSION},
            },
            auth=True,
        )

    def _get(self, endpoint: str, query: dict[str, str], *, auth: bool) -> dict[str, Any]:
        url = self._url(endpoint, query)
        request = urllib.request.Request(url=url, method="GET", headers=self._headers(auth=auth))
        return self._open_json(request, self.timeout_seconds)

    def _post(
        self,
        endpoint: str,
        payload: dict[str, Any],
        *,
        auth: bool,
        timeout_seconds: int | None = None,
    ) -> dict[str, Any]:
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        request = urllib.request.Request(
            url=self._url(endpoint),
            data=body,
            method="POST",
            headers={**self._headers(auth=auth), "Content-Type": "application/json"},
        )
        return self._open_json(request, timeout_seconds or self.timeout_seconds)

    def _url(self, endpoint: str, query: dict[str, str] | None = None) -> str:
        url = urllib.parse.urljoin(self.base_url, endpoint)
        if not query:
            return url
        return url + "?" + urllib.parse.urlencode(query)

    def _headers(self, *, auth: bool) -> dict[str, str]:
        headers = {
            "iLink-App-Id": WEIXIN_ILINK_APP_ID,
            "iLink-App-ClientVersion": WEIXIN_CLIENT_VERSION,
        }
        if auth:
            headers["AuthorizationType"] = "ilink_bot_token"
            headers["X-WECHAT-UIN"] = str(secrets.randbits(32))
            if self.token:
                headers["Authorization"] = f"Bearer {self.token}"
        return headers

    @staticmethod
    def _open_json(request: urllib.request.Request, timeout_seconds: int) -> dict[str, Any]:
        try:
            with urllib.request.urlopen(request, timeout=timeout_seconds) as response:
                body = response.read().decode("utf-8")
        except urllib.error.HTTPError as error:
            body = error.read().decode("utf-8", errors="replace")[:500]
            raise WeixinChannelError(f"Weixin API HTTP {error.code}: {body}") from error
        except urllib.error.URLError as error:
            raise WeixinChannelError(f"Weixin API request failed: {error.reason}") from error
        except TimeoutError as error:
            raise WeixinChannelError("Weixin API request timed out") from error
        try:
            return json.loads(body) if body else {}
        except json.JSONDecodeError as error:
            raise WeixinChannelError("Weixin API response was not valid JSON") from error


class WeixinChannel:
    name = "weixin"

    def __init__(self, config: WeixinChannelConfig, api: WeixinApiClient | None = None):
        self.config = config
        self.api = api or WeixinApiClient(
            base_url=config.base_url,
            token=config.token,
            timeout_seconds=config.poll_timeout_seconds,
        )
        self.get_updates_buf = ""
        self.context_tokens: dict[str, str] = {}

    def run(self, handler: Callable[[InboundMessage], str], context: ChannelContext) -> None:
        if not self.config.token:
            raise WeixinChannelError("channels.weixin.token is required")
        while not context.stop_requested():
            for message in self.poll_messages():
                reply = handler(message)
                if reply.strip():
                    self.send_text(message.sender_id, reply)

    def poll_messages(self) -> list[InboundMessage]:
        data = self.api.get_updates(self.get_updates_buf)
        self.get_updates_buf = str(data.get("get_updates_buf") or self.get_updates_buf)
        messages: list[InboundMessage] = []
        for raw in data.get("msgs") or []:
            message = self._parse_inbound(raw)
            if message is not None:
                messages.append(message)
        return messages

    def wait_for_text(self, sender_id: str, timeout_seconds: int) -> str | None:
        deadline = time.monotonic() + timeout_seconds
        while time.monotonic() < deadline:
            for message in self.poll_messages():
                if message.sender_id == sender_id and message.text.strip():
                    return message.text.strip()
            time.sleep(1)
        return None

    def send_text(self, recipient_id: str, text: str) -> None:
        context_token = self.context_tokens.get(recipient_id, "")
        response = self.api.send_text(recipient_id, context_token, text)
        if _api_failed(response):
            raise WeixinChannelError(_api_error_text("sendmessage", response))

    def _parse_inbound(self, raw: dict[str, Any]) -> InboundMessage | None:
        if int(raw.get("message_type") or 0) != 1:
            return None
        if int(raw.get("message_state") or 0) != 2:
            return None
        sender_id = str(raw.get("from_user_id") or "").strip()
        if not sender_id or not self._is_allowed(sender_id):
            return None
        context_token = str(raw.get("context_token") or "")
        if context_token:
            self.context_tokens[sender_id] = context_token
        text = _text_from_items(raw.get("item_list") or [])
        if not text.strip():
            return None
        return InboundMessage(
            channel=self.name,
            sender_id=sender_id,
            text=text.strip(),
            message_id=str(raw.get("message_id") or ""),
        )

    def _is_allowed(self, sender_id: str) -> bool:
        allow_from = [item for item in self.config.allow_from if item]
        return not allow_from or "*" in allow_from or sender_id in allow_from


class WeixinPermissionPrompter:
    def __init__(self, channel: WeixinChannel, recipient_id: str, timeout_seconds: int = 300):
        self.channel = channel
        self.recipient_id = recipient_id
        self.timeout_seconds = timeout_seconds

    def confirm(self, request: PermissionRequest) -> str:
        self.channel.send_text(self.recipient_id, _permission_prompt_text(request))
        reply = self.channel.wait_for_text(self.recipient_id, self.timeout_seconds)
        if reply is None:
            return "n"
        return _permission_choice(reply)


def perform_weixin_auth(base_url: str, timeout_seconds: int, print_func: Callable[[str], None] = print) -> WeixinAuthResult:
    api = WeixinApiClient(base_url=base_url, timeout_seconds=35)
    qrcode = api.get_qrcode()
    qr_payload = str(qrcode.get("qrcode_img_content") or "")
    qr_id = str(qrcode.get("qrcode") or "")
    if not qr_payload or not qr_id:
        raise WeixinChannelError("Weixin auth did not return a QR code")

    print_func("Scan this Weixin QR code with WeChat:")
    rendered_qr = render_terminal_qr(qr_payload)
    if rendered_qr:
        print_func(rendered_qr)
    else:
        print_func("(QR payload is too large for the built-in terminal renderer.)")
    print_func("QR payload:")
    print_func(qr_payload)
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        status = api.get_qrcode_status(qr_id)
        state = str(status.get("status") or "")
        if state == "confirmed":
            token = str(status.get("bot_token") or "")
            account_id = str(status.get("ilink_bot_id") or "")
            user_id = str(status.get("ilink_user_id") or "")
            if not token or not account_id:
                raise WeixinChannelError("Weixin auth confirmed but missing token")
            return WeixinAuthResult(
                token=token,
                user_id=user_id,
                account_id=account_id,
                base_url=str(status.get("baseurl") or base_url),
            )
        if state == "expired":
            raise WeixinChannelError("Weixin auth QR code expired")
        time.sleep(2)
    raise WeixinChannelError("Weixin auth timed out")


def _text_from_items(items: list[Any]) -> str:
    texts: list[str] = []
    for item in items:
        if not isinstance(item, dict):
            continue
        if int(item.get("type") or 0) != 1:
            continue
        text_item = item.get("text_item")
        if isinstance(text_item, dict):
            text = text_item.get("text")
            if isinstance(text, str) and text.strip():
                texts.append(text.strip())
    return "\n".join(texts)


def _api_failed(response: dict[str, Any]) -> bool:
    return int(response.get("ret") or 0) != 0 or int(response.get("errcode") or 0) != 0


def _api_error_text(action: str, response: dict[str, Any]) -> str:
    return (
        f"Weixin {action} failed: ret={response.get('ret') or 0} "
        f"errcode={response.get('errcode') or 0} errmsg={response.get('errmsg') or ''}"
    )


def _permission_prompt_text(request: PermissionRequest) -> str:
    lines = [f"Colibri wants to run {request.tool_name}."]
    if request.subject.shell_command:
        lines.append(f"command: {request.subject.shell_command}")
    elif request.subject.file_path:
        lines.append(f"path: {request.subject.file_path}")
    else:
        lines.append(f"arguments: {request.arguments}")
    lines.extend(["", "Reply one of:"])
    if request.subject.kind == "shell":
        lines.append("y=once s=session e=executable-session p=project n=deny")
    else:
        lines.append("y=once s=session p=project n=deny")
    return "\n".join(lines)


def _permission_choice(reply: str) -> str:
    normalized = reply.strip().lower()
    aliases = {
        "yes": "y",
        "once": "y",
        "allow": "y",
        "session": "s",
        "always": "s",
        "executable": "e",
        "executable-session": "e",
        "project": "p",
        "no": "n",
        "deny": "n",
    }
    first = normalized.split()[0] if normalized.split() else "n"
    return aliases.get(first, first if first in {"y", "s", "e", "p", "n"} else "n")
