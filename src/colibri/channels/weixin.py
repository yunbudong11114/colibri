from __future__ import annotations

import base64
from dataclasses import dataclass
import hashlib
import mimetypes
import json
from pathlib import Path
import queue
import secrets
import stat
import threading
import time
from typing import Any, Callable
import urllib.error
import urllib.parse
import urllib.request

from Crypto.Cipher import AES

from colibri.channels.base import ChannelContext, InboundMessage
from colibri.config import WeixinChannelConfig
from colibri.media import MediaPart
from colibri.terminal_qr import render_terminal_qr
from colibri.tools.permissions import PermissionRequest, format_permission_prompt_lines


WEIXIN_CHANNEL_VERSION = "2.1.1"
WEIXIN_ILINK_APP_ID = "bot"
WEIXIN_CLIENT_VERSION = "131329"
WEIXIN_MEDIA_MAX_BYTES = 25 * 1024 * 1024
WEIXIN_DEFAULT_CDN_BASE_URL = "https://novac2c.cdn.weixin.qq.com/c2c"
MEDIA_TEMP_DIR = Path("/tmp/colibri/media")
MEDIA_RETENTION_SECONDS = 24 * 60 * 60
MEDIA_MAX_TOTAL_BYTES = 256 * 1024 * 1024
MEDIA_CLEANUP_INTERVAL_SECONDS = 60
MAX_PENDING_MESSAGES = 8
WORK_QUEUE_WAIT_SECONDS = 0.05
WORKER_JOIN_SECONDS = 1.0
_WORKER_STOP = object()
_media_cleanup_lock = threading.Lock()
_last_media_cleanup_at = 0.0


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

    def upload_media(self, path: Path, media_type: str, to_user_id: str = "") -> dict[str, Any]:
        data = path.read_bytes()
        if len(data) > WEIXIN_MEDIA_MAX_BYTES:
            raise WeixinChannelError(f"Weixin media is too large: {len(data)} bytes")

        filekey = secrets.token_hex(16)
        aes_key = secrets.token_bytes(16)
        aes_key_hex = aes_key.hex()
        cipher_data = _encrypt_aes_ecb(data, aes_key)
        upload_response = self._post(
            "ilink/bot/getuploadurl",
            {
                "filekey": filekey,
                "media_type": _weixin_upload_media_type(media_type),
                "to_user_id": to_user_id,
                "rawsize": len(data),
                "rawfilemd5": hashlib.md5(data).hexdigest(),
                "filesize": len(cipher_data),
                "no_need_thumb": True,
                "aeskey": aes_key_hex,
                "base_info": {"channel_version": WEIXIN_CHANNEL_VERSION},
            },
            auth=True,
        )
        if _api_failed(upload_response):
            raise WeixinChannelError(_api_error_text("getuploadurl", upload_response))
        upload_url = str(upload_response.get("upload_full_url") or "").strip()
        upload_param = str(upload_response.get("upload_param") or "").strip()
        if not upload_url:
            if not upload_param:
                raise WeixinChannelError("Weixin getuploadurl returned no upload URL")
            upload_url = _cdn_upload_url(WEIXIN_DEFAULT_CDN_BASE_URL, upload_param, filekey)
        download_param = self._upload_cdn(upload_url, cipher_data)
        return {
            "download_param": download_param,
            "aes_key": aes_key_hex,
            "file_size": len(data),
            "cipher_size": len(cipher_data),
            "filename": path.name,
        }

    def send_media(
        self,
        to_user_id: str,
        context_token: str,
        media_type: str,
        uploaded: dict[str, Any],
        filename: str,
        caption: str,
    ) -> dict[str, Any]:
        if caption.strip():
            text_response = self.send_text(to_user_id, context_token, caption)
            if _api_failed(text_response):
                return text_response
        media_ref = _media_ref_from_upload(uploaded)
        cipher_size = uploaded.get("cipher_size") or uploaded.get("filesize") or uploaded.get("file_size") or 0
        item: dict[str, Any]
        if media_type == "image":
            item = {"type": 2, "image_item": {"media": media_ref, "mid_size": cipher_size}}
        elif media_type == "video":
            item = {"type": 5, "video_item": {"media": media_ref, "video_size": cipher_size}}
        else:
            item = {
                "type": 4,
                "file_item": {
                    "media": media_ref,
                    "file_name": filename,
                    "len": str(uploaded.get("file_size") or uploaded.get("rawsize") or ""),
                },
            }
        return self._post(
            "ilink/bot/sendmessage",
            {
                "msg": {
                    "to_user_id": to_user_id,
                    "client_id": "colibri-" + secrets.token_hex(8),
                    "message_type": 2,
                    "message_state": 2,
                    "item_list": [item],
                    "context_token": context_token,
                },
                "base_info": {"channel_version": WEIXIN_CHANNEL_VERSION},
            },
            auth=True,
        )

    def download_inbound_media(self, item: dict[str, Any]) -> MediaPart:
        media_type, filename, media_ref = _inbound_media_info(item)
        if media_ref is None:
            raise WeixinChannelError("Weixin inbound media item has no media reference")
        data = self._download_inbound_media_bytes(media_ref)
        aes_key = _decode_inbound_aes_key(str(media_ref.get("aes_key") or ""))
        if aes_key:
            data = _decrypt_aes_ecb(data, aes_key)
        content_type = mimetypes.guess_type(filename)[0] or "application/octet-stream"
        path = _write_inbound_media_file(filename, data)
        return MediaPart(type=media_type, path=path, filename=filename, content_type=content_type)

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

    def _upload_cdn(self, upload_url: str, data: bytes) -> str:
        request = urllib.request.Request(
            url=upload_url,
            data=data,
            method="POST",
            headers={"Content-Type": "application/octet-stream"},
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout_seconds) as response:
                status = int(getattr(response, "status", 200) or 200)
                if status < 200 or status >= 300:
                    body = response.read().decode("utf-8", errors="replace")[:500]
                    raise WeixinChannelError(f"Weixin CDN upload HTTP {status}: {body}")
                encrypted_param = response.getheader("X-Encrypted-Param", "")
        except urllib.error.HTTPError as error:
            body = error.read().decode("utf-8", errors="replace")[:500]
            raise WeixinChannelError(f"Weixin CDN upload HTTP {error.code}: {body}") from error
        except urllib.error.URLError as error:
            raise WeixinChannelError(f"Weixin CDN upload failed: {error.reason}") from error
        if not encrypted_param:
            raise WeixinChannelError("Weixin CDN upload missing X-Encrypted-Param")
        return encrypted_param

    def _download_inbound_media_bytes(self, media_ref: dict[str, Any]) -> bytes:
        full_url = str(media_ref.get("full_url") or "").strip()
        encrypted_param = str(media_ref.get("encrypt_query_param") or "").strip()
        url = full_url or _cdn_download_url(WEIXIN_DEFAULT_CDN_BASE_URL, encrypted_param)
        if not url:
            raise WeixinChannelError("Weixin inbound media has no download URL")
        request = urllib.request.Request(url=url, method="GET", headers=self._headers(auth=True))
        try:
            with urllib.request.urlopen(request, timeout=self.timeout_seconds) as response:
                data = response.read()
        except urllib.error.HTTPError as error:
            body = error.read().decode("utf-8", errors="replace")[:500]
            raise WeixinChannelError(f"Weixin CDN download HTTP {error.code}: {body}") from error
        except urllib.error.URLError as error:
            raise WeixinChannelError(f"Weixin CDN download failed: {error.reason}") from error
        if len(data) > WEIXIN_MEDIA_MAX_BYTES:
            raise WeixinChannelError(f"Weixin inbound media is too large: {len(data)} bytes")
        return data


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
        self._waiter_lock = threading.Lock()
        self._text_waiters: dict[str, queue.Queue[str]] = {}
        self._receive_loop_active = False

    def run(self, handler: Callable[[InboundMessage], str], context: ChannelContext) -> None:
        if not self.config.token:
            raise WeixinChannelError("channels.weixin.token is required")
        _maybe_cleanup_inbound_media()
        work: queue.Queue[InboundMessage | object] = queue.Queue(maxsize=MAX_PENDING_MESSAGES)
        errors: queue.Queue[BaseException] = queue.Queue()
        stop_event = threading.Event()
        worker = threading.Thread(
            target=self._run_message_worker,
            args=(handler, work, errors, stop_event),
            name="colibri-weixin-messages",
            daemon=True,
        )
        self._receive_loop_active = True
        worker.start()
        try:
            while not context.stop_requested() and not stop_event.is_set():
                if not errors.empty():
                    raise errors.get_nowait()
                for message in self.poll_messages():
                    if self._deliver_text_waiter(message):
                        continue
                    if (
                        context.try_steer is not None
                        and message.text.strip()
                        and not message.media
                        and context.try_steer(message.sender_id, message.text)
                    ):
                        continue
                    if not _publish_work(work, message, stop_event):
                        break
            if not errors.empty():
                raise errors.get_nowait()
        finally:
            self._receive_loop_active = False
            stop_event.set()
            try:
                work.put_nowait(_WORKER_STOP)
            except queue.Full:
                pass
            worker.join(timeout=WORKER_JOIN_SECONDS)

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
        if self._receive_loop_active:
            waiter = self._register_text_waiter(sender_id)
            try:
                return self._wait_for_registered_text(waiter, timeout_seconds)
            finally:
                self._remove_text_waiter(sender_id, waiter)
        deadline = time.monotonic() + timeout_seconds
        while time.monotonic() < deadline:
            for message in self.poll_messages():
                if message.sender_id == sender_id and not message.media and message.text.strip():
                    return message.text.strip()
            time.sleep(1)
        return None

    def prompt_for_text(self, recipient_id: str, prompt: str, timeout_seconds: int) -> str | None:
        if not self._receive_loop_active:
            self.send_text(recipient_id, prompt)
            return self.wait_for_text(recipient_id, timeout_seconds)
        waiter = self._register_text_waiter(recipient_id)
        try:
            self.send_text(recipient_id, prompt)
            return self._wait_for_registered_text(waiter, timeout_seconds)
        finally:
            self._remove_text_waiter(recipient_id, waiter)

    def send_text(self, recipient_id: str, text: str) -> None:
        context_token = self.context_tokens.get(recipient_id, "")
        response = self.api.send_text(recipient_id, context_token, text)
        if _api_failed(response):
            raise WeixinChannelError(_api_error_text("sendmessage", response))

    def send_media(self, recipient_id: str, media: MediaPart) -> None:
        context_token = self.context_tokens.get(recipient_id, "")
        media_type = _media_type_for_part(media)
        response = self.api.upload_media(media.path, media_type, to_user_id=recipient_id)
        if _api_failed(response):
            raise WeixinChannelError(_api_error_text("getuploadurl", response))
        send_response = self.api.send_media(
            recipient_id,
            context_token,
            media_type,
            response,
            media.filename or media.path.name,
            media.caption,
        )
        if _api_failed(send_response):
            raise WeixinChannelError(_api_error_text("sendmessage", send_response))

    def _run_message_worker(
        self,
        handler: Callable[[InboundMessage], str],
        work: queue.Queue[InboundMessage | object],
        errors: queue.Queue[BaseException],
        stop_event: threading.Event,
    ) -> None:
        try:
            while True:
                try:
                    message = work.get(timeout=WORK_QUEUE_WAIT_SECONDS)
                except queue.Empty:
                    if stop_event.is_set():
                        return
                    continue
                if message is _WORKER_STOP:
                    return
                if not isinstance(message, InboundMessage):
                    continue
                reply = handler(message)
                if reply.strip():
                    self.send_text(message.sender_id, reply)
        except BaseException as error:
            errors.put(error)
            stop_event.set()

    def _register_text_waiter(self, sender_id: str) -> queue.Queue[str]:
        waiter: queue.Queue[str] = queue.Queue(maxsize=1)
        with self._waiter_lock:
            self._text_waiters[sender_id] = waiter
        return waiter

    @staticmethod
    def _wait_for_registered_text(waiter: queue.Queue[str], timeout_seconds: int) -> str | None:
        try:
            return waiter.get(timeout=max(0, timeout_seconds))
        except queue.Empty:
            return None

    def _remove_text_waiter(self, sender_id: str, waiter: queue.Queue[str]) -> None:
        with self._waiter_lock:
            if self._text_waiters.get(sender_id) is waiter:
                self._text_waiters.pop(sender_id, None)

    def _deliver_text_waiter(self, message: InboundMessage) -> bool:
        if message.media or not message.text.strip():
            return False
        with self._waiter_lock:
            waiter = self._text_waiters.get(message.sender_id)
        if waiter is None:
            return False
        try:
            waiter.put_nowait(message.text.strip())
        except queue.Full:
            return False
        return True

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
        text, media = self._content_from_items(raw.get("item_list") or [])
        if not text.strip() and not media:
            return None
        return InboundMessage(
            channel=self.name,
            sender_id=sender_id,
            text=text.strip(),
            message_id=str(raw.get("message_id") or ""),
            media=media,
        )

    def _is_allowed(self, sender_id: str) -> bool:
        allow_from = [item for item in self.config.allow_from if item]
        return not allow_from or "*" in allow_from or sender_id in allow_from

    def _content_from_items(self, items: list[Any]) -> tuple[str, list[MediaPart]]:
        texts: list[str] = []
        media_parts: list[MediaPart] = []
        for item in items:
            if not isinstance(item, dict):
                continue
            item_type = int(item.get("type") or 0)
            if item_type == 1:
                text_item = item.get("text_item")
                if isinstance(text_item, dict):
                    text = text_item.get("text")
                    if isinstance(text, str) and text.strip():
                        texts.append(text.strip())
                continue
            if media_parts or item_type not in {2, 4}:
                continue
            try:
                media = self.api.download_inbound_media(item)
            except Exception:
                continue
            media_parts.append(media)
            if item_type == 2:
                texts.append(f"[image: {media.filename}]")
            else:
                texts.append(f"[file: {media.filename}]")
        return "\n".join(texts), media_parts


