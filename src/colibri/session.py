from __future__ import annotations

from dataclasses import dataclass, field
from time import monotonic

from colibri.config import AgentConfig
from colibri.context import (
    append_summary,
    budget_model_messages,
    model_input_chars,
    summarize_messages,
    summary_context,
)
from colibri.memory import MemoryRecall
from colibri.messages import AgentResponse, Message, ModelLimits
from colibri.model.base import ModelClient
from colibri.tools.base import ToolContext, ToolResult
from colibri.tools.permissions import PermissionPolicy
from colibri.tools.registry import ToolRegistry
from colibri.transcript import TranscriptWriter


SYSTEM_PROMPT = (
    "You are a lightweight personal agent running on a CardputerZero-class Linux device. "
    "Prefer short, practical responses and respect low memory, battery, and tool limits."
)


@dataclass
class AgentSession:
    config: AgentConfig
    model: ModelClient
    tools: ToolRegistry | None = None
    permission_policy: PermissionPolicy | None = None
    transcript: TranscriptWriter | None = None
    messages: list[Message] = field(default_factory=list)
    summary: str = ""
    started_at: float = field(default_factory=monotonic)
    last_activity_at: float = field(default_factory=monotonic)

    def submit(self, user_text: str) -> AgentResponse:
        bounded_text = self._bound_text(user_text, self.config.session.compact_trigger_chars)
        self.messages.append(Message(role="user", content=bounded_text))
        self._write_transcript("user_message", {"text": bounded_text})
        self._trim_recent_messages()

        registry = self.tools or ToolRegistry.from_config(self.config)
        if self.permission_policy is None:
            self.permission_policy = PermissionPolicy.from_config(self.config)
        policy = self.permission_policy
        context = ToolContext(config=self.config, cwd=registry.cwd)
        memory_result = MemoryRecall(self.config).recall(bounded_text, list(self.messages))
        if memory_result.text:
            self._write_transcript(
                "memory_recall",
                {"topics": memory_result.topics, "truncated": memory_result.truncated},
            )
        model_messages = self._budgeted_model_messages(memory_result.text)

        for _round_index in range(self.config.session.max_tool_rounds):
            try:
                model_response = self.model.complete(
                    messages=list(model_messages),
                    tools=registry.specs(),
                    system=SYSTEM_PROMPT,
                    limits=ModelLimits(
                        timeout_seconds=self.config.model.timeout_seconds,
                        max_output_tokens=self.config.model.max_output_tokens,
                    ),
                )
            except Exception as error:
                self._write_transcript(
                    "model_error",
                    {"error_type": type(error).__name__, "message": str(error)},
                )
                raise
            assistant_text = self._bound_text(model_response.text, self.config.tools.max_result_chars)
            self.messages.append(
                Message(role="assistant", content=assistant_text, tool_calls=list(model_response.tool_calls))
            )
            self._write_transcript(
                "assistant_message",
                {"text": assistant_text, "tool_call_count": len(model_response.tool_calls)},
            )
            self._trim_recent_messages()

            if not model_response.tool_calls:
                self.last_activity_at = monotonic()
                return AgentResponse(text=assistant_text, messages=list(self.messages))

            for call in model_response.tool_calls:
                self._write_transcript(
                    "tool_call",
                    {"id": call.id, "name": call.name, "arguments": call.arguments},
                )
                tool = registry.get(call.name)
                if tool is None:
                    result = registry.run(call, context)
                else:
                    allowed, decision = policy.decide(tool, call.arguments)
                    self._write_transcript(
                        "permission_decision",
                        {"tool_name": call.name, "decision": decision, "allowed": allowed},
                    )
                    if allowed:
                        result = tool.run(call.arguments, context)
                    else:
                        result = ToolResult(
                            ok=False,
                            text="Tool call denied",
                            error_type="permission_denied",
                        )
                self._write_transcript(
                    "tool_result",
                    {
                        "id": call.id,
                        "name": call.name,
                        "ok": result.ok,
                        "error_type": result.error_type,
                        "text": self._bound_text(result.text, self.config.tools.max_result_chars),
                        "truncated": result.truncated,
                    },
                )
                self.messages.append(
                    Message(role="tool", content=self._tool_result_text(result), tool_call_id=call.id)
                )
                self._trim_recent_messages()
                model_messages = self._budgeted_model_messages(memory_result.text)

        limit_text = "Tool round limit reached"
        self.messages.append(Message(role="assistant", content=limit_text))
        self._write_transcript(
            "round_limit",
            {"max_tool_rounds": self.config.session.max_tool_rounds, "text": limit_text},
        )
        self._trim_recent_messages()
        self.last_activity_at = monotonic()
        return AgentResponse(text=limit_text, messages=list(self.messages))

    def reset(self) -> None:
        self.messages.clear()
        self.summary = ""
        self.last_activity_at = monotonic()

    def close(self) -> None:
        if self.transcript is not None:
            self.transcript.close()

    def _trim_recent_messages(self) -> None:
        limit = self.config.session.recent_message_limit
        if len(self.messages) > limit:
            dropped = self.messages[:-limit]
            addition = summarize_messages(dropped)
            self.summary = append_summary(self.summary, addition, self.config.session.summary_max_chars)
            self.messages = self.messages[-limit:]
            self._write_transcript(
                "context_compact",
                {"dropped_messages": len(dropped), "summary_chars": len(self.summary)},
            )

    @staticmethod
    def _bound_text(text: str, max_chars: int) -> str:
        if len(text) <= max_chars:
            return text
        keep = max(0, max_chars - len("\n...[truncated]"))
        return text[:keep] + "\n...[truncated]"

    @staticmethod
    def _tool_result_text(result: ToolResult) -> str:
        if result.ok:
            return result.text
        return f"{result.error_type or 'tool_error'}: {result.text}"

    def _write_transcript(self, event_type: str, payload: dict) -> None:
        if self.transcript is not None:
            self.transcript.write(event_type, payload)

    def _model_messages(self, memory_text: str) -> list[Message]:
        messages = list(self.messages)
        context_messages: list[Message] = []
        summary_text = summary_context(self.summary)
        if summary_text:
            context_messages.append(Message(role="system", content=summary_text))
        if memory_text:
            context_messages.append(Message(role="system", content=memory_text))
        return context_messages + messages

    def _budgeted_model_messages(self, memory_text: str) -> list[Message]:
        messages = self._model_messages(memory_text)
        budgeted, dropped = budget_model_messages(messages, self.config.session.compact_trigger_chars)
        if dropped:
            self._write_transcript(
                "context_budget",
                {
                    "dropped_model_messages": dropped,
                    "input_chars": model_input_chars(budgeted),
                },
            )
        return budgeted
