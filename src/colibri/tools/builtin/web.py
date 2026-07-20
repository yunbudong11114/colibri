from __future__ import annotations

from datetime import datetime, timedelta
import json
import os
import re
import socket
from typing import Any
import urllib.error
import urllib.request
from urllib.parse import urlparse

from colibri.config import WebSearchConfig
from colibri.tools.base import ToolContext, ToolResult, ToolSpec, bound_tool_text


_FRESHNESS_RANGE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}to\d{4}-\d{2}-\d{2}$")
_BAIDU_DEFAULT_ENDPOINT = "https://qianfan.baidubce.com/v2/ai_search/web_search"
_MCP_PROTOCOL_VERSION = "2025-06-18"
_ALIYUN_MCP_TOOL_NAME = "bailian_web_search"


class WebSearchTool:
    spec = ToolSpec(
        name="web.search",
        description="Search the web using the configured search engine.",
        input_schema={
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query."},
                "count": {
                    "type": "integer",
                    "description": "Maximum number of web results. Defaults to config.web_search.max_results.",
                },
                "freshness": {
                    "type": "string",
                    "description": "Optional freshness filter: pd, pw, pm, py, or YYYY-MM-DDtoYYYY-MM-DD.",
                },
            },
            "required": ["query"],
            "additionalProperties": False,
        },
        read_only=False,
    )

    def run(self, arguments: dict[str, Any], context: ToolContext) -> ToolResult:
        config = context.config.web_search
        if config.engine not in {"baidu", "aliyun_mcp"}:
            return ToolResult(ok=False, text=f"Unsupported web search engine: {config.engine}", error_type="invalid_config")

        query = arguments.get("query")
        if not isinstance(query, str) or not query.strip():
            return ToolResult(ok=False, text="web.search requires a non-empty string query", error_type="invalid_arguments")

        try:
            count = _result_count(arguments.get("count"), config.max_results)
            freshness = arguments.get("freshness")
            search_filter = _freshness_filter(freshness)
            if config.engine == "baidu":
                payload = _baidu_payload(query=query.strip(), count=count, search_filter=search_filter)
                response = _post_baidu_search(config=config, payload=payload)
                text = _format_baidu_response(response)
            else:
                text = _search_aliyun_mcp(
                    config=config,
                    query=query.strip(),
                    count=count,
                    freshness=freshness if isinstance(freshness, str) and freshness else None,
                )
        except WebSearchError as error:
            return ToolResult(ok=False, text=str(error), error_type=error.error_type)

        bounded, truncated = bound_tool_text(text, context.config.tools.max_result_chars)
        return ToolResult(ok=True, text=bounded, truncated=truncated)


class WebSearchError(RuntimeError):
    def __init__(self, message: str, error_type: str):
        super().__init__(message)
        self.error_type = error_type


def _result_count(value: Any, default: int) -> int:
    if value is None:
        count = default
    elif isinstance(value, int):
        count = value
    else:
        raise WebSearchError("web.search count must be an integer", "invalid_arguments")
    return min(max(count, 1), 50)


def _freshness_filter(value: Any) -> dict[str, Any]:
    if value is None or value == "":
        return {}
    if not isinstance(value, str):
        raise WebSearchError("web.search freshness must be a string", "invalid_arguments")

    current_time = datetime.now()
    end_date = (current_time + timedelta(days=1)).strftime("%Y-%m-%d")
    offsets = {
        "pd": 1,
        "pw": 6,
        "pm": 30,
        "py": 364,
    }
    if value in offsets:
        start_date = (current_time - timedelta(days=offsets[value])).strftime("%Y-%m-%d")
    elif _FRESHNESS_RANGE_RE.match(value):
        start_date, end_date = value.split("to", maxsplit=1)
    else:
        raise WebSearchError(
            "web.search freshness must be pd, pw, pm, py, or YYYY-MM-DDtoYYYY-MM-DD",
            "invalid_arguments",
        )
    return {"range": {"page_time": {"gte": start_date, "lt": end_date}}}


def _baidu_payload(query: str, count: int, search_filter: dict[str, Any]) -> dict[str, Any]:
    return {
        "messages": [{"content": query, "role": "user"}],
        "search_source": "baidu_search_v2",
        "resource_type_filter": [{"type": "web", "top_k": count}],
        "search_filter": search_filter,
    }


