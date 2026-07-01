from __future__ import annotations

from typing import Protocol

from cardputer_agent.messages import Message, ModelLimits, ModelResponse


class ModelClient(Protocol):
    def complete(
        self,
        messages: list[Message],
        tools: list[dict],
        system: str,
        limits: ModelLimits,
    ) -> ModelResponse:
        ...
