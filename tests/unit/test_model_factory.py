import pytest

from colibri.config import ConfigError, ModelConfig
from colibri.model.factory import build_model_client
from colibri.model.fake import FakeModelClient
from colibri.model.openai_compatible import OpenAICompatibleModelClient


def test_factory_returns_fake_model_for_default_provider():
    client = build_model_client(ModelConfig())

    assert isinstance(client, FakeModelClient)


def test_factory_returns_openai_compatible_model(monkeypatch):
    monkeypatch.delenv("COLIBRI_API_KEY", raising=False)
    config = ModelConfig(
        provider="openai_compatible",
        base_url="https://api.example.test/v1",
        model="test-model",
        api_key="test-key",
    )

    client = build_model_client(config)

    assert isinstance(client, OpenAICompatibleModelClient)
    assert client.base_url == "https://api.example.test/v1"
    assert client.model == "test-model"
    assert client.api_key == "test-key"


def test_factory_rejects_unknown_provider():
    config = ModelConfig(provider="mystery")

    with pytest.raises(ConfigError, match="Unsupported model provider: mystery"):
        build_model_client(config)