class WeixinPermissionPrompter:
    def __init__(self, channel: WeixinChannel, recipient_id: str, timeout_seconds: int = 300):
        self.channel = channel
        self.recipient_id = recipient_id
        self.timeout_seconds = timeout_seconds

    def confirm(self, request: PermissionRequest) -> str:
        reply = self.channel.prompt_for_text(
            self.recipient_id,
            _permission_prompt_text(request),
            self.timeout_seconds,
        )
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


def _inbound_media_info(item: dict[str, Any]) -> tuple[str, str, dict[str, Any] | None]:
    item_type = int(item.get("type") or 0)
    if item_type == 2:
        image_item = item.get("image_item") if isinstance(item.get("image_item"), dict) else {}
        media_ref = image_item.get("media") if isinstance(image_item.get("media"), dict) else None
        filename = str(image_item.get("file_name") or "image.png")
        return "image", _safe_filename(filename), media_ref
    if item_type == 4:
        file_item = item.get("file_item") if isinstance(item.get("file_item"), dict) else {}
        media_ref = file_item.get("media") if isinstance(file_item.get("media"), dict) else None
        filename = str(file_item.get("file_name") or "file.bin")
        return "file", _safe_filename(filename), media_ref
    raise WeixinChannelError(f"Unsupported Weixin inbound media type: {item_type}")


