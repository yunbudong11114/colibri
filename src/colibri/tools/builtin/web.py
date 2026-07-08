from __future__ import annotations

from datetime import datetime, timedelta
import json
import os
import re
from typing import Any
import urllib.error
import urllib.request
from urllib.parse import urlparse

from colibri.config import WebSearchConfig
from colibri.tools.base import ToolContext, ToolResult, ToolSpec, bound_tool_text


_FRESHNESS_RANGE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}to\d{4}-\d{2}-\d{2}$")


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
        if config.engine != "baidu":
            return ToolResult(ok=False, text=f"Unsupported web search engine: {config.engine}", error_type="invalid_config")

        query = arguments.get("query")
        if not isinstance(query, str) or not query.strip():
            return ToolResult(ok=False, text="web.search requires a non-empty string query", error_type="invalid_arguments")

        try:
            count = _result_count(arguments.get("count"), config.max_results)
            search_filter = _freshness_filter(arguments.get("freshness"))
            payload = _baidu_payload(query=query.strip(), count=count, search_filter=search_filter)
            response = _post_baidu_search(config=config, payload=payload)
            text = _format_baidu_response(response)
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
