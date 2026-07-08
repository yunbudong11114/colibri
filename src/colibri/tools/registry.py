from __future__ import annotations

from pathlib import Path

from colibri.config import AgentConfig
from colibri.messages import ToolCall
from colibri.tools.base import Tool, ToolContext, ToolResult
from colibri.tools.builtin import (
    FilesListTool,
    FilesReadTool,
    MemoryListTool,
    MemoryReadTool,
    MemorySearchTool,
    MemoryWriteTool,
    ShellRunTool,
    SkillRunTool,
    WebSearchTool,
)


class ToolRegistry:
    def __init__(self, tools: list[Tool], cwd: Path | None = None):
        self._tools = {tool.spec.name: tool for tool in tools}
        self.cwd = cwd or Path.cwd()

    @classmethod
    def from_config(cls, config: AgentConfig, cwd: Path | None = None) -> "ToolRegistry":
        tools: list[Tool] = []
        enabled = set(config.tools.enabled)
        if "files" in enabled:
            tools.extend([FilesListTool(), FilesReadTool()])
        if "memory" in enabled:
            tools.extend([MemoryListTool(), MemoryReadTool(), MemorySearchTool(), MemoryWriteTool()])
        if "shell" in enabled:
            tools.append(ShellRunTool())
        if "web" in enabled:
            tools.append(WebSearchTool())
        if "skills" in enabled:
            tools.append(SkillRunTool())
        return cls(tools=tools, cwd=cwd)

    def specs(self) -> list[dict]:
        return [tool.spec.as_openai_tool() for tool in self._tools.values()]

    def get(self, name: str) -> Tool | None:
        return self._tools.get(name)

    def run(self, call: ToolCall, context: ToolContext) -> ToolResult:
        tool = self._tools.get(call.name)
        if tool is None:
            return ToolResult(ok=False, text=f"Unknown tool: {call.name}", error_type="unknown_tool")
        return tool.run(call.arguments, context)
