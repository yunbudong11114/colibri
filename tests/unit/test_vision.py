from pathlib import Path

import pytest

from colibri.config import AgentConfig
from colibri.media import MediaPart
from colibri.messages import ModelLimits, ModelResponse, ToolCall
from colibri.model.fake import FakeModelClient
from colibri.model.openai_compatible import OpenAICompatibleModelClient
from colibri.session import AgentSession
from colibri.tools.base import ToolContext
from colibri.tools.builtin.image import ImageUnderstandTool
from colibri.vision import ImageAnalyzer


def test_vision_config_falls_back_to_agent_model():
    config = AgentConfig.default().with_overrides(
        {"model": {"provider": "openai_compatible", "model": "agent-model", "base_url": "https://agent.test/v1"}}
    )

    assert config.vision.model == ""
    assert config.vision.base_url == ""
    assert config.vision.max_image_bytes == 4 * 1024 * 1024


def test_image_tool_reads_allowed_image_without_permission_prompt(tmp_path):
    image = tmp_path / "photo.png"
    image.write_bytes(b"not-really-a-png")
    config = AgentConfig.default().with_overrides({"files": {"roots": [str(tmp_path)]}})
    calls = []
    context = ToolContext(
        config=config,
        cwd=tmp_path,
        image_analyzer=lambda path, prompt: calls.append((path, prompt)) or "looks good",
    )

    result = ImageUnderstandTool().run(
        {"path": str(image), "prompt": "describe"},
        context,
    )

    assert result.ok
    assert result.text == "looks good"
    assert calls == [(image.resolve(), "describe")]


def test_image_tool_rejects_non_image_file(tmp_path):
    text_file = tmp_path / "note.txt"
    text_file.write_text("hello", encoding="utf-8")
    config = AgentConfig.default().with_overrides({"files": {"roots": [str(tmp_path)]}})

    result = ImageUnderstandTool().run(
        {"path": str(text_file), "prompt": "describe"},
        ToolContext(config=config, cwd=tmp_path, image_analyzer=lambda path, prompt: "unused"),
    )

    assert not result.ok
    assert result.error_type == "invalid_media"


def test_image_analyzer_builds_bounded_data_url(tmp_path):
    image = tmp_path / "photo.png"
    image.write_bytes(b"png-bytes")
    captured = {}

    class FakeVisionModel(FakeModelClient):
        def complete_image(self, prompt, image_data_url, limits):
            captured.update(prompt=prompt, image_data_url=image_data_url, limits=limits)
            return ModelResponse(text="understood")

    config = AgentConfig.default().with_overrides({"files": {"roots": [str(tmp_path)]}})
    result = ImageUnderstandTool().run(
        {"path": str(image), "prompt": "describe"},
        ToolContext(
            config=config,
            cwd=tmp_path,
            image_analyzer=ImageAnalyzer(config, FakeVisionModel()),
        ),
    )

    assert result.ok
    assert result.text == "understood"
    assert captured["prompt"] == "describe"
    assert captured["image_data_url"].startswith("data:image/png;base64,")


def test_image_analyzer_rejects_oversized_image(tmp_path):
    image = tmp_path / "photo.png"
    image.write_bytes(b"12345")
    config = AgentConfig.default().with_overrides(
        {"files": {"roots": [str(tmp_path)]}, "vision": {"max_image_bytes": 4}}
    )

    result = ImageUnderstandTool().run(
        {"path": str(image), "prompt": "describe"},
        ToolContext(config=config, cwd=tmp_path, image_analyzer=ImageAnalyzer(config, FakeModelClient())),
    )

    assert not result.ok
    assert result.error_type == "image_too_large"


def test_openai_compatible_image_request_uses_multimodal_content(monkeypatch):
    client = OpenAICompatibleModelClient(
        base_url="https://api.example.test/v1",
        model="vision-model",
        api_key="test-key",
    )
    captured = {}

    def fake_request(self, url, payload, timeout_seconds):
        captured.update(url=url, payload=payload, timeout_seconds=timeout_seconds)
        return {"choices": [{"message": {"content": "an image"}}]}

    monkeypatch.setattr(OpenAICompatibleModelClient, "_request_json", fake_request)

    response = client.complete_image(
        "describe this",
        "data:image/png;base64,cG5n",
        ModelLimits(timeout_seconds=7, max_output_tokens=80),
    )

    assert response.text == "an image"
    assert captured["payload"]["messages"][0]["content"] == [
        {"type": "text", "text": "describe this"},
        {"type": "image_url", "image_url": {"url": "data:image/png;base64,cG5n"}},
    ]


def test_session_can_call_image_tool_after_media_path_is_received(tmp_path):
    image = tmp_path / "photo.png"
    image.write_bytes(b"png")
    config = AgentConfig.default().with_overrides(
        {"files": {"roots": [str(tmp_path)]}, "tools": {"enabled": ["image"]}}
    )

    class ScriptedImageModel(FakeModelClient):
        def __init__(self):
            self.calls = 0

        def complete(self, messages, tools, system, limits):
            self.calls += 1
            if self.calls == 1:
                assert any(item["function"]["name"] == "image.understand" for item in tools)
                return ModelResponse(
                    text="",
                    tool_calls=[
                        ToolCall(
                            id="image-1",
                            name="image.understand",
                            arguments={"path": str(image), "prompt": "describe"},
                        )
                    ],
                )
            tool_result = next(message.content for message in messages if message.role == "tool")
            return ModelResponse(text=f"answer: {tool_result}")

    session = AgentSession(config=config, model=ScriptedImageModel())
    response = session.submit(
        "what is this?",
        media=[MediaPart(type="image", path=image, filename="photo.png", content_type="image/png")],
    )

    assert "fake image: describe" in response.text
