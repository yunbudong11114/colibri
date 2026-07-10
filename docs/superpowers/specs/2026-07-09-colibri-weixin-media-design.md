# Colibri Weixin Media Design

Date: 2026-07-09

This document records two future Weixin media capabilities for Colibri, based on
the local PicoClaw implementation review. Colibri should borrow the shape of the
design while keeping memory use and dependencies small enough for a headless
CardputerZero-class server.

## 1. Send Local Files To Weixin

Goal:

Allow the model or user-approved tools to send a local file from the Colibri host
to the active Weixin conversation.

PicoClaw reference:

- `/Users/ybd/picoclaw/pkg/tools/fs/send_file.go`
- `/Users/ybd/picoclaw/pkg/bus/types.go`
- `/Users/ybd/picoclaw/pkg/media/store.go`
- `/Users/ybd/picoclaw/pkg/channels/weixin/media.go`
- `/Users/ybd/picoclaw/pkg/channels/weixin/api.go`

Observed PicoClaw flow:

```text
send_file tool
  -> validate the local path against allowed workspace roots
  -> inspect file existence, directory status, size, filename, and MIME type
  -> register the local path in MediaStore
  -> return a media://... reference
  -> agent pipeline builds OutboundMediaMessage
  -> ChannelManager.SendMedia
  -> WeixinChannel.SendMedia
  -> resolve media://... back to a local path
  -> call iLink getuploadurl
  -> encrypt and upload file bytes to Weixin CDN
  -> call iLink sendmessage with image, video, or file item
```

Recommended Colibri design:

- Add a small `MediaStore` abstraction that maps `media://...` references to
  local paths plus filename/content-type metadata.
- Add a channel-neutral outbound media message shape:

```text
OutboundMediaMessage:
  channel
  sender_id / chat_id
  parts:
    - type: image | audio | video | file
      ref: media://... | file://... | local absolute path | https://...
      filename
      content_type
      caption
```

- Add a `send_file` tool only after the channel media path exists. The tool
  should not silently exfiltrate host files.
- Treat sending a local file as a separate permission subject from reading a
  file. A file that is readable by Colibri is not automatically allowed to be
  sent to Weixin.
- Prefer project/session grants for repeated sends, but the default should ask
  through the active permission prompter.
- Keep uploads streaming or bounded by a configured max byte limit. Avoid
  retaining full file contents in long-lived session memory.

Minimal first version:

```text
files.send:
  path: absolute or workspace-relative path
  caption: optional text

Weixin implementation:
  image/* -> image item
  video/* -> video item
  everything else -> file item
```

Implementation boundary for the first Colibri change:

- Implement `files.send` as a built-in files-category tool. It is only
  available when `"files"` is enabled in `[tools].enabled`.
- The tool resolves the path exactly like other file tools, checks existence,
  rejects directories, detects a conservative content type from the file
  extension, and returns a structured media result. It does not read file bytes
  into the model context.
- `files.send` is always write/exfiltration-sensitive even though it reads a
  local path. It must use file-path permission prompts and must not be covered
  by read-only default allow rules.
- CLI-only sessions do not have a channel media sender. In that case the tool
  should fail clearly with `media_unavailable` instead of pretending the file was
  sent.
- Gateway sessions inject a channel media sender into `ToolContext`. After a
  successful `files.send`, `AgentSession` calls that sender with a `MediaPart`.
- The Weixin channel exposes `send_media(recipient_id, media_part)`. The first
  implementation may keep upload details behind `WeixinApiClient.upload_media`
  and `WeixinApiClient.send_media`, so unit tests can verify payload routing
  without making real network calls.
- The Weixin API client uses `pycryptodome` only for AES-ECB encryption. Other
  HTTP, hashing, random-byte, base64, and request-building logic stays on Python
  standard library modules.
- `WeixinApiClient.upload_media` must follow PicoClaw's real iLink upload flow:
  read the local file with a bounded max size, generate a 16-byte AES key and
  random file key, call `ilink/bot/getuploadurl`, PKCS7-pad and AES-ECB-encrypt
  file bytes, upload the encrypted bytes to the returned CDN URL, read
  `X-Encrypted-Param`, and return enough metadata for the final message item.
- `WeixinApiClient.send_media` must send an optional caption as text first, then
  send an image/video/file item with `encrypt_query_param`, base64-encoded AES
  key, `encrypt_type = 1`, filename, and original file length.
- If upload/send fails, the tool result must be an error that is visible to the
  model and transcript.

Out of scope for the first Colibri implementation:

- multi-file album sending,
- resumable uploads,
- marketplace/plugin packaging,
- generic channel media fanout beyond the current Weixin channel.

## 2. Receive Weixin Files And Images

Goal:

When a Weixin user sends an image, file, video, or voice message to Colibri,
Colibri should preserve the attachment as structured media and pass a bounded
reference to the agent session.

PicoClaw reference:

- `/Users/ybd/picoclaw/pkg/channels/weixin/weixin.go`
- `/Users/ybd/picoclaw/pkg/channels/weixin/media.go`
- `/Users/ybd/picoclaw/pkg/bus/types.go`
- `/Users/ybd/picoclaw/pkg/media/store.go`

Observed PicoClaw flow:

```text
Weixin long-poll message
  -> parse item_list
  -> build text placeholders such as [image], [file: name], [video]
  -> select downloadable image/video/file/voice item
  -> download media bytes from Weixin CDN
  -> decrypt when the item carries encrypted media metadata
  -> write bytes to a managed temporary file
  -> register the file in MediaStore
  -> attach media://... to InboundMessage.media
  -> agent receives text content plus structured media refs
```

Important PicoClaw behavior:

- The inbound message type has a separate `Media []string` field, so media is
  not only embedded in text.
- Weixin image, file, video, and voice items are all considered downloadable
  media when the item carries an encrypted query parameter or full URL.
- Voice with server-side text transcription is treated as text. Voice without
  text is downloaded; PicoClaw tries to transcode SILK to WAV and falls back to
  storing the SILK file.
- The current PicoClaw selector returns one media item per message, prioritized
  as image, video, file, voice. Colibri can start with the same single-attachment
  behavior and later extend to multiple attachments.

Recommended Colibri design:

- Extend the channel inbound message shape with a `media` list of references.
- Store incoming media under a bounded temporary media directory, not in session
  history as raw bytes.
- Register each downloaded file in the same small `MediaStore` used by outbound
  media.
- Add content placeholders to the text prompt so non-vision models still know an
  attachment arrived:

```text
[image]
[file: report.pdf]
[video]
[audio]
```

- If the active model supports image input, the session/model adapter may resolve
  image `media://...` refs into the provider's accepted image input format.
- For non-image files, expose a separate read/analyze tool path instead of
  blindly injecting file bytes into the prompt.
- Enforce max download size and cleanup policy. Inbound attachments should be
  deleted when their media scope expires.

Minimal first version:

```text
Weixin inbound:
  image -> media://... plus [image]
  file  -> media://... plus [file: filename]

Deferred:
  video
  voice/SILK transcoding
  multiple attachments in one Weixin message
```

Security notes:

- Inbound files are untrusted external input. Do not execute them.
- Do not automatically write inbound files into the project workspace.
- Make tool descriptions clear that `media://...` refs come from chat channels
  and may need user confirmation before being copied, read, transformed, or sent
  elsewhere.
