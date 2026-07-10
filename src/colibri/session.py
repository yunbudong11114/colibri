from __future__ import annotations

from dataclasses import dataclass, field, replace
from time import monotonic
from typing import Callable

from colibri.config import AgentConfig
from colibri.context import (
    COMPACT_SYSTEM_PROMPT,
    append_summary,
    budget_model_messages,
    compact_prompt_message,
    format_model_summary,
    model_input_chars,
    summarize_messages,
    summary_context,
)
from colibri.memory import MemoryContext
from colibri.messages import AgentResponse, Message, ModelLimits, ToolCall
from colibri.media import MediaPart
from colibri.model.base import ModelClient
from colibri.skills import SkillIndex
from colibri.tools.base import ToolContext, ToolResult
from colibri.tools.permissions import PermissionPolicy
from colibri.tools.registry import ToolRegistry
from colibri.transcript import TranscriptSink


SYSTEM_PROMPT = (
    "Your name is Colibri. You are a lightweight personal agent running on the CardputerZero, a multi-interface device powered by the CM0 chip. "
    "Prefer short, practical responses and respect low memory, battery, and tool limits. "
)


@dataclass
class AgentSession:
    config: AgentConfig
    model: ModelClient
    tools: ToolRegistry | None = None
    permission_policy: PermissionPolicy | None = None
    transcript: TranscriptSink | None = None
    media_sender: Callable[[MediaPart], None] | None = None
    messages: list[Message] = field(default_factory=list)
    summary: str = ""
    started_at: float = field(default_factory=monotonic)
    last_activity_at: float = field(default_factory=monotonic)

    def submit(self, user_text: str) -> AgentResponse:
        bounded_text = self._bound_text(user_text, self.config.session.model_input_char_limit)
        self.messages.append(Message(role="user", content=bounded_text))
        self._write_transcript("user_message", {"text": bounded_text})
        self._compact_messages_if_needed()

        registry = self.tools or ToolRegistry.from_config(self.config)
        if self.permission_policy is None:
            self.permission_policy = PermissionPolicy.from_config(self.config, cwd=registry.cwd)
        policy = self.permission_policy
        context = ToolContext(config=self.config, cwd=registry.cwd, media_sender=self.media_sender)
        memory_result = MemoryContext(self.config).load()
        if memory_result.text:
            self._write_transcript(
                "memory_context",
                {"files": memory_result.files, "truncated": memory_result.truncated},
            )
        skill_result = SkillIndex.scan(self.config.skills.dirs).context_for(bounded_text, self.config.skills)
        if skill_result.text:
            self._write_transcript(
                "skill_recall",
                {"skills": skill_result.skills, "truncated": skill_result.truncated},
            )
        model_messages = self._budgeted_model_messages(memory_result.text, skill_result.text)

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
            self._compact_messages_if_needed()

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
                    decision = policy.decide(tool, call.arguments, context)
                    self._write_transcript(
                        "permission_decision",
                        {
                            "tool_name": call.name,
                            "subject_kind": decision.subject_kind,
                            "decision": decision.decision,
                            "scope": decision.scope,
                            "allowed": decision.allowed,
                            "reason": decision.reason,
                            "shell_command": call.arguments.get("command") if call.name == "shell.run" else None,
                            "file_path": decision.file_path,
                            "file_root": decision.file_root,
                        },
                    )
                    if decision.allowed:
                        run_context = context
                        if decision.file_root is not None:
                            run_context = replace(
                                context,
                                allowed_file_roots=frozenset({decision.file_root}),
                            )
                        result = tool.run(call.arguments, run_context)
                    else:
                        result = ToolResult(
                            ok=False,
                            text=_denied_tool_text(call),
                            error_type="permission_denied",
                        )
                result = self._send_media_result_if_needed(result)
                self._write_transcript(
                    "tool_result",
                    {
                        "id": call.id,
                        "name": call.name,
                        "ok": result.ok,
                        "error_type": result.error_type,
                        "text": self._bound_text(result.text, self.config.tools.max_result_chars),
                        "truncated": result.truncated,
                        "media": _media_payload(result.media),
                    },
                )
                self.messages.append(
                    Message(role="tool", content=self._tool_result_text(result), tool_call_id=call.id)
                )
                self._compact_messages_if_needed()
                model_messages = self._budgeted_model_messages(memory_result.text, skill_result.text)

        limit_text = _round_limit_text(
            self.messages,
            max_tool_rounds=self.config.session.max_tool_rounds,
            max_chars=self.config.tools.max_result_chars,
        )
        self.messages.append(Message(role="assistant", content=limit_text))
        self._write_transcript(
            "round_limit",
            {"max_tool_rounds": self.config.session.max_tool_rounds, "text": limit_text},
        )
        self._compact_messages_if_needed()
        self.last_activity_at = monotonic()
        return AgentResponse(text=limit_text, messages=list(self.messages))

    def reset(self) -> None:
        self.messages.clear()
        self.summary = ""
        self.last_activity_at = monotonic()

    def close(self) -> None:
        if self.transcript is not None:
            self.transcript.close()

    def _compact_messages_if_needed(self) -> None:
        trigger_limit = max(1, self.config.session.trigger_message_limit)
        if len(self.messages) >= trigger_limit:
            messages_to_compact = list(self.messages)
            addition, mode = self._compact_messages(messages_to_compact)
            self.summary = append_summary(self.summary, addition, self.config.session.summary_max_chars)
            self.messages = _retained_messages_after_compact(
                messages_to_compact,
                max(0, self.config.session.recent_message_limit),
            )
            self._write_transcript(
                "context_compact",
                {
                    "removed_messages": len(messages_to_compact) - len(self.messages),
                    "compacted_messages": len(messages_to_compact),
                    "kept_messages": len(self.messages),
                    "mode": mode,
                    "summary_chars": len(self.summary),
                },
            )

    def _compact_messages(self, messages: list[Message]) -> tuple[str, str]:
        if self._should_model_compact():
            try:
                compact_response = self.model.complete(
                    messages=[compact_prompt_message(self.summary, messages)],
                    tools=[],
                    system=COMPACT_SYSTEM_PROMPT,
                    limits=ModelLimits(
                        timeout_seconds=self.config.model.timeout_seconds,
                        max_output_tokens=self.config.model.max_output_tokens,
                    ),
                )
                if compact_response.tool_calls:
                    raise RuntimeError("compact response included tool calls")
                addition = format_model_summary(compact_response.text)
                if not addition:
                    raise RuntimeError("compact response was empty")
                return addition, "model"
            except Exception as error:
                self._write_transcript(
                    "context_compact_error",
                    {"error_type": type(error).__name__, "message": str(error), "fallback": True},
                )
        return summarize_messages(messages), "fallback"

    def _should_model_compact(self) -> bool:
        return self.config.session.model_compact and self.config.model.provider != "fake"

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

    def _send_media_result_if_needed(self, result: ToolResult) -> ToolResult:
        if not result.ok or result.media is None:
            return result
        if self.media_sender is None:
            return ToolResult(
                ok=False,
                text="No active channel can send files in this session",
                error_type="media_unavailable",
            )
        try:
            self.media_sender(result.media)
        except Exception as error:
            return ToolResult(ok=False, text=str(error), error_type="media_send_error")
        return result

    def _write_transcript(self, event_type: str, payload: dict) -> None:
        if self.transcript is not None:
            self.transcript.write(event_type, payload)

    def _model_messages(self, memory_text: str, skill_text: str = "") -> list[Message]:
        messages = list(self.messages)
        context_messages: list[Message] = []
        summary_text = summary_context(self.summary)
        if summary_text:
            context_messages.append(Message(role="system", content=summary_text))
        if memory_text:
            context_messages.append(Message(role="system", content=memory_text))
        if skill_text:
            context_messages.append(Message(role="system", content=skill_text))
        return context_messages + messages

    def _budgeted_model_messages(self, memory_text: str, skill_text: str = "") -> list[Message]:
        messages = self._model_messages(memory_text, skill_text)
        budgeted, dropped = budget_model_messages(messages, self.config.session.model_input_char_limit)
        if dropped:
            self._write_transcript(
                "context_budget",
                {
                    "dropped_model_messages": dropped,
                    "input_chars": model_input_chars(budgeted),
                },
            )
        return budgeted