def _api_failed(response: dict[str, Any]) -> bool:
    return int(response.get("ret") or 0) != 0 or int(response.get("errcode") or 0) != 0


def _api_error_text(action: str, response: dict[str, Any]) -> str:
    return (
        f"Weixin {action} failed: ret={response.get('ret') or 0} "
        f"errcode={response.get('errcode') or 0} errmsg={response.get('errmsg') or ''}"
    )


def _media_type_for_part(media: MediaPart) -> str:
    if media.type in {"image", "video", "audio", "file"}:
        return "file" if media.type == "audio" else media.type
    content_type = media.content_type or mimetypes.guess_type(media.filename or media.path.name)[0] or ""
    if content_type.startswith("image/"):
        return "image"
    if content_type.startswith("video/"):
        return "video"
    return "file"


def _weixin_upload_media_type(media_type: str) -> int:
    return {"image": 1, "video": 2, "file": 3}.get(media_type, 3)


def _media_ref_from_upload(uploaded: dict[str, Any]) -> dict[str, Any]:
    if "media" in uploaded and isinstance(uploaded["media"], dict):
        return uploaded["media"]
    return {
        "encrypt_query_param": uploaded.get("download_param") or uploaded.get("upload_param") or uploaded.get("media_id") or "",
        "aes_key": _encode_weixin_aes_key(str(uploaded.get("aes_key") or "")),
        "encrypt_type": uploaded.get("encrypt_type") or 1,
    }


