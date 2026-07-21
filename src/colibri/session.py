from __future__ import annotations

import queue
from dataclasses import dataclass, field, replace
from time import monotonic
from typing import Callable

from colibri.config import AgentConfig
from colibri.context import (
    COMPACT_SYSTEM_PROMPT,
    append_summary,
    compact_prompt_message,
    format_model_summary,
    estimate_model_input_tokens,
    retain_recent_message_groups,
    summarize_messages,
    summary_context,
)
from colibri.memory import MemoryContext
from colibri.messages import AgentResponse, Message, ModelLimits, ModelResponse, ToolCall
from colibri.media import MediaPart
from colibri.model.base import ModelClient
from colibri.model.errors import ModelError
from colibri.skills import SkillIndex
from colibri.steering import (
    SKIPPED_TOOL_RESULT,
    STEERING_QUEUE_MAX,
    format_steering_ack,
)
from colibri.textutil import bound_text
from colibri.tools.base import ToolContext, ToolResult
from colibri.tools.permissions import PermissionPolicy
from colibri.tools.registry import ToolRegistry
from colibri.transcript import TranscriptSink
from colibri.vision import ImageAnalyzer


SYSTEM_PROMPT = (
    "Your name is Colibri. You are a lightweight personal agent running on the CardputerZero, a multi-interface device powered by the CM0 chip. "
    "Prefer short, practical responses and respect low memory, battery, and tool limits. "
)
MODEL_UNAVAILABLE_TEXT = "模型暂时不可用，请检查网络后重试。"


