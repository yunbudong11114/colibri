import json
import urllib.error

import pytest

from colibri.config import ConfigError, ModelConfig
from colibri.messages import Message, ModelLimits, ModelUsage, ToolCall
from colibri.model.errors import ModelError
from colibri.model.openai_compatible import OpenAICompatibleModelClient


def make_client() -> OpenAICompatibleModelClient:
    return OpenAICompatibleModelClient(
        base_url="https://api.example.test/v1",
        model="test-model",
        api_key="test-key",
    )


def test_from_config_requires_api_key(monkeypatch):
    monkeypatch.delenv("COLIBRI_API_KEY", raising=False)
    config = ModelConfig(provider="openai_compatible")

    with pytest.raises(ConfigError, match="model.api_key or COLIBRI_API_KEY"):
        OpenAICompatibleModelClient.from_config(config)


def test_from_config_prefers_config_api_key(monkeypatch):
    monkeypatch.setenv("COLIBRI_API_KEY", "env-key")
    config = ModelConfig(provider="openai_compatible", api_key="config-key")

    client = OpenAICompatibleModelClient.from_config(config)

    assert client.api_key == "config-key"


def test_from_config_falls_back_to_colibri_api_key(monkeypatch):
    monkeypatch.setenv("COLIBRI_API_KEY", "env-key")
    config = ModelConfig(provider="openai_compatible")

    client = OpenAICompatibleModelClient.from_config(config)

    assert client.api_key == "env-key"


def test_complete_builds_chat_completion_request(monkeypatch):
    client = make_client()
    captured = {}

    def fake_request(self, url, payload, timeout_seconds):
        captured["url"] = url
        captured["payload"] = payload
        captured["timeout_seconds"] = timeout_seconds
        return {"choices": [{"message": {"content": "hi there"}}]}

    monkeypatch.setattr(OpenAICompatibleModelClient, "_request_json", fake_request)

    response = client.complete(
        messages=[Message(role="user", content="hello")],
        tools=[],
        system="system prompt",
        limits=ModelLimits(timeout_seconds=12, max_output_tokens=34),
    )

    assert response.text == "hi there"
    assert captured["url"] == "https://api.example.test/v1/chat/completions"
    assert captured["timeout_seconds"] == 12
    assert captured["payload"] == {
        "model": "test-model",
        "messages": [
            {"role": "system", "content": "system prompt"},
            {"role": "user", "content": "hello"},
        ],
        "max_completion_tokens": 34,
    }


def test_complete_passes_tools_when_present(monkeypatch):
    client = make_client()
    captured = {}
    tools = [{"type": "function", "function": {"name": "lookup", "parameters": {"type": "object"}}}]

    def fake_request(self, url, payload, timeout_seconds):
        captured["payload"] = payload
        return {"choices": [{"message": {"content": "done"}}]}

    monkeypatch.setattr(OpenAICompatibleModelClient, "_request_json", fake_request)

    client.complete(
        messages=[Message(role="user", content="use a tool")],
        tools=tools,
        system="",
        limits=ModelLimits(timeout_seconds=5, max_output_tokens=20),
    )

    assert captured["payload"]["tools"] == tools
    assert captured["payload"]["messages"] == [{"role": "user", "content": "use a tool"}]


def test_complete_serializes_tool_result_messages(monkeypatch):
    client = make_client()
    captured = {}

    def fake_request(self, url, payload, timeout_seconds):
        captured["payload"] = payload
        return {"choices": [{"message": {"content": "done"}}]}

    monkeypatch.setattr(OpenAICompatibleModelClient, "_request_json", fake_request)

    client.complete(
        messages=[Message(role="tool", content="file contents", tool_call_id="call_1")],
        tools=[],
        system="",
        limits=ModelLimits(timeout_seconds=5, max_output_tokens=20),
    )

    assert captured["payload"]["messages"] == [
        {"role": "tool", "content": "file contents", "tool_call_id": "call_1"}
    ]


