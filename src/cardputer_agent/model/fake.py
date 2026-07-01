from __future__ import annotations

from cardputer_agent.messages import Message, ModelLimits, ModelResponse


class FakeModelClient:
    def complete(
        self,
        messages: list[Message],
        tools: list[dict],
        system: str,
        limits: ModelLimits,
    ) -> ModelResponse:
        last_user = next((message.content for message in reversed(messages) if message.role == "user"), "")
        return ModelResponse(text=f"fake: {last_user}")