def _pkcs7_pad(data: bytes, block_size: int = 16) -> bytes:
    padding = block_size - (len(data) % block_size)
    return data + bytes([padding]) * padding


def _encrypt_aes_ecb(data: bytes, key: bytes) -> bytes:
    return AES.new(key, AES.MODE_ECB).encrypt(_pkcs7_pad(data, AES.block_size))


def _decrypt_aes_ecb(data: bytes, key: bytes) -> bytes:
    if len(data) % AES.block_size != 0:
        raise WeixinChannelError(f"Invalid AES-ECB ciphertext length: {len(data)}")
    plaintext = AES.new(key, AES.MODE_ECB).decrypt(data)
    return _pkcs7_unpad(plaintext, AES.block_size)


def _pkcs7_unpad(data: bytes, block_size: int = 16) -> bytes:
    if not data or len(data) % block_size != 0:
        raise WeixinChannelError(f"Invalid PKCS7 data length: {len(data)}")
    padding = data[-1]
    if padding <= 0 or padding > block_size or padding > len(data):
        raise WeixinChannelError("Invalid PKCS7 padding")
    if data[-padding:] != bytes([padding]) * padding:
        raise WeixinChannelError("Invalid PKCS7 padding bytes")
    return data[:-padding]


def _cdn_upload_url(base_url: str, upload_param: str, filekey: str) -> str:
    return (
        base_url.rstrip("/")
        + "/upload?encrypted_query_param="
        + urllib.parse.quote(upload_param)
        + "&filekey="
        + urllib.parse.quote(filekey)
    )


