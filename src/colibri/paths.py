from __future__ import annotations

import os
from pathlib import Path


def colibri_home() -> Path:
    return Path(os.environ.get("COLIBRI_HOME", "~/.colibri")).expanduser()
