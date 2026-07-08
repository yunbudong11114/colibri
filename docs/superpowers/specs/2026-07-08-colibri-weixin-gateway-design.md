# Colibri Weixin Gateway Design

## Goal

Add Colibri's first channel integration: a Weixin personal-account channel backed by Tencent iLink API, started through a `gateway` CLI command.

The gateway must stay small enough for CardputerZero-class devices:

- no web dashboard,
- no resident database,
- no third-party packages,
- bounded in-memory channel sessions,
- text-only Weixin messages for the first implementation.

## Requirements

1. CLI uses gateway terminology because channel interfaces are intentionally extensible.
2. Implement Weixin channel support first.
3. Implement permission confirmation in Weixin, not only local terminal confirmation.
4. Add `session.idle_exit_enabled` and keep `session.idle_exit_seconds = 300` configurable. Default idle exit is disabled.
5. Change documentation before code.

## Reference Decision

Colibri should follow PicoClaw's channel shape more than ZeroClaw's.

PicoClaw's Weixin implementation is a compact long-poll channel:

- `picoclaw auth weixin` obtains a token through iLink QR login.
- `picoclaw gateway` starts configured channels.
- `channel_list.weixin.token` stores the iLink bot token.
- `allow_from` limits who can talk to the bot.

ZeroClaw's channel subsystem is more general and powerful, but too heavy for Colibri's current size target: async trait objects, feature gates, large orchestrator state, per-channel observability, LRU conversation maps, approval channels, and many platform-specific extras.

Colibri will use a smaller Python gateway inspired by PicoClaw:

- one gateway command,
- one channel registry,
- one Weixin long-poll implementation,
- bounded `AgentSession` cache keyed by channel user.

## Configuration

```toml
[session]
idle_exit_enabled = false
idle_exit_seconds = 300

[gateway]
enabled_channels = ["weixin"]
max_sessions = 4
session_idle_seconds = 600

[channels.weixin]
enabled = false
token = ""
base_url = "https://ilinkai.weixin.qq.com/"
allow_from = []
poll_timeout_seconds = 35
auth_timeout_seconds = 300
```

`allow_from = []` means open access, matching existing Colibri defaults. Users should set explicit IDs for personal deployments.

## CLI

```bash
colibri auth weixin
colibri gateway
```

`auth weixin` starts the iLink QR login flow. Without third-party QR libraries, Colibri prints the QR payload URL. If the terminal or client can render it externally, the same payload is sufficient for login.

On successful login, Colibri writes `channels.weixin.token`, `channels.weixin.base_url`, and `channels.weixin.enabled = true` into the active config path. If `--config` is omitted, it writes `~/.colibri/config.toml`.

`gateway` loads config, starts all enabled channels from `gateway.enabled_channels`, and blocks until interrupted.

## Internal Interfaces

`colibri.gateway` owns process orchestration:

- builds one model client,
- builds one tool registry,
- creates per-peer sessions lazily,
- evicts idle sessions,
- routes channel messages into `AgentSession.submit()`,
- sends replies back through the originating channel.

`colibri.channels.base` defines:

- `InboundMessage`
- `Channel`
- `ChannelContext`

`colibri.channels.weixin` implements:

- iLink API client,
- auth QR status polling,
- long-poll message receiving,
- text reply sending,
- permission confirmation prompt.

## Weixin Text Flow

1. Gateway starts `WeixinChannel`.
2. Channel long-polls `ilink/bot/getupdates`.
3. For each finished user text message:
   - read `from_user_id`,
   - persist `context_token` in memory for replies,
   - apply `allow_from`,
   - pass text to gateway.
4. Gateway picks or creates `AgentSession` for `weixin:<from_user_id>`.
5. Session returns assistant text.
6. Channel sends text through `ilink/bot/sendmessage`.

The first version ignores image, voice, file, video, and group media payloads. It may surface a short unsupported-media message later.

## Weixin Permission Confirmation

When channel mode hits a permission prompt, Colibri must ask through Weixin.

The prompter sends a text prompt to the same Weixin user:

```text
Colibri wants to run shell.run
command: pwd

Reply one of:
y = once
s = session
e = executable session
p = project
n = deny
```

Then the prompter waits for the next text reply from that user and maps it to the existing permission choices. For non-shell tools it omits the executable option.

This is intentionally text-based. Buttons can be added later if the iLink API exposes suitable structured controls.

## Limits

- `gateway.max_sessions` bounds concurrent `AgentSession` objects.
- `gateway.session_idle_seconds` evicts inactive channel sessions.
- Weixin permission wait is blocking for the current user turn.
- During the first version, unrelated Weixin messages received while waiting for permission may be skipped rather than queued. This keeps memory bounded and implementation simple.

## Tests

- config defaults and overrides for idle exit, gateway, and Weixin channel,
- CLI parser accepts `auth weixin` and `gateway`,
- gateway session cache reuses and evicts sessions,
- Weixin API client builds iLink headers and payloads,
- Weixin text extraction ignores non-user/non-finished messages,
- Weixin permission prompter sends prompt and maps reply choices,
- REPL idle timeout is disabled by default and works when enabled.