def _denied_tool_text(call: ToolCall) -> str:
    if call.name == "shell.run":
        command = call.arguments.get("command")
        if isinstance(command, str) and command.strip():
            return f"User denied shell.run: {command.strip()}"
    return f"User denied {call.name}"


def _media_payload(media: MediaPart | None) -> dict | None:
    if media is None:
        return None
    return {
        "type": media.type,
        "path": str(media.path),
        "filename": media.filename,
        "content_type": media.content_type,
        "caption": media.caption,
    }


def _retained_messages_after_compact(messages: list[Message], recent_limit: int) -> list[Message]:
    if not messages:
        return []
    kept_start = max(0, len(messages) - recent_limit)
    kept = list(messages[kept_start:]) if recent_limit > 0 else []
    latest_user_index = _latest_user_index(messages)
    if latest_user_index is not None and latest_user_index < kept_start:
        return [messages[latest_user_index], *kept]
    return kept


def _latest_user_index(messages: list[Message]) -> int | None:
    for index in range(len(messages) - 1, -1, -1):
        if messages[index].role == "user":
            return index
    return None


def _round_limit_text(messages: list[Message], max_tool_rounds: int, max_chars: int) -> str:
    round_word = "round" if max_tool_rounds == 1 else "rounds"
    lines = [
        f"Tool round limit reached after {max_tool_rounds} {round_word}.",
        "The task may still be incomplete.",
    ]
    recent = _recent_tool_summaries(messages)
    if recent:
        lines.append("Recent tool results:")
        lines.extend(f"- {item}" for item in recent)
    lines.append("You can continue the task, or increase session.max_tool_rounds if this is expected.")
    return _bound_text_block("\n".join(lines), max_chars)


def _recent_tool_summaries(messages: list[Message], limit: int = 4) -> list[str]:
    tool_names_by_id: dict[str, str] = {}
    for message in messages:
        for call in message.tool_calls:
            tool_names_by_id[call.id] = call.name

    summaries: list[str] = []
    for message in reversed(messages):
        if message.role != "tool":
            continue
        tool_name = tool_names_by_id.get(message.tool_call_id or "", "unknown")
        text = " ".join(message.content.split())
        if len(text) > 120:
            text = text[:116] + " ..."
        summaries.append(f"{tool_name}: {text}")
        if len(summaries) >= limit:
            break
    return list(reversed(summaries))


def _bound_text_block(text: str, max_chars: int) -> str:
    if len(text) <= max_chars:
        return text
    suffix = "\n...[truncated]"
    keep = max(0, max_chars - len(suffix))
    return text[:keep] + suffix
