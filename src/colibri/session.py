from __future__ import annotations

from dataclasses import dataclass, field
from time import monotonic

from colibri.config import AgentConfig
from colibri.messages import AgentResponse, Message, ModelLimits
from colibri.model.base import ModelClient


SYSTEM_PROMPT = (
    "You are a lightweight personal agent running on a CardputerZero-class Linux device. "
    "Prefer short, practical responses and respect low memory, battery, and tool limits."
)


@dataclass
class AgentSession:
    config: AgentConfig
    model: ModelClient
    messages: list[Message] = field(default_factory=list)
    summary: str = ""
    started_at: float = field(default_factory=monotonic)
    last_activity_at: float = field(default_factory=monotonic)

    def submit(self, user_text: str) -> AgentResponse:
        bounded_text = self._bound_text(user_text, self.config.session.compact_trigger_chars)
        self.messages.append(Message(role="user", content=bounded_text))
        self._trim_recent_messages()

        model_response = self.model.complete(
            messages=list(self.messages),
            tools=[],
            system=SYSTEM_PROMPT,
            limits=ModelLimits(
                timeout_seconds=self.config.model.timeout_seconds,
                max_output_tokens=self.config.model.max_output_tokens,
            ),
        )
        assistant_text = self._bound_text(model_response.text, self.config.tools.max_result_chars)
        self.messages.append(Message(role="assistant", content=assistant_text))
        self._trim_recent_messages()
        self.last_activity_at = monotonic()

        return AgentResponse(text=assistant_text, messages=list(self.messages))

    def reset(self) -> None:
        self.messages.clear()
        self.summary = ""
        self.last_activity_at = monotonic()

    def close(self) -> None:
        return None

    def _trim_recent_messages(self) -> None:
        limit = self.config.session.recent_message_limit
        if len(self.messages) > limit:
            self.messages = self.messages[-limit:]

    @staticmethod
    def _bound_text(text: str, max_chars: int) -> str:
        if len(text) <= max_chars:
            return text
        keep = max(0, max_chars - len("\n...[truncated]"))
        return text[:keep] + "\n...[truncated]"
