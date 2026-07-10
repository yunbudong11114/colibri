from __future__ import annotations

from dataclasses import dataclass
from typing import Callable, Protocol

from colibri.media import MediaPart


@dataclass(frozen=True)
class InboundMessage:
    channel: str
    sender_id: str
    text: str
    message_id: str = ""


@dataclass(frozen=True)
class ChannelContext:
    stop_requested: Callable[[], bool] = lambda: False


class Channel(Protocol):
    name: str

    def run(self, handler: Callable[[InboundMessage], str], context: ChannelContext) -> None:
        ...

    def send_text(self, recipient_id: str, text: str) -> None:
        ...

    def send_media(self, recipient_id: str, media: MediaPart) -> None:
        ...
