from __future__ import annotations

from dataclasses import dataclass, field
from typing import Callable, Protocol

from colibri.media import MediaPart


@dataclass(frozen=True)
class InboundMessage:
    channel: str
    sender_id: str
    text: str
    message_id: str = ""
    media: list[MediaPart] = field(default_factory=list)


@dataclass(frozen=True)
class ChannelContext:
    stop_requested: Callable[[], bool] = lambda: False
    try_steer: Callable[[str, str], bool] | None = None  # (sender_id, text) -> bool


class Channel(Protocol):
    name: str

    def run(self, handler: Callable[[InboundMessage], str], context: ChannelContext) -> None:
        ...

    def send_text(self, recipient_id: str, text: str) -> None:
        ...

    def send_media(self, recipient_id: str, media: MediaPart) -> None:
        ...