def _post_baidu_search(config: WebSearchConfig, payload: dict[str, Any]) -> dict[str, Any]:
    url, headers = _baidu_url_and_headers(config)
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    request = urllib.request.Request(url=url, data=body, method="POST", headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=config.timeout_seconds) as response:
            return json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as error:
        body_text = error.read().decode("utf-8", errors="replace")[:500]
        raise WebSearchError(f"web.search failed with HTTP {error.code}: {body_text}", "http_error") from error
    except urllib.error.URLError as error:
        raise WebSearchError(f"web.search request failed: {error.reason}", "network_error") from error
    except TimeoutError as error:
        raise WebSearchError("web.search request timed out", "timeout") from error
    except json.JSONDecodeError as error:
        raise WebSearchError("web.search response was not valid JSON", "invalid_response") from error


def _baidu_url_and_headers(config: WebSearchConfig) -> tuple[str, dict[str, str]]:
    session_id = os.environ.get("DUMATE_SESSION_ID")
    scheduler_url = os.environ.get("DUMATE_SCHEDULER_URL")
    headers = {"Content-Type": "application/json"}

    if session_id and scheduler_url:
        parsed = urlparse(config.endpoint)
        url = f"{scheduler_url.rstrip('/')}/api/qianfanproxy{parsed.path}"
        if parsed.query:
            url += f"?{parsed.query}"
        headers.update(
            {
                "Host": parsed.netloc,
                "X-Dumate-Session-Id": session_id,
                "X-Appbuilder-From": "desktop",
            }
        )
        return url, headers

    if not config.api_key:
        raise WebSearchError("Missing Baidu web search API key: set web_search.api_key", "invalid_config")

    headers.update(
        {
            "Authorization": f"Bearer {config.api_key}",
            "X-Appbuilder-From": "openclaw",
        }
    )
    return config.endpoint, headers


def _format_baidu_response(data: dict[str, Any]) -> str:
    if "code" in data:
        message = data.get("message") or data.get("msg") or "Baidu web search API error"
        raise WebSearchError(str(message), "api_error")
    references = data.get("references")
    if not isinstance(references, list):
        raise WebSearchError("Baidu web search response missing references", "invalid_response")

    cleaned = []
    for item in references:
        if isinstance(item, dict):
            copied = dict(item)
            copied.pop("snippet", None)
            cleaned.append(copied)
        else:
            cleaned.append(item)
    return json.dumps(cleaned, indent=2, ensure_ascii=False)


