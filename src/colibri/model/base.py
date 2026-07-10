from __future__ import annotations

from typing import Protocol

from colibri.messages import Message, ModelLimits, ModelResponse


class ModelClient(Protocol):
    def complete(
        self,
        messages: list[Message],
        tools: list[dict],
        system: str,
        limits: ModelLimits,
    ) -> ModelResponse:
        ...

    def complete_image(
        self,
        prompt: str,
        image_data_url: str,
        limits: ModelLimits,
    ) -> ModelResponse:
        ...
