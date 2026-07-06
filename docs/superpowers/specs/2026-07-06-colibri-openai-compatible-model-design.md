# Colibri OpenAI-Compatible Model Adapter Design

Date: 2026-07-06
Status: Draft for review
Milestone: 2
Scope: Real model adapter only

## 1. Goal

Milestone 2 turns Colibri from a fake-model skeleton into a CLI runtime that can call a real OpenAI-compatible chat completion API.

The milestone should preserve the current `ModelClient` boundary, keep runtime dependencies at zero beyond the Python standard library, and avoid implementing the full tool system yet. After this milestone, `colibri ask "hello"` and `colibri repl` should be able to use either the existing fake model or a configured OpenAI-compatible model.

## 2. Non-Goals

- Do not implement shell, file, HTTP, memory, skills, GPIO, or MCP tools.
- Do not implement the full bounded tool loop.
- Do not add the official OpenAI Python SDK as a runtime dependency.
- Do not implement streaming responses.
- Do not implement retries, backoff, rate-limit scheduling, or token accounting beyond basic request timeouts.
- Do not store API keys in config files or transcripts.

## 3. API Surface Choice

Use an OpenAI-compatible `/chat/completions` request shape for this milestone.

Rationale:

- The current `ModelClient.complete()` interface already accepts a list of role/content messages and returns a single assistant response.
- The existing `ModelResponse.tool_calls` field can carry parsed function/tool calls later without changing the session interface.
- Many third-party providers expose an OpenAI-compatible chat completions endpoint, making this a useful first adapter for Colibri.
- The OpenAI API reference documents Chat Completions as generating model responses from conversation messages, while Responses API is richer and more agent-native. Colibri can add a Responses adapter later if needed.

The adapter should construct requests against:

```text
{base_url}/chat/completions
```

where `base_url` comes from `config.model.base_url` and defaults to `https://api.openai.com/v1`.

## 4. Configuration

Reuse the existing `ModelConfig` fields:

```python
provider: str = "fake"
base_url: str = "https://api.openai.com/v1"
model: str = "fake-colibri-model"
api_key_env: str = "OPENAI_API_KEY"
timeout_seconds: int = 60
max_output_tokens: int = 1024
```

Provider behavior:

- `provider = "fake"` creates `FakeModelClient`.
- `provider = "openai_compatible"` creates `OpenAICompatibleModelClient`.
- Any other provider value is a configuration error with a short, actionable message.

API key behavior:

- Read the key from `os.environ[config.model.api_key_env]`.
- If the environment variable is missing or empty, fail before creating the HTTP request.
- Error text should name the missing environment variable but must not include secret values.

Example config:

```toml
[model]
provider = "openai_compatible"
base_url = "https://api.openai.com/v1"
model = "gpt-5.5"
api_key_env = "OPENAI_API_KEY"
timeout_seconds = 60
max_output_tokens = 1024
```

## 5. Components

### 5.1 `model/factory.py`

Create a small factory so CLI code does not decide provider details.

Interface:

```python
def build_model_client(config: ModelConfig) -> ModelClient: ...
```

Responsibilities:

- Return `FakeModelClient` for `provider = "fake"`.
- Return `OpenAICompatibleModelClient.from_config(config)` for `provider = "openai_compatible"`.
- Raise `ConfigError` for unsupported providers.

### 5.2 `model/openai_compatible.py`

Create the adapter using only standard-library HTTP.

Suggested public interface:

```python
@dataclass(frozen=True)
class OpenAICompatibleModelClient:
    base_url: str
    model: str
    api_key: str

    @classmethod
    def from_config(cls, config: ModelConfig) -> "OpenAICompatibleModelClient": ...

    def complete(
        self,
        messages: list[Message],
        tools: list[dict],
        system: str,
        limits: ModelLimits,
    ) -> ModelResponse: ...
```

Responsibilities:

