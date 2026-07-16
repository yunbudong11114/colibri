from __future__ import annotations

from dataclasses import dataclass, replace
from pathlib import Path
from typing import Callable

from colibri.config import AgentConfig
from colibri.model.base import ModelClient


@dataclass(frozen=True)
class RuntimeSnapshot:
    config: AgentConfig
    model: ModelClient


@dataclass(frozen=True)
class RuntimeReloadResult:
    snapshot: RuntimeSnapshot | None = None
    error: str | None = None

    @property
    def changed(self) -> bool:
        return self.snapshot is not None


class PartialRuntimeReloader:
    def __init__(
        self,
        path: Path,
        config: AgentConfig,
        model: ModelClient,
        *,
        model_builder: Callable[[object], ModelClient],
    ):
        self.path = path.expanduser()
        self.config = config
        self.model = model
        self.model_builder = model_builder
        self._fingerprint = config_fingerprint(self.path)

    def reload_if_changed(self) -> RuntimeReloadResult:
        fingerprint = config_fingerprint(self.path)
        if fingerprint == self._fingerprint:
            return RuntimeReloadResult()
        self._fingerprint = fingerprint
        try:
            candidate = AgentConfig.load(self.path)
            config = replace(
                self.config,
                model=candidate.model,
                vision=candidate.vision,
                web_search=candidate.web_search,
            )
            model = self.model_builder(config.model)
        except Exception as error:
            return RuntimeReloadResult(error=str(error))
        self.config = config
        self.model = model
        return RuntimeReloadResult(snapshot=RuntimeSnapshot(config=config, model=model))


def config_fingerprint(path: Path) -> tuple[int, int, int, int] | None:
    try:
        stat = path.stat()
    except OSError:
        return None
    return (stat.st_dev, stat.st_ino, stat.st_mtime_ns, stat.st_size)