def _search_aliyun_mcp(
    config: WebSearchConfig,
    query: str,
    count: int,
    freshness: str | None,
) -> str:
    endpoint = config.endpoint.strip()
    if not endpoint or endpoint == _BAIDU_DEFAULT_ENDPOINT:
        raise WebSearchError(
            "Missing Aliyun WebSearch MCP endpoint: set web_search.endpoint",
            "invalid_config",
        )
    api_key = config.api_key or os.environ.get("DASHSCOPE_API_KEY", "")
    if not api_key:
        raise WebSearchError(
            "Missing Aliyun WebSearch MCP API key: set web_search.api_key or DASHSCOPE_API_KEY",
            "invalid_config",
        )

    session_id: str | None = None
    try:
        initialize, session_id = _mcp_post(
            config=config,
            endpoint=endpoint,
            api_key=api_key,
            payload={
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": _MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "colibri", "version": "0.1.0"},
                },
            },
        )
        _mcp_result(initialize, 1)
        _mcp_post(
            config=config,
            endpoint=endpoint,
            api_key=api_key,
            payload={"jsonrpc": "2.0", "method": "notifications/initialized"},
            session_id=session_id,
            allow_empty=True,
        )
        listed, _ = _mcp_post(
            config=config,
            endpoint=endpoint,
            api_key=api_key,
            payload={"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
            session_id=session_id,
        )
        tool = _select_mcp_search_tool(_mcp_result(listed, 2))
        tool_arguments: dict[str, Any] = {"query": query}
        properties = tool.get("inputSchema", {}).get("properties", {})
        if isinstance(properties, dict) and "count" in properties:
            tool_arguments["count"] = min(count, 20)
        if freshness and isinstance(properties, dict) and "freshness" in properties:
            tool_arguments["freshness"] = freshness

        called, _ = _mcp_post(
            config=config,
            endpoint=endpoint,
            api_key=api_key,
            payload={
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": tool["name"], "arguments": tool_arguments},
            },
            session_id=session_id,
        )
        return _mcp_tool_text(_mcp_result(called, 3))
    finally:
        if session_id:
            _mcp_delete(config, endpoint, api_key, session_id)


def _mcp_post(
    config: WebSearchConfig,
    endpoint: str,
    api_key: str,
    payload: dict[str, Any],
    session_id: str | None = None,
    allow_empty: bool = False,
) -> tuple[list[dict[str, Any]], str | None]:
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    headers = {
        "Authorization": f"Bearer {api_key}",
        "Accept": "application/json, text/event-stream",
        "MCP-Protocol-Version": _MCP_PROTOCOL_VERSION,
        "Content-Type": "application/json",
    }
    if session_id:
        headers["Mcp-Session-Id"] = session_id
    request = urllib.request.Request(url=endpoint, data=body, method="POST", headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=config.timeout_seconds) as response:
            response_body = response.read().decode("utf-8")
            response_session = response.headers.get("Mcp-Session-Id") or session_id
            content_type = response.headers.get("Content-Type", "")
    except urllib.error.HTTPError as error:
        body_text = error.read().decode("utf-8", errors="replace")[:500]
        raise WebSearchError(f"web.search MCP failed with HTTP {error.code}: {body_text}", "http_error") from error
    except urllib.error.URLError as error:
        if isinstance(error.reason, (TimeoutError, socket.timeout)):
            raise WebSearchError("web.search MCP request timed out", "timeout") from error
        raise WebSearchError(f"web.search MCP request failed: {error.reason}", "network_error") from error
    except (TimeoutError, socket.timeout) as error:
        raise WebSearchError("web.search MCP request timed out", "timeout") from error

    if not response_body.strip():
        if allow_empty:
            return [], response_session
        raise WebSearchError("web.search MCP response was empty", "invalid_response")
    return _mcp_messages(response_body, content_type), response_session


def _mcp_delete(
    config: WebSearchConfig,
    endpoint: str,
    api_key: str,
    session_id: str,
) -> None:
    request = urllib.request.Request(
        url=endpoint,
        method="DELETE",
        headers={
            "Authorization": f"Bearer {api_key}",
            "Accept": "application/json, text/event-stream",
            "MCP-Protocol-Version": _MCP_PROTOCOL_VERSION,
            "Mcp-Session-Id": session_id,
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=config.timeout_seconds):
            pass
    except (urllib.error.URLError, TimeoutError, socket.timeout):
        pass


def _mcp_messages(body: str, content_type: str) -> list[dict[str, Any]]:
    try:
        if "text/event-stream" not in content_type.lower() and not body.lstrip().startswith(("data:", "event:")):
            parsed = json.loads(body)
            values = parsed if isinstance(parsed, list) else [parsed]
            if all(isinstance(value, dict) for value in values):
                return values
            raise ValueError

        messages: list[dict[str, Any]] = []
        for event in re.split(r"\r?\n\r?\n", body):
            data = "\n".join(
                line.partition(":")[2].lstrip()
                for line in event.splitlines()
                if line.startswith("data:")
            )
            if not data:
                continue
            value = json.loads(data)
            if isinstance(value, dict):
                messages.append(value)
        if messages:
            return messages
    except (json.JSONDecodeError, ValueError) as error:
        raise WebSearchError("web.search MCP response was not valid JSON or SSE", "invalid_response") from error
    raise WebSearchError("web.search MCP response contained no JSON-RPC message", "invalid_response")


def _mcp_result(messages: list[dict[str, Any]], request_id: int) -> dict[str, Any]:
    response = next((message for message in messages if message.get("id") == request_id), None)
    if response is None:
        raise WebSearchError("web.search MCP response was missing the requested result", "invalid_response")
    if "error" in response:
        error = response.get("error")
        message = error.get("message") if isinstance(error, dict) else error
        raise WebSearchError(str(message or "MCP request failed"), "api_error")
    result = response.get("result")
    if not isinstance(result, dict):
        raise WebSearchError("web.search MCP response was missing result", "invalid_response")
    return result


def _select_mcp_search_tool(result: dict[str, Any]) -> dict[str, Any]:
    tools = result.get("tools")
    if not isinstance(tools, list):
        raise WebSearchError("web.search MCP tools/list response was invalid", "invalid_response")
    candidates = [tool for tool in tools if isinstance(tool, dict) and isinstance(tool.get("name"), str)]
    selected = next((tool for tool in candidates if tool["name"] == _ALIYUN_MCP_TOOL_NAME), None)
    if selected is None and len(candidates) == 1:
        selected = candidates[0]
    if selected is None:
        raise WebSearchError("web.search MCP did not advertise a unique search tool", "invalid_response")
    return selected


def _mcp_tool_text(result: dict[str, Any]) -> str:
    if result.get("isError") is True:
        raise WebSearchError("Aliyun WebSearch MCP tool returned an error", "api_error")
    content = result.get("content")
    if isinstance(content, list):
        text = "\n".join(
            block["text"]
            for block in content
            if isinstance(block, dict) and block.get("type") == "text" and isinstance(block.get("text"), str)
        ).strip()
        if text:
            return text
    structured = result.get("structuredContent")
    if structured is not None:
        return json.dumps(structured, indent=2, ensure_ascii=False)
    raise WebSearchError("Aliyun WebSearch MCP result contained no usable content", "invalid_response")