- Convert Colibri `Message` objects into API messages.
- Prepend the session `system` prompt as a system message when non-empty.
- Send a JSON `POST` request to `/chat/completions`.
- Include `Authorization: Bearer <api_key>` and `Content-Type: application/json`.
- Include `model`, `messages`, and output limit fields in the request body.
- Parse response JSON and return `ModelResponse(text=assistant_text, tool_calls=parsed_tool_calls)`.

The first implementation may ignore `tools` when it is empty. If `tools` is non-empty before the real tool system exists, pass them through using the OpenAI-compatible `tools` field and parse returned tool calls, but do not execute them in `AgentSession`.

### 5.3 `config.py`

Add a small `ConfigError` exception class if needed for clear user-facing configuration failures.

Do not expand `ModelConfig` unless implementation discovers a required field that cannot fit the current model settings.

### 5.4 `cli.py`

Replace direct construction of `FakeModelClient()` with `build_model_client(config.model)`.

CLI behavior:

- `colibri ask "hello"` still works with default fake config.
- `colibri --config configs/openai.toml ask "hello"` uses the configured real adapter.
- Config errors print a short message and return a non-zero exit code.
- Unexpected programming errors may still raise during development.

## 6. Request Shape

For a simple user prompt, the adapter sends:

```json
{
  "model": "gpt-5.5",
  "messages": [
    {"role": "system", "content": "You are a lightweight personal agent..."},
    {"role": "user", "content": "hello"}
  ],
  "max_completion_tokens": 1024
}
```

Compatibility note:

- Prefer `max_completion_tokens` for current OpenAI chat completions.
- If a provider rejects that field, users can set `base_url` for providers that support OpenAI-compatible chat completions. Provider-specific compatibility shims are out of scope for this milestone.

## 7. Response Parsing

The adapter should accept the standard chat completion response shape:

```json
{
  "choices": [
    {
      "message": {
        "role": "assistant",
        "content": "Hello",
        "tool_calls": []
      }
    }
  ]
}
```

Parsing rules:

- Use `choices[0].message.content` as assistant text.
- Treat missing or `null` content as an empty string if tool calls are present.
- Parse `message.tool_calls` into existing `ToolCall` dataclasses when present.
- Raise `ModelError` if `choices` is missing or empty.
- Include HTTP status and compact error body text in `ModelError` for non-2xx responses.

## 8. Error Handling

Add a small model-layer exception:

```python
class ModelError(RuntimeError):
    pass
```

Expected failures:

- Missing API key environment variable.
- Invalid `base_url`.
- Network error or timeout.
- Non-2xx HTTP response.
- Malformed JSON response.
- Response JSON missing `choices`.

CLI should catch `ConfigError` and `ModelError`, print one concise line, and return exit code `1`.

Do not print stack traces for expected configuration or API failures.

## 9. Testing

Tests should avoid real network calls.

Required tests:

- Factory returns fake model for default config.
- Factory returns OpenAI-compatible client for `provider = "openai_compatible"`.
- Factory rejects unknown provider.
- Missing API key raises a clear error naming the expected environment variable.
- Adapter builds a valid JSON request with system and user messages.
- Adapter parses assistant text.
- Adapter parses tool calls without executing them.
- Adapter turns non-2xx responses into `ModelError`.
- CLI default fake path still passes.
- CLI returns exit code `1` for expected model/config errors.

Use fake HTTP callables or monkeypatch the low-level request helper rather than calling the network.

## 10. Validation

Minimum local validation:

```bash
python -m pytest
PYTHONPATH=src python -m colibri.cli ask "hello"
```

Optional real API smoke test, only when the user provides an API key in the environment:

```bash
PYTHONPATH=src OPENAI_API_KEY=... python -m colibri.cli --config configs/openai.example.toml ask "say hi in five words"
```

The real API smoke test should not be required in CI or routine local validation.

## 11. Future Work

After this milestone, the next good slice is the tool-system milestone:

- `ToolSpec`
- tool registry
- bounded tool loop in `AgentSession`
- read-only shell and file tools
- permission confirmation for writes and risky commands
- JSONL transcript logging
