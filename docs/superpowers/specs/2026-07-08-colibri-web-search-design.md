# Colibri Web Search Tool Design

## Goal

Add a built-in `web.search` tool so the agent can fetch current web information through a configurable search provider.

This milestone also simplifies model credentials:

- `model.api_key` is the preferred in-config credential.
- If `model.api_key` is empty or absent, Colibri falls back to `COLIBRI_API_KEY`.

Real API keys should live in a user's private config such as `~/.colibri/config.toml`, not in committed example files.

## Constraints

- Keep Colibri usable on small headless devices.
- Do not introduce third-party packages.
- Use the Python standard library for HTTP.
- Keep the search provider interface narrow enough to add future engines.
- Treat network search as a permissioned tool call because it sends user text to an external API.

## Configuration

```toml
[model]
provider = "openai_compatible"
base_url = "https://oneapi.qunhequnhe.com/v1"
model = "ZHIPU/GLM-5.2"
api_key = ""

[web_search]
engine = "baidu"
api_key = ""
endpoint = "https://qianfan.baidubce.com/v2/ai_search/web_search"
max_results = 10
timeout_seconds = 10
```

`web_search.engine` selects the provider. The first implementation supports only `baidu`.

`web_search.api_key` is provider-specific. Colibri does not read a web search API key from the environment by default; the key belongs in private config.

## Tool

`web.search` input:

```json
{
  "query": "required search text",
  "count": 10,
  "freshness": "pd | pw | pm | py | YYYY-MM-DDtoYYYY-MM-DD"
}
```

- `query` is required.
- `count` defaults to `web_search.max_results` and is clamped to `1..50`.
- `freshness` is optional.

`web.search` returns compact JSON text. For Baidu responses, it returns `references` and removes large `snippet` fields before applying Colibri's tool result character budget.

## Baidu Provider

Endpoint:

```text
https://qianfan.baidubce.com/v2/ai_search/web_search
```

Request body:

```json
{
  "messages": [{"role": "user", "content": "query"}],
  "search_source": "baidu_search_v2",
  "resource_type_filter": [{"type": "web", "top_k": 10}],
  "search_filter": {}
}
```

Headers outside Dumate sandbox:

```text
Content-Type: application/json
Authorization: Bearer <web_search.api_key>
X-Appbuilder-From: openclaw
```

When `DUMATE_SESSION_ID` and `DUMATE_SCHEDULER_URL` are present, use the qianfan proxy path and Dumate headers so the same tool can run in that environment.

## Permissions

`web.search` has `read_only = false`.

The tool does not mutate local files, but it performs network I/O and sends the query to a provider. Marking it non-read-only makes it participate in Colibri's dynamic permission flow:

- once
- session
- executable-session
- project
- deny

## Tests

- Config loads `model.api_key` and `web_search` fields.
- OpenAI-compatible model uses `model.api_key`, then falls back to `COLIBRI_API_KEY`.
- Registry exposes `web.search` when `"web"` is enabled.
- Baidu request construction uses endpoint, headers, count, freshness, and no third-party package.
- Baidu response parsing removes `snippet` and respects tool result limits.
