from __future__ import annotations

import base64
import mimetypes
from pathlib import Path

from colibri.config import AgentConfig, ModelConfig
from colibri.messages import ModelLimits, ModelResponse
from colibri.model.base import ModelClient
from colibri.model.factory import build_model_client


class VisionError(RuntimeError):
    def __init__(self, message: str, error_type: str):
        super().__init__(message)
        self.error_type = error_type


class ImageAnalyzer:
    def __init__(self, config: AgentConfig, agent_model: ModelClient):
        self.config = config
        self.agent_model = agent_model
        self._vision_model: ModelClient | None = None

    def __call__(self, path: Path, prompt: str) -> str:
        content_type = mimetypes.guess_type(path.name)[0] or ""
        if not content_type.startswith("image/"):
            raise VisionError("image.understand requires an image file", "invalid_media")

        try:
            size = path.stat().st_size
        except OSError as error:
            raise VisionError(str(error), "read_error") from error
        max_bytes = max(1, self.config.vision.max_image_bytes)
        if size > max_bytes:
            raise VisionError(
                f"Image is too large: {size} bytes exceeds {max_bytes} bytes",
                "image_too_large",
            )

        try:
            image_bytes = path.read_bytes()
        except OSError as error:
            raise VisionError(str(error), "read_error") from error
        if len(image_bytes) > max_bytes:
            raise VisionError(
                f"Image is too large: {len(image_bytes)} bytes exceeds {max_bytes} bytes",
                "image_too_large",
            )

        image_data_url = f"data:{content_type};base64,{base64.b64encode(image_bytes).decode('ascii')}"
        response = self._model().complete_image(
            prompt or "Describe this image and extract its important information.",
            image_data_url,
            ModelLimits(
                timeout_seconds=self.config.vision.timeout_seconds,
                max_output_tokens=self.config.model.max_output_tokens,
            ),
        )
        if response.tool_calls:
            raise VisionError("Vision model returned tool calls instead of an image description", "model_error")
        return response.text

    def _model(self) -> ModelClient:
        vision = self.config.vision
        if not vision.model and not vision.base_url and not vision.api_key:
            return self.agent_model
        if self._vision_model is None:
            self._vision_model = build_model_client(
                ModelConfig(
                    provider=self.config.model.provider,
                    base_url=vision.base_url or self.config.model.base_url,
                    model=vision.model or self.config.model.model,
                    api_key=vision.api_key or self.config.model.api_key,
                    timeout_seconds=vision.timeout_seconds,
                    max_output_tokens=self.config.model.max_output_tokens,
                )
            )
        return self._vision_model
