from colibri.context import (
    append_summary,
    estimate_model_input_tokens,
    format_model_summary,
    retain_recent_message_groups,
    summarize_messages,
)
from colibri.messages import Message, ToolCall


def test_summarize_user_and_assistant_messages():
    text = summarize_messages(
        [
            Message(role="user", content="Where is the router?"),
            Message(role="assistant", content="The router is upstairs."),
        ]
    )

    assert text.splitlines() == [
        "user: Where is the router?",
        "assistant: The router is upstairs.",
    ]


def test_summarize_tool_message_uses_metadata_not_full_output():
    text = summarize_messages(
        [
            Message(
                role="assistant",
                content="",
                tool_calls=[ToolCall(id="call_1", name="files.read", arguments={"path": "secret.txt"})],
            ),
            Message(role="tool", content="x" * 200, tool_call_id="call_1"),
        ]
    )

    assert text == "assistant tool_calls: files.read\ntool files.read ok: 200 chars"
    assert "x" * 20 not in text


def test_append_summary_keeps_tail_within_limit():
    summary = append_summary("old line\n", "new line one\nnew line two", max_chars=18)

    assert summary == "new line two"
    assert len(summary) <= 18


def test_format_model_summary_strips_analysis_and_keeps_summary_body():
    summary = format_model_summary(
        "<analysis>\nprivate scratchpad\n</analysis>\n"
        "<summary>\n1. Primary Request and Intent:\nKeep the project moving.\n</summary>"
    )

    assert summary == "Summary:\n1. Primary Request and Intent:\nKeep the project moving."
    assert "private scratchpad" not in summary


def test_token_estimate_includes_system_tools_and_tool_call_arguments():
    plain = estimate_model_input_tokens([Message(role="user", content="hello")])
    complete = estimate_model_input_tokens(
        [
            Message(
                role="assistant",
                content="",
                tool_calls=[ToolCall(id="call_1", name="files.read", arguments={"path": "x" * 80})],
            )
        ],
        system="system prompt",
        tools=[{"type": "function", "function": {"name": "files.read", "description": "x" * 80}}],
    )

    assert complete > plain


def test_recent_groups_respect_token_limit_and_keep_latest_user_group():
    messages = [
        Message(role="user", content="old " + "x" * 80),
        Message(role="assistant", content="old answer " + "y" * 80),
        Message(role="user", content="latest request"),
        Message(role="assistant", content="latest answer"),
    ]

    kept = retain_recent_message_groups(messages, recent_limit=4, recent_token_limit=8)

    assert [message.content for message in kept] == ["latest request", "latest answer"]
