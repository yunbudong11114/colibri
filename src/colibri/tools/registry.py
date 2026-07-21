from __future__ import annotations

from pathlib import Path

from colibri.config import AgentConfig
from colibri.messages import ToolCall
from colibri.tools.base import Tool, ToolContext, ToolResult
from colibri.tools.builtin import (
    FilesListTool,
    FilesReadTool,
    FilesSendTool,
    FilesWriteTool,
    HARDWARE_TOOLS,
    ImageUnderstandTool,
    MemoryListTool,
    MemoryReadTool,
    MemorySearchTool,
    MemoryWriteTool,
    ShellRunTool,
    SkillReadTool,
    SkillRunTool,
    WebSearchTool,
)


class ToolRegistry:
    def __init__(self, tools: list[Tool], cwd: Path | None = None):
        self._tools = {tool.spec.name: tool for tool in tools}
        self._specs = [tool.spec.as_openai_tool() for tool in tools]
        self.cwd = cwd or Path.cwd()

    @classmethod
    def from_config(cls, config: AgentConfig, cwd: Path | None = None) -> "ToolRegistry":
        tools: list[Tool] = []
        enabled = set(config.tools.enabled)
        if "files" in enabled:
            tools.extend([FilesListTool(), FilesReadTool(), FilesWriteTool(), FilesSendTool()])
        if "memory" in enabled:
            tools.extend([MemoryListTool(), MemoryReadTool(), MemorySearchTool(), MemoryWriteTool()])
        if "shell" in enabled:
            tools.append(ShellRunTool())
        if "web" in enabled:
            tools.append(WebSearchTool())
        if "hardware" in enabled and config.hardware.enabled:
            tools.extend(tool() for tool in HARDWARE_TOOLS)
        if "image" in enabled:
            tools.append(ImageUnderstandTool())
        if "skills" in enabled:
            tools.extend([SkillReadTool(), SkillRunTool()])
        return cls(tools=tools, cwd=cwd)

    def specs(self) -> list[dict]:
        return list(self._specs)

    def get(self, name: str) -> Tool | None:
        return self._tools.get(name)

    def run(self, call: ToolCall, context: ToolContext) -> ToolResult:
        tool = self._tools.get(call.name)
        if tool is None:
            return ToolResult(ok=False, text=f"Unknown tool: {call.name}", error_type="unknown_tool")
        return tool.run(call.arguments, context)
