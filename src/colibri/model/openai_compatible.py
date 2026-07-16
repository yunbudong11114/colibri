from __future__ import annotations

from dataclasses import dataclass
import json
import os
import socket
from time import sleep
import urllib.error
import urllib.request

from colibri.config import ConfigError, ModelConfig
from colibri.messages import Message, ModelLimits, ModelResponse, ToolCall
from colibri.model.errors import ModelError


@dataclass(frozen=True)
class OpenAICompatibleModelClient:
    base_url: str
    model: str
    api_key: str
    max_retries: int = 2
    retry_backoff_ms: int = 500

    @classmethod
    def from_config(cls, config: ModelConfig) -> "OpenAICompatibleModelClient":
        api_key = config.api_key or os.environ.get("COLIBRI_API_KEY", "")
        if not api_key:
            raise ConfigError("Missing API key: set model.api_key or COLIBRI_API_KEY")
        return cls(
            base_url=config.base_url.rstrip("/"),
            model=config.model,
            api_key=api_key,
            max_retries=max(0, config.max_retries),
            retry_backoff_ms=max(0, config.retry_backoff_ms),
        )

    def complete(
        self,
        messages: list[Message],
        tools: list[dict],
        system: str,
        limits: ModelLimits,
    ) -> ModelResponse:
        payload: dict = {
            "model": self.model,
            "messages": self._api_messages(messages=messages, system=system),
            "max_completion_tokens": limits.max_output_tokens,
        }
        if tools:
            payload["tools"] = tools

        data = self._request_with_retry(
            self._chat_completions_url(), payload, limits.timeout_seconds
        )
        return self._parse_response(data)

    def complete_image(
        self,
        prompt: str,
        image_data_url: str,
        limits: ModelLimits,
    ) -> ModelResponse:
        payload = {
            "model": self.model,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": prompt},
                        {"type": "image_url", "image_url": {"url": image_data_url}},
                    ],
                }
            ],
            "max_completion_tokens": limits.max_output_tokens,
        }
        data = self._request_with_retry(
            self._chat_completions_url(), payload, limits.timeout_seconds
        )
        return self._parse_response(data)

    def _request_with_retry(self, url: str, payload: dict, timeout_seconds: int) -> dict:
        for attempt in range(self.max_retries + 1):
            try:
                return self._request_json(url, payload, timeout_seconds)
            except ModelError as error:
                if not error.retryable or attempt >= self.max_retries:
                    raise
                delay_seconds = (self.retry_backoff_ms * (2**attempt)) / 1000
                if delay_seconds > 0:
                    sleep(delay_seconds)
        raise AssertionError("unreachable")

    def _chat_completions_url(self) -> str:
        return f"{self.base_url}/chat/completions"

    def _api_messages(self, messages: list[Message], system: str) -> list[dict]:
        api_messages = []
        if system:
            api_messages.append({"role": "system", "content": system})
        api_messages.extend(self._api_message(message) for message in messages)
        return api_messages

    def _api_message(self, message: Message) -> dict:
        api_message: dict = {"role": message.role, "content": message.content}
        if message.tool_call_id:
            api_message["tool_call_id"] = message.tool_call_id
        if message.tool_calls:
            api_message["tool_calls"] = [
                {
                    "id": call.id,
                    "type": "function",
                    "function": {
                        "name": call.name,
                        "arguments": json.dumps(call.arguments),
                    },
                }
                for call in message.tool_calls
            ]
        return api_message

    def _request_json(self, url: str, payload: dict, timeout_seconds: int) -> dict:
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        request = urllib.request.Request(
            url=url,
            data=body,
            method="POST",
            headers={
                "Authorization": f"Bearer {self.api_key}",
                "Content-Type": "application/json",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=timeout_seconds) as response:
                return json.loads(response.read().decode("utf-8"))
        except urllib.error.HTTPError as error:
            return self._raise_http_error(error)
        except urllib.error.URLError as error:
            category = (
                "timeout"
                if isinstance(error.reason, (TimeoutError, socket.timeout))
                else "transient_network"
            )
            raise ModelError(
                f"Model request failed: {error.reason}", category=category
            ) from error
        except (TimeoutError, socket.timeout) as error:
            raise ModelError("Model request timed out", category="timeout") from error
        except json.JSONDecodeError as error:
            raise ModelError(
                "Model response was not valid JSON", category="invalid_response"
            ) from error

    def _raise_http_error(self, error: urllib.error.HTTPError) -> dict:
        body = error.read().decode("utf-8", errors="replace")
        compact = body[:500]
        if error.code == 408:
            category = "timeout"
        elif error.code == 429:
            category = "rate_limit"
        elif error.code >= 500:
            category = "server_error"
        else:
            category = "client_error"
        raise ModelError(
            f"Model request failed with HTTP {error.code}: {compact}",
            category=category,
        ) from error

    def _parse_response(self, data: dict) -> ModelResponse:
        choices = data.get("choices")
        if not choices:
            raise ModelError("Model response missing choices", category="invalid_response")

        message = choices[0].get("message", {})
        text = message.get("content") or ""
        tool_calls = [self._parse_tool_call(item) for item in message.get("tool_calls", [])]
        return ModelResponse(text=text, tool_calls=tool_calls)

    def _parse_tool_call(self, item: dict) -> ToolCall:
        function = item.get("function", {})
        raw_arguments = function.get("arguments") or "{}"
        try:
            arguments = json.loads(raw_arguments)
        except json.JSONDecodeError:
            arguments = {"raw": raw_arguments}
        return ToolCall(
            id=item.get("id", ""),
            name=function.get("name", ""),
            arguments=arguments,
        )
