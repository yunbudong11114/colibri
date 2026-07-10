from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class MediaPart:
    type: str
    path: Path
    filename: str = ""
    content_type: str = ""
    caption: str = ""
