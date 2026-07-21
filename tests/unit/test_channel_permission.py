from colibri.channels.permission import ChannelTextPermissionPrompter, format_channel_permission_prompt
from colibri.tools.permissions import PermissionRequest, PermissionSubject


class FakeTextReplyChannel:
    def __init__(self):
        self.prompts = []
        self.reply = "1"

    def prompt_for_text(self, recipient_id, prompt, timeout_seconds):
        self.prompts.append((recipient_id, prompt, timeout_seconds))
        return self.reply


def test_channel_text_permission_prompter_is_transport_agnostic():
    channel = FakeTextReplyChannel()
    prompter = ChannelTextPermissionPrompter(channel, "user-1", timeout_seconds=9)
    request = PermissionRequest(
        tool_name="shell.run",
        arguments={"command": "ls"},
        read_only=False,
        subject=PermissionSubject(
            kind="shell",
            tool_name="shell.run",
            shell_command="ls",
            shell_executable="ls",
            read_only=False,
        ),
    )

    choice = prompter.confirm(request)

    assert choice == "1"
    assert channel.prompts[0][0] == "user-1"
    assert "Colibri wants to run shell.run." in channel.prompts[0][1]
    assert channel.prompts[0][2] == 9


def test_format_channel_permission_prompt_includes_choices():
    request = PermissionRequest(
        tool_name="files.write",
        arguments={"path": "/tmp/a", "content": "x"},
        read_only=False,
        subject=PermissionSubject(
            kind="file_path",
            tool_name="files.write",
            file_path="/tmp/a",
            file_root="/tmp",
            read_only=False,
        ),
    )
    text = format_channel_permission_prompt(request)
    assert "path: /tmp/a" in text
    assert "1. once" in text


def test_format_channel_permission_prompt_uses_device_scopes():
    request = PermissionRequest(
        tool_name="gpio.write",
        arguments={"device": "controller", "pin": 13, "value": 1},
        read_only=False,
        subject=PermissionSubject(
            kind="hardware_device",
            tool_name="gpio.write",
            hardware_device="controller",
            read_only=False,
        ),
    )

    text = format_channel_permission_prompt(request)

    assert "hardware: gpio.write" in text
    assert "device: controller" in text
    assert "2. session-device" in text
    assert "4. user-device" in text
