from __future__ import annotations

from colibri.config import ConfigError, ModelConfig
from colibri.model.base import ModelClient
from colibri.model.fake import FakeModelClient
from colibri.model.openai_compatible import OpenAICompatibleModelClient


def build_model_client(config: ModelConfig) -> ModelClient:
    if config.provider == "fake":
        return FakeModelClient()
    if config.provider == "openai_compatible":
        return OpenAICompatibleModelClient.from_config(config)
    raise ConfigError(f"Unsupported model provider: {config.provider}")
