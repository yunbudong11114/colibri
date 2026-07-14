# Colibri Weixin Auth QR Streaming Design

## Context

`colibri auth weixin` must print the Weixin login QR code before it starts waiting for scan confirmation. The Python runtime already streams QR output through a print callback as soon as the QR payload is fetched.

The Rust runtime previously collected QR lines inside `perform_weixin_auth` and returned them only after auth completed. In real terminal use this creates a deadlock-like experience: the command waits for a scan, but the QR code is not printed until after the scan can no longer happen.

## Design

- Keep the existing auth API sequence:
  - request QR code
  - print/render QR payload immediately
  - poll confirmation until confirmed, expired, or timed out
  - save only the Weixin auth fields to the active config on success
- Change the Rust auth implementation to accept an output callback for QR lines, matching the Python behavior.
- Keep token secrecy unchanged: never print `bot_token` to stdout.
- Keep config write semantics unchanged: only update `channels.weixin.enabled`, `channels.weixin.token`, and `channels.weixin.base_url`.

## Verification

- Add a Rust unit/runtime test that proves QR text is written before the status polling request is allowed to complete.
- Keep the existing auth success test to verify config preservation and token secrecy.
