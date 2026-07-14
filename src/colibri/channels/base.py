from __future__ import annotations

from dataclasses import dataclass, field
from typing import Callable, Protocol

from colibri.media import MediaPart
from colibri.tools.permissions import PermissionPrompter


@dataclass(frozen=True)
class InboundMessage:
    """Channel-agnostic inbound work item (light envelope until media is resolved)."""

    channel: str
    sender_id: str
    text: str
    message_id: str = ""
    media: list[MediaPart] = field(default_factory=list)
    # Channel-private media item refs; resolved to `media` before agent submit.
    media_refs: list[object] = field(default_factory=list)
    # Channel-private context (e.g. weixin context_token).
    context: dict[str, str] = field(default_factory=dict)


@dataclass(frozen=True)
class ChannelContext:
    stop_requested: Callable[[], bool] = lambda: False
    try_steer: Callable[[str, str], bool] | None = None  # (sender_id, text) -> bool


# offer(message) -> False if the shared bus dropped the message (full).
OfferInbound = Callable[[InboundMessage], bool]


class Channel(Protocol):
    """Gateway channel adapter. Poll only; shared bus owns queues/workers."""

    name: str

    def run_poll(self, offer: OfferInbound, context: ChannelContext) -> None:
        """Long-poll / receive loop. Must not download CDN bodies or run agent turns."""

    def send_text(self, recipient_id: str, text: str) -> None:
        ...

    def send_media(self, recipient_id: str, media: MediaPart) -> None:
        ...

    def resolve_inbound_media(self, message: InboundMessage) -> InboundMessage:
        ...

    def permission_prompter(self, recipient_id: str) -> PermissionPrompter | None:
        ...
