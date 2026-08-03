from __future__ import annotations

from dataclasses import dataclass
from hashlib import md5
import json
import os
import re
import socket
from time import sleep
import urllib.error
import urllib.request

from colibri.config import ConfigError, ModelConfig
from colibri.messages import Message, ModelLimits, ModelResponse, ToolCall
from colibri.model.errors import ModelError


_API_TOOL_NAME_PATTERN = re.compile(r"^[A-Za-z][A-Za-z0-9_-]*$")
_API_TOOL_NAME_MAX_LENGTH = 64


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
        tool_names = _ToolNameMap.build(tools=tools, messages=messages)
        payload: dict = {
            "model": self.model,
            "messages": self._api_messages(
                messages=messages,
                system=system,
                tool_names=tool_names,
            ),
            "max_completion_tokens": limits.max_output_tokens,
        }
        if tools:
            payload["tools"] = tool_names.encode_tools(tools)

        data = self._request_with_retry(
            self._chat_completions_url(), payload, limits.timeout_seconds
        )
        return self._parse_response(data, tool_names=tool_names)

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

    def _api_messages(
        self,
        messages: list[Message],
        system: str,
        tool_names: _ToolNameMap,
    ) -> list[dict]:
        api_messages = []
        if system:
            api_messages.append({"role": "system", "content": system})
        api_messages.extend(self._api_message(message, tool_names) for message in messages)
        return api_messages

    def _api_message(self, message: Message, tool_names: _ToolNameMap) -> dict:
        api_message: dict = {"role": message.role, "content": message.content}
        if message.tool_call_id:
            api_message["tool_call_id"] = message.tool_call_id
        if message.tool_calls:
            api_message["tool_calls"] = [
                {
                    "id": call.id,
                    "type": "function",
                    "function": {
                        "name": tool_names.encode(call.name),
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

    def _parse_response(
        self,
        data: dict,
        tool_names: _ToolNameMap | None = None,
    ) -> ModelResponse:
        choices = data.get("choices")
        if not choices:
            raise ModelError("Model response missing choices", category="invalid_response")

        message = choices[0].get("message", {})
        text = message.get("content") or ""
        tool_calls = [
            self._parse_tool_call(item, tool_names=tool_names)
            for item in message.get("tool_calls", [])
        ]
        return ModelResponse(text=text, tool_calls=tool_calls)

    def _parse_tool_call(
        self,
        item: dict,
        tool_names: _ToolNameMap | None = None,
    ) -> ToolCall:
        function = item.get("function", {})
        raw_arguments = function.get("arguments") or "{}"
        try:
            arguments = json.loads(raw_arguments)
        except json.JSONDecodeError:
            arguments = {"raw": raw_arguments}
        return ToolCall(
            id=item.get("id", ""),
            name=(tool_names or _ToolNameMap.empty()).decode(function.get("name", "")),
            arguments=arguments,
        )


@dataclass(frozen=True)
class _ToolNameMap:
    internal_to_api: dict[str, str]
    api_to_internal: dict[str, str]

    @classmethod
    def empty(cls) -> "_ToolNameMap":
        return cls(internal_to_api={}, api_to_internal={})

    @classmethod
    def build(cls, tools: list[dict], messages: list[Message]) -> "_ToolNameMap":
        names = {
            name
            for tool in tools
            if isinstance(tool, dict)
            for function in [tool.get("function")]
            if isinstance(function, dict)
            for name in [function.get("name")]
            if isinstance(name, str)
        }
        names.update(
            call.name
            for message in messages
            for call in message.tool_calls
            if call.name
        )

        internal_to_api: dict[str, str] = {}
        used: set[str] = set()
        legal_names = sorted(name for name in names if _is_legal_api_tool_name(name))
        for name in legal_names:
            internal_to_api[name] = name
            used.add(name)
        for name in sorted(names - set(legal_names)):
            api_name = _unique_api_tool_name(name, used)
            internal_to_api[name] = api_name
            used.add(api_name)

        return cls(
            internal_to_api=internal_to_api,
            api_to_internal={api_name: name for name, api_name in internal_to_api.items()},
        )

    def encode(self, name: str) -> str:
        return self.internal_to_api.get(name, name)

    def decode(self, name: str) -> str:
        return self.api_to_internal.get(name, name)

    def encode_tools(self, tools: list[dict]) -> list[dict]:
        encoded = []
        for tool in tools:
            function = tool.get("function")
            if not isinstance(function, dict):
                encoded.append(tool)
                continue
            encoded_function = dict(function)
            name = encoded_function.get("name")
            if isinstance(name, str):
                encoded_function["name"] = self.encode(name)
            encoded.append({**tool, "function": encoded_function})
        return encoded


def _is_legal_api_tool_name(name: str) -> bool:
    return len(name) <= _API_TOOL_NAME_MAX_LENGTH and bool(
        _API_TOOL_NAME_PATTERN.fullmatch(name)
    )


def _unique_api_tool_name(name: str, used: set[str]) -> str:
    candidate = re.sub(r"[^A-Za-z0-9_-]+", "_", name)
    if not candidate or not candidate[0].isalpha() or not candidate[0].isascii():
        candidate = f"tool_{candidate}"
    candidate = candidate[:_API_TOOL_NAME_MAX_LENGTH]
    if candidate not in used:
        return candidate

    digest = md5(name.encode("utf-8"), usedforsecurity=False).hexdigest()[:12]
    for attempt in range(len(used) + 1):
        suffix = f"_{digest}" if attempt == 0 else f"_{digest}_{attempt}"
        hashed = f"{candidate[:_API_TOOL_NAME_MAX_LENGTH - len(suffix)]}{suffix}"
        if hashed not in used:
            return hashed
    raise ValueError(f"Could not create a unique API tool name for {name!r}")
