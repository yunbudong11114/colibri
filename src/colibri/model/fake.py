from __future__ import annotations

from colibri.messages import Message, ModelLimits, ModelResponse
from colibri.time_context import user_text_without_time_context


class FakeModelClient:
    def complete(
        self,
        messages: list[Message],
        tools: list[dict],
        system: str,
        limits: ModelLimits,
    ) -> ModelResponse:
        last_user = next(
            (message.content for message in reversed(messages) if message.role == "user"),
            "",
        )
        last_user = user_text_without_time_context(last_user)
        return ModelResponse(text=f"fake: {last_user}")

    def complete_image(self, prompt: str, image_data_url: str, limits: ModelLimits) -> ModelResponse:
        return ModelResponse(text=f"fake image: {prompt}")