def test_complete_serializes_assistant_tool_calls(monkeypatch):
    client = make_client()
    captured = {}
    tool_call = ToolCall(id="call_1", name="files.read", arguments={"path": "/tmp/a.txt"})

    def fake_request(self, url, payload, timeout_seconds):
        captured["payload"] = payload
        return {"choices": [{"message": {"content": "done"}}]}

    monkeypatch.setattr(OpenAICompatibleModelClient, "_request_json", fake_request)

    client.complete(
        messages=[Message(role="assistant", content="", tool_calls=[tool_call])],
        tools=[],
        system="",
        limits=ModelLimits(timeout_seconds=5, max_output_tokens=20),
    )

    assert captured["payload"]["messages"] == [
        {
            "role": "assistant",
            "content": "",
            "tool_calls": [
                {
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "files_read",
                        "arguments": "{\"path\": \"/tmp/a.txt\"}",
                    },
                }
            ],
        }
    ]


def test_complete_maps_dotted_tool_names_in_both_directions(monkeypatch):
    client = make_client()
    captured = {}
    tools = [
        {
            "type": "function",
            "function": {
                "name": "files.read",
                "description": "Read a file",
                "parameters": {"type": "object"},
            },
        }
    ]

    def fake_request(self, url, payload, timeout_seconds):
        captured["payload"] = payload
        return {
            "choices": [
                {
                    "message": {
                        "content": None,
                        "tool_calls": [
                            {
                                "id": "call_1",
                                "type": "function",
                                "function": {
                                    "name": "files_read",
                                    "arguments": "{\"path\": \"notes.txt\"}",
                                },
                            }
                        ],
                    }
                }
            ]
        }

    monkeypatch.setattr(OpenAICompatibleModelClient, "_request_json", fake_request)

    response = client.complete(
        messages=[Message(role="user", content="read notes")],
        tools=tools,
        system="",
        limits=ModelLimits(timeout_seconds=5, max_output_tokens=20),
    )

    assert captured["payload"]["tools"][0]["function"]["name"] == "files_read"
    assert tools[0]["function"]["name"] == "files.read"
    assert response.tool_calls[0].name == "files.read"


def test_complete_keeps_colliding_tool_names_distinct(monkeypatch):
    client = make_client()
    captured = {}
    tools = [
        {"type": "function", "function": {"name": name, "parameters": {"type": "object"}}}
        for name in ["a.b", "a_b"]
    ]

    def fake_request(self, url, payload, timeout_seconds):
        captured["payload"] = payload
        encoded_name = payload["tools"][0]["function"]["name"]
        return {
            "choices": [
                {
                    "message": {
                        "tool_calls": [
                            {
                                "id": "call_1",
                                "type": "function",
                                "function": {"name": encoded_name, "arguments": "{}"},
                            }
                        ]
                    }
                }
            ]
        }

    monkeypatch.setattr(OpenAICompatibleModelClient, "_request_json", fake_request)

    response = client.complete(
        messages=[Message(role="user", content="use a tool")],
        tools=tools,
        system="",
        limits=ModelLimits(timeout_seconds=5, max_output_tokens=20),
    )

    encoded_names = [tool["function"]["name"] for tool in captured["payload"]["tools"]]
    assert encoded_names[1] == "a_b"
    assert encoded_names[0].startswith("a_b_")
    assert len(set(encoded_names)) == 2
    assert response.tool_calls[0].name == "a.b"


def test_complete_parses_tool_calls(monkeypatch):
    client = make_client()

    def fake_request(self, url, payload, timeout_seconds):
        return {
            "choices": [
                {
                    "message": {
                        "content": None,
                        "tool_calls": [
                            {
                                "id": "call_1",
                                "type": "function",
                                "function": {
                                    "name": "lookup",
                                    "arguments": "{\"city\": \"Shanghai\"}",
                                },
                            }
                        ],
                    }
                }
            ]
        }

    monkeypatch.setattr(OpenAICompatibleModelClient, "_request_json", fake_request)

    response = client.complete(
        messages=[Message(role="user", content="weather")],
        tools=[],
        system="",
        limits=ModelLimits(timeout_seconds=5, max_output_tokens=20),
    )

    assert response.text == ""
    assert len(response.tool_calls) == 1
    assert response.tool_calls[0].id == "call_1"
    assert response.tool_calls[0].name == "lookup"
    assert response.tool_calls[0].arguments == {"city": "Shanghai"}


