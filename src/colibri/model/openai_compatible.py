from __future__ import annotations

from dataclasses import dataclass
import os

from colibri.config import ConfigError, ModelConfig
from colibri.messages import Message, ModelLimits, ModelResponse


@dataclass(frozen=True)
class OpenAICompatibleModelClient:
    base_url: str
    model: str
    api_key: str

    @classmethod
    def from_config(cls, config: ModelConfig) -> "OpenAICompatibleModelClient":
        api_key = os.environ.get(config.api_key_env, "")
        if not api_key:
            raise ConfigError(f"Missing API key environment variable: {config.api_key_env}")
        return cls(base_url=config.base_url.rstrip("/"), model=config.model, api_key=api_key)

    def complete(
        self,
        messages: list[Message],
        tools: list[dict],
        system: str,
        limits: ModelLimits,
    ) -> ModelResponse:
        raise NotImplementedError("OpenAI-compatible completion is implemented in Task 2")
