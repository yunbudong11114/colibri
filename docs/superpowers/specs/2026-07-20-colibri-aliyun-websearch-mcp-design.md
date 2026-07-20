# Colibri Aliyun WebSearch MCP Design

Date: 2026-07-20

## Goal

Add Alibaba Cloud Model Studio's hosted WebSearch MCP service as a second
backend for the existing `web.search` tool while preserving the current tool
name, arguments, permission behavior, result bound, and Python/Rust parity.

The integration must remain suitable for low-memory devices:

- Do not start a local MCP server or an `npx` bridge.
- Do not add an MCP SDK, async runtime, or background process.
- Reuse each runtime's existing blocking HTTP implementation.
- Keep no permanent MCP connection or unbounded response cache.

## Configuration

The existing `[web_search]` fields are reused:

```toml
[web_search]
engine = "aliyun_mcp"
api_key = ""
endpoint = "https://dashscope.aliyuncs.com/api/v1/mcps/WebSearch/mcp"
max_results = 10
timeout_seconds = 10
```

`engine = "baidu"` remains the default and keeps its current behavior.

For `aliyun_mcp`, the API key is resolved in this order:

1. `web_search.api_key`
2. `DASHSCOPE_API_KEY`

The endpoint must be configured explicitly when selecting `aliyun_mcp`. This
prevents accidentally sending MCP traffic to the default Baidu endpoint.

The running REPL and gateway continue hot-reloading the complete
`[web_search]` section before the next turn.

## MCP Transport

Use MCP Streamable HTTP with protocol version `2025-06-18`. Each `web.search`
call owns a short-lived MCP session:

1. POST `initialize`.
2. Capture `Mcp-Session-Id` from the response when the server supplies one.
3. POST `notifications/initialized`.
4. POST `tools/list`.
5. Select `bailian_web_search`; if that exact name is absent, accept the only
   advertised tool. Multiple unknown tools are an invalid response.
6. POST `tools/call`.
7. Best-effort DELETE the MCP session when a session ID was supplied.

Every request sends:

- `Authorization: Bearer <key>`
- `Accept: application/json, text/event-stream`
- `MCP-Protocol-Version: 2025-06-18`
- `Mcp-Session-Id` after initialization when available

POST responses may be either one JSON object or an SSE stream. The client
extracts `data:` events and selects the JSON-RPC response with the matching
request ID. Notifications may return an empty `202` response.

A `404` after initialization is reported as a network/protocol failure for the
current tool call. The next invocation starts a fresh session automatically.

## Tool Arguments

The public Colibri schema remains:

- `query`: required non-empty string.
- `count`: optional integer, clamped to `1..50`.
- `freshness`: optional existing Colibri freshness syntax.

The MCP tool arguments are built from its advertised `inputSchema`:

- Always pass `query`.
- Pass `count`, clamped again to `1..20`, only when the MCP tool advertises a
  `count` property.
- Pass `freshness` only when the MCP tool advertises a `freshness` property.

This avoids sending unsupported properties to the hosted service while keeping
the Colibri tool interface stable across engines.

## Result Mapping

For a successful `tools/call` response:

- Join text blocks from `result.content` in order.
- If no text block exists, serialize `result.structuredContent` when present.
- Treat `result.isError = true` as `api_error`.
- Treat missing usable content as `invalid_response`.
- Apply `tools.max_result_chars` exactly as the Baidu backend does.

The model receives the MCP result through the existing `tool_result` path. No
second model call is introduced.

## Error Mapping

- Unsupported engine or missing key/endpoint: `invalid_config`
- Invalid Colibri arguments: `invalid_arguments`
- HTTP status failure: `http_error`
- Timeout: `timeout`
- Transport failure: `network_error`
- JSON-RPC error or MCP tool error: `api_error`
- Invalid JSON/SSE, missing tool list, or missing content: `invalid_response`

Secrets must not appear in tool results, transcript payloads, or logs.

## Verification

Python and Rust tests must cover:

- Existing Baidu request and response behavior remains unchanged.
- API key fallback to `DASHSCOPE_API_KEY`.
- MCP initialize, initialized notification, tool discovery, and tool call.
- Session ID forwarding.
- JSON and SSE JSON-RPC responses.
- Dynamic omission of unsupported `freshness`.
- Text result extraction and result bounding.
- JSON-RPC and malformed-response errors.
- Configuration/default parity and runtime hot reload.