def test_complete_parses_finish_reason_and_usage(monkeypatch):
    client = make_client()

    monkeypatch.setattr(
        OpenAICompatibleModelClient,
        "_request_json",
        lambda self, url, payload, timeout_seconds: {
            "choices": [{"finish_reason": "length", "message": {"content": "partial"}}],
            "usage": {
                "prompt_tokens": 120,
                "completion_tokens": 20,
                "total_tokens": 140,
                "prompt_tokens_details": {"cached_tokens": 30, "cache_write_tokens": 10},
            },
        },
    )

    response = client.complete(
        messages=[Message(role="user", content="hello")],
        tools=[],
        system="",
        limits=ModelLimits(timeout_seconds=5, max_output_tokens=20),
    )

    assert response.stop_reason == "length"
    assert response.usage == ModelUsage(
        input_tokens=80,
        output_tokens=20,
        cache_read_tokens=30,
        cache_write_tokens=10,
        total_tokens=140,
    )


def test_complete_ignores_malformed_usage(monkeypatch):
    client = make_client()
    monkeypatch.setattr(
        OpenAICompatibleModelClient,
        "_request_json",
        lambda self, url, payload, timeout_seconds: {
            "choices": [{"message": {"content": "ok"}}],
            "usage": {"prompt_tokens": "unknown"},
        },
    )

    response = client.complete(
        messages=[Message(role="user", content="hello")],
        tools=[],
        system="",
        limits=ModelLimits(timeout_seconds=5, max_output_tokens=20),
    )

    assert response.usage is None


def test_complete_rejects_empty_choices(monkeypatch):
    client = make_client()
    monkeypatch.setattr(
        OpenAICompatibleModelClient,
        "_request_json",
        lambda self, url, payload, timeout_seconds: {"choices": []},
    )

    with pytest.raises(ModelError, match="missing choices"):
        client.complete(
            messages=[Message(role="user", content="hello")],
            tools=[],
            system="",
            limits=ModelLimits(timeout_seconds=5, max_output_tokens=20),
        )


def test_complete_retries_transient_error_then_succeeds(monkeypatch):
    client = OpenAICompatibleModelClient(
        base_url="https://api.example.test/v1",
        model="test-model",
        api_key="test-key",
        max_retries=2,
        retry_backoff_ms=500,
    )
    attempts = []
    delays = []

    def fake_request(self, url, payload, timeout_seconds):
        attempts.append(url)
        if len(attempts) < 3:
            raise ModelError("network down", category="transient_network")
        return {"choices": [{"message": {"content": "recovered"}}]}

    monkeypatch.setattr(OpenAICompatibleModelClient, "_request_json", fake_request)
    monkeypatch.setattr("colibri.model.openai_compatible.sleep", delays.append)

    response = client.complete(
        messages=[Message(role="user", content="hello")],
        tools=[],
        system="",
        limits=ModelLimits(timeout_seconds=5, max_output_tokens=20),
    )

    assert response.text == "recovered"
    assert len(attempts) == 3
    assert delays == [0.5, 1.0]


def test_complete_does_not_retry_permanent_error(monkeypatch):
    client = OpenAICompatibleModelClient(
        base_url="https://api.example.test/v1",
        model="test-model",
        api_key="test-key",
        max_retries=2,
        retry_backoff_ms=500,
    )
    attempts = []

    def fake_request(self, url, payload, timeout_seconds):
        attempts.append(url)
        raise ModelError("bad auth", category="client_error")

    monkeypatch.setattr(OpenAICompatibleModelClient, "_request_json", fake_request)

    with pytest.raises(ModelError, match="bad auth"):
        client.complete(
            messages=[Message(role="user", content="hello")],
            tools=[],
            system="",
            limits=ModelLimits(timeout_seconds=5, max_output_tokens=20),
        )

    assert len(attempts) == 1