def _cdn_download_url(base_url: str, encrypted_param: str) -> str:
    if not encrypted_param:
        return ""
    return base_url.rstrip("/") + "/download?encrypted_query_param=" + urllib.parse.quote(encrypted_param)


def _encode_weixin_aes_key(aes_key_hex: str) -> str:
    return base64.b64encode(aes_key_hex.encode("utf-8")).decode("ascii")


def _decode_inbound_aes_key(value: str) -> bytes:
    if not value:
        return b""
    decoded = base64.b64decode(value)
    if len(decoded) == 16:
        return decoded
    if len(decoded) == 32:
        try:
            raw = bytes.fromhex(decoded.decode("ascii"))
        except ValueError as error:
            raise WeixinChannelError("Invalid Weixin inbound AES key") from error
        if len(raw) == 16:
            return raw
    raise WeixinChannelError(f"Unsupported Weixin inbound AES key length: {len(decoded)}")


def _write_inbound_media_file(filename: str, data: bytes) -> Path:
    _maybe_cleanup_inbound_media(required_bytes=len(data))
    MEDIA_TEMP_DIR.mkdir(parents=True, exist_ok=True)
    ext = Path(filename).suffix or ".bin"
    path = MEDIA_TEMP_DIR / f"weixin-inbound-{secrets.token_hex(8)}{ext}"
    path.write_bytes(data)
    return path


def _maybe_cleanup_inbound_media(required_bytes: int = 0) -> None:
    global _last_media_cleanup_at

    now = time.time()
    with _media_cleanup_lock:
        if required_bytes <= 0 and now - _last_media_cleanup_at < MEDIA_CLEANUP_INTERVAL_SECONDS:
            return
        _last_media_cleanup_at = now
        _cleanup_media_directory(
            MEDIA_TEMP_DIR,
            now=now,
            retention_seconds=MEDIA_RETENTION_SECONDS,
            max_total_bytes=max(0, MEDIA_MAX_TOTAL_BYTES - max(0, required_bytes)),
        )


def _cleanup_media_directory(
    root: Path,
    *,
    now: float,
    retention_seconds: int,
    max_total_bytes: int,
) -> None:
    try:
        entries = list(root.iterdir())
    except OSError:
        return

    retained: list[tuple[Path, float, int]] = []
    for path in entries:
        try:
            metadata = path.stat(follow_symlinks=False)
        except OSError:
            continue
        if not stat.S_ISREG(metadata.st_mode):
            continue
        if now - metadata.st_mtime >= max(0, retention_seconds):
            try:
                path.unlink()
            except OSError:
                retained.append((path, metadata.st_mtime, metadata.st_size))
            continue
        retained.append((path, metadata.st_mtime, metadata.st_size))

    total_bytes = sum(size for _path, _modified, size in retained)
    budget = max(0, max_total_bytes)
    if total_bytes <= budget:
        return
    for path, _modified, size in sorted(retained, key=lambda item: (item[1], item[0].name)):
        try:
            path.unlink()
        except OSError:
            continue
        total_bytes -= size
        if total_bytes <= budget:
            return


def _publish_work(
    work: queue.Queue[InboundMessage | object],
    message: InboundMessage,
    stop_event: threading.Event,
) -> bool:
    while not stop_event.is_set():
        try:
            work.put(message, timeout=WORK_QUEUE_WAIT_SECONDS)
            return True
        except queue.Full:
            continue
    return False


def _safe_filename(filename: str) -> str:
    name = Path(filename.strip()).name
    return name if name and name not in {".", "/"} else "file.bin"


def _permission_prompt_text(request: PermissionRequest) -> str:
    lines = [f"Colibri wants to run {request.tool_name}."]
    for line in format_permission_prompt_lines(request):
        if request.subject.kind == "file_path" and line.startswith("file: "):
            lines.append("path: " + line.removeprefix("file: ").split(" ", 1)[-1])
        else:
            lines.append(line)
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