@dataclass
class AgentSession:
    config: AgentConfig
    model: ModelClient
    tools: ToolRegistry | None = None
    permission_policy: PermissionPolicy | None = None
    transcript: TranscriptSink | None = None
    media_sender: Callable[[MediaPart], None] | None = None
    history_loader: Callable[[], list[Message]] | None = None
    steer_notifier: Callable[[str], None] | None = None
    messages: list[Message] = field(default_factory=list)
    summary: str = ""
    started_at: float = field(default_factory=monotonic)
    last_activity_at: float = field(default_factory=monotonic)
    _history_loaded: bool = field(default=False, init=False, repr=False)
    _image_analyzer: ImageAnalyzer | None = field(default=None, init=False, repr=False)
    _steering: queue.Queue[str] = field(
        default_factory=lambda: queue.Queue(maxsize=STEERING_QUEUE_MAX),
        init=False,
        repr=False,
    )
    _turn_active: bool = field(default=False, init=False, repr=False)
    _permission_pending: bool = field(default=False, init=False, repr=False)

    def steer(self, text: str) -> bool:
        cleaned = text.strip()
        if not cleaned or not self._turn_active or self._permission_pending:
            return False
        try:
            self._steering.put_nowait(cleaned)
            return True
        except queue.Full:
            return False

    def is_turn_active(self) -> bool:
        return self._turn_active

    def is_permission_pending(self) -> bool:
        return self._permission_pending

    def submit(self, user_text: str, media: list[MediaPart] | None = None) -> AgentResponse:
        self._restore_history_once()
        bounded_text = self._prepare_user_message(user_text, media or [])
        registry, policy, image_analyzer = self._runtime_dependencies()
        context = self._tool_context(registry, image_analyzer)
        memory_text, skill_text = self._load_dynamic_context(bounded_text)
        model_messages = self._model_messages_for_completion(memory_text, skill_text)

        self._turn_active = True
        try:
            for _round_index in range(self.config.session.max_tool_rounds):
                model_response = self._complete_model(model_messages, registry)
                assistant_text = self._record_assistant_message(model_response)

                if not model_response.tool_calls:
                    steered = self._drain_one_steering()
                    if steered is not None:
                        self._apply_steering(steered, skipped=0)
                        model_messages = self._model_messages_for_completion(memory_text, skill_text)
                        continue
                    return self._finish_response(assistant_text)

                calls = list(model_response.tool_calls)
                for index, call in enumerate(calls):
                    self._execute_tool_call(call, registry, policy, context)
                    steered = self._drain_one_steering()
                    if steered is not None:
                        skipped = len(calls) - index - 1
                        for skipped_call in calls[index + 1 :]:
                            self._record_skipped_tool(skipped_call)
                        self._apply_steering(steered, skipped=skipped)
                        break
                model_messages = self._model_messages_for_completion(memory_text, skill_text)

            return self._round_limit_response()
        except ModelError as error:
            return self._finish_model_error(error)
        finally:
            self._turn_active = False
            self._clear_steering_queue()

    def reset(self) -> None:
        self.messages.clear()
        self.summary = ""
        self.last_activity_at = monotonic()

    def close(self) -> None:
        if self.transcript is not None:
            self.transcript.close()

    def _prepare_user_message(self, user_text: str, media: list[MediaPart]) -> str:
        text_with_media = _user_text_with_media(user_text, media)
        self.messages.append(Message(role="user", content=text_with_media))
        self._write_transcript(
            "user_message",
            {"text": text_with_media, "media": [_media_payload(part) for part in media]},
        )
        return text_with_media

    def _tool_context(self, registry: ToolRegistry, image_analyzer: ImageAnalyzer) -> ToolContext:
        return ToolContext(
            config=self.config,
            cwd=registry.cwd,
            media_sender=self.media_sender,
            image_analyzer=image_analyzer,
        )

    def _load_dynamic_context(self, user_text: str) -> tuple[str, str]:
        memory_result = MemoryContext(self.config).load()
        if memory_result.text:
            self._write_transcript(
                "memory_context",
                {"files": memory_result.files, "truncated": memory_result.truncated},
            )
        skill_result = SkillIndex.scan(self.config.skills.dir).catalog(self.config.skills)
        if skill_result.text:
            self._write_transcript(
                "skill_catalog",
                {"skills": skill_result.skills, "truncated": skill_result.truncated},
            )
        return memory_result.text, skill_result.text

    def _complete_model(self, messages: list[Message], registry: ToolRegistry) -> ModelResponse:
        try:
            return self.model.complete(
                messages=list(messages),
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

    def _record_assistant_message(self, response: ModelResponse) -> str:
        assistant_text = self._bound_text(response.text, self.config.tools.max_result_chars)
        self.messages.append(
            Message(role="assistant", content=assistant_text, tool_calls=list(response.tool_calls))
        )
        self._write_transcript(
            "assistant_message",
            {"text": assistant_text, "tool_call_count": len(response.tool_calls)},
        )
        return assistant_text

    def _execute_tool_call(
        self,
        call: ToolCall,
        registry: ToolRegistry,
        policy: PermissionPolicy,
        context: ToolContext,
    ) -> None:
        self._write_transcript(
            "tool_call",
            {"id": call.id, "name": call.name, "arguments": call.arguments},
        )
        tool = registry.get(call.name)
        if tool is None:
            result = registry.run(call, context)
        else:
            self._permission_pending = True
            try:
                decision = policy.decide(tool, call.arguments, context)
            finally:
                self._permission_pending = False
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
                    "hardware_device": decision.hardware_device,
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
        self.messages.append(Message(role="tool", content=self._tool_result_text(result), tool_call_id=call.id))

    def _finish_response(self, text: str, error_type: str | None = None) -> AgentResponse:
        self.last_activity_at = monotonic()
        return AgentResponse(text=text, messages=list(self.messages), error_type=error_type)

    def _finish_model_error(self, error: ModelError) -> AgentResponse:
        self.messages.append(Message(role="assistant", content=MODEL_UNAVAILABLE_TEXT))
        self._write_transcript(
            "assistant_message",
            {"text": MODEL_UNAVAILABLE_TEXT, "tool_call_count": 0, "model_error": error.category},
        )
        return self._finish_response(MODEL_UNAVAILABLE_TEXT, error_type=error.category)

    def _round_limit_response(self) -> AgentResponse:
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
        return self._finish_response(limit_text)

    def _restore_history_once(self) -> None:
        if self._history_loaded:
            return
        self._history_loaded = True
        if self.history_loader is None:
            return
        try:
            self.messages.extend(self.history_loader())
        except Exception as error:
            self._write_transcript(
                "history_restore_error",
                {"error_type": type(error).__name__, "message": str(error)},
            )
            return

    def _runtime_dependencies(self) -> tuple[ToolRegistry, PermissionPolicy, ImageAnalyzer]:
        if self.tools is None:
            self.tools = ToolRegistry.from_config(self.config)
        if self.permission_policy is None:
            self.permission_policy = PermissionPolicy.from_config(self.config, cwd=self.tools.cwd)
        if self._image_analyzer is None:
            self._image_analyzer = ImageAnalyzer(self.config, self.model)
        return self.tools, self.permission_policy, self._image_analyzer

    def _compact_messages_if_needed(self, memory_text: str = "", skill_text: str = "") -> None:
        trigger_limit = max(1, self.config.session.trigger_message_limit)
        token_limit = max(0, self.config.model.input_context_tokens)
        token_threshold = token_limit * 8 // 10 if token_limit else 0
        should_compact = len(self.messages) >= trigger_limit
        if not should_compact and token_threshold:
            should_compact = estimate_model_input_tokens(self._model_messages(memory_text, skill_text)) >= token_threshold
        if not should_compact or not self.messages:
            return

        messages_to_compact = list(self.messages)
        addition, mode = self._compact_messages(messages_to_compact)
        self.summary = append_summary(self.summary, addition, self.config.session.summary_max_chars)
        self.messages = retain_recent_message_groups(
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
        return bound_text(text, max_chars)

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

    def _drain_one_steering(self) -> str | None:
        try:
            return self._steering.get_nowait()
        except queue.Empty:
            return None

    def _clear_steering_queue(self) -> None:
        while True:
            try:
                self._steering.get_nowait()
            except queue.Empty:
                return

    def _record_skipped_tool(self, call: ToolCall) -> None:
        self._write_transcript(
            "tool_result",
            {
                "id": call.id,
                "name": call.name,
                "ok": False,
                "error_type": "steered_skip",
                "text": SKIPPED_TOOL_RESULT,
                "truncated": False,
                "media": None,
            },
        )
        self.messages.append(
            Message(
                role="tool",
                content=self._tool_result_text(
                    ToolResult(ok=False, text=SKIPPED_TOOL_RESULT, error_type="steered_skip")
                ),
                tool_call_id=call.id,
            )
        )

    def _apply_steering(self, text: str, *, skipped: int) -> None:
        self._write_transcript(
            "steered",
            {"skipped": skipped, "chars": len(text), "text": text[:200]},
        )
        if self.steer_notifier is not None:
            self.steer_notifier(format_steering_ack(skipped, text))
        self.messages.append(Message(role="user", content=text))
        self._write_transcript(
            "user_message",
            {"text": text, "media": [], "steering": True},
        )

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

    def _model_messages_for_completion(self, memory_text: str, skill_text: str = "") -> list[Message]:
        self._compact_messages_if_needed(memory_text, skill_text)
        return self._model_messages(memory_text, skill_text)

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


def _user_text_with_media(user_text: str, media: list[MediaPart]) -> str:
    if not media:
        return user_text
    lines = ["Attachments saved locally:"]
    for index, part in enumerate(media, start=1):
        label = part.type or "file"
        filename = part.filename or part.path.name
        content_type = f", content_type={part.content_type}" if part.content_type else ""
        lines.append(f"{index}. {label}: {filename} at {part.path}{content_type}")
    text = user_text.strip()
    attachment_text = "\n".join(lines)
    if not text:
        return attachment_text
    return f"{text}\n\n{attachment_text}"


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
    lines.append(
        'If the user says "continue", continue from this stopped state with targeted reads and do not claim '
        "the previous task was fully completed."
    )
    return bound_text("\n".join(lines), max_chars)


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