def test_complete_respects_zero_retries(monkeypatch):
    client = OpenAICompatibleModelClient(
        base_url="https://api.example.test/v1",
        model="test-model",
        api_key="test-key",
        max_retries=0,
        retry_backoff_ms=500,
    )
    attempts = []

    def fake_request(self, url, payload, timeout_seconds):
        attempts.append(url)
        raise ModelError("network down", category="transient_network")

    monkeypatch.setattr(OpenAICompatibleModelClient, "_request_json", fake_request)

    with pytest.raises(ModelError):
        client.complete(
            messages=[Message(role="user", content="hello")],
            tools=[],
            system="",
            limits=ModelLimits(timeout_seconds=5, max_output_tokens=20),
        )

    assert len(attempts) == 1


def test_request_json_turns_http_error_into_model_error():
    client = make_client()
    body = json.dumps({"error": {"message": "bad auth"}}).encode("utf-8")
    error = urllib.error.HTTPError(
        url="https://api.example.test/v1/chat/completions",
        code=401,
        msg="Unauthorized",
        hdrs={},
        fp=FakeErrorBody(body),
    )

    with pytest.raises(ModelError, match="HTTP 401") as caught:
        client._raise_http_error(error)

    assert caught.value.category == "client_error"
    assert not caught.value.retryable


def test_http_context_overflow_is_classified_without_transport_retry():
    client = make_client()
    body = json.dumps({"error": {"code": "context_length_exceeded"}}).encode("utf-8")
    error = urllib.error.HTTPError(
        url="https://api.example.test/v1/chat/completions",
        code=400,
        msg="Bad Request",
        hdrs={},
        fp=FakeErrorBody(body),
    )

    with pytest.raises(ModelError) as caught:
        client._raise_http_error(error)

    assert caught.value.category == "context_overflow"
    assert not caught.value.retryable


@pytest.mark.parametrize(
    ("status", "category", "retryable"),
    [
        (408, "timeout", True),
        (429, "rate_limit", True),
        (500, "server_error", True),
        (503, "server_error", True),
        (400, "client_error", False),
        (403, "client_error", False),
    ],
)
def test_http_error_classification(status, category, retryable):
    client = make_client()
    error = urllib.error.HTTPError(
        url="https://api.example.test/v1/chat/completions",
        code=status,
        msg="error",
        hdrs={},
        fp=FakeErrorBody(b"failure"),
    )

    with pytest.raises(ModelError) as caught:
        client._raise_http_error(error)

    assert caught.value.category == category
    assert caught.value.retryable is retryable


def test_request_json_preserves_chinese_utf8(monkeypatch):
    client = make_client()
    captured = {}

    class FakeResponse:
        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, tb):
            return None

        def read(self):
            return json.dumps({"choices": [{"message": {"content": "ok"}}]}).encode("utf-8")

    def fake_urlopen(request, timeout):
        captured["body"] = request.data
        captured["timeout"] = timeout
        return FakeResponse()

    monkeypatch.setattr("colibri.model.openai_compatible.urllib.request.urlopen", fake_urlopen)

    data = client._request_json(
        "https://api.example.test/v1/chat/completions",
        {"messages": [{"role": "user", "content": "杭州天气"}]},
        timeout_seconds=9,
    )

    assert data["choices"][0]["message"]["content"] == "ok"
    assert captured["timeout"] == 9
    assert "杭州天气".encode("utf-8") in captured["body"]
    assert b"\\u676d" not in captured["body"]


class FakeErrorBody:
    def __init__(self, body: bytes):
        self.body = body

    def read(self) -> bytes:
        return self.body

    def close(self) -> None:
        return None
