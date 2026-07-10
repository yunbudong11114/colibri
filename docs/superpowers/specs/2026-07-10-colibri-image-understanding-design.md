# Colibri Image Understanding Design

Date: 2026-07-10

## Goal

Add an `image.understand` tool that lets the agent ask a compatible multimodal
model to analyze a local image. The tool must work for both REPL sessions and
channel/gateway sessions, reuse the existing attachment paths and permission
system, and keep image bytes out of session history and long-lived memory.

## Model routing

The agent model remains the default image model. An optional `[vision]` section
can override only the fields needed for image requests:

```toml
[vision]
model = ""
base_url = ""
api_key = ""
timeout_seconds = 60
max_image_bytes = 4194304
```

Resolution order:

1. `vision.model`, `vision.base_url`, and `vision.api_key` when configured.
2. The corresponding `[model]` value when the vision value is empty.
3. `COLIBRI_API_KEY` for the API key when both config values are empty.

The existing OpenAI-compatible adapter will support a single image content
part encoded as a bounded `data:` URL. Encoding happens only during the
request. The image is never added to `Message`, transcript, context summaries,
or memory files.

## Tool contract

Tool name: `image.understand`

Arguments:

```json
{
  "path": "/tmp/colibri/media/photo.png",
  "prompt": "请描述图片中的主要内容"
}
```

The tool validates that the path resolves to an existing regular image file,
checks its MIME type, enforces `vision.max_image_bytes`, and returns the model's
bounded text response. It does not write files or execute image contents.

## Permission and path boundary

Image understanding follows the same path behavior as `files.read`:

- paths under the current working directory, configured `[files].roots`,
  `/tmp/colibri/media`, or an already granted root run without a prompt;
- paths outside those roots go through the existing permission prompt;
- REPL prompts in the terminal and gateway prompts through the active channel;
- `once`, `session`, `project`, and `deny` keep their existing meanings;
- a denial returns `permission_denied` to the model;
- the tool cannot bypass path resolution, symlink checks, or dynamic grants.

The tool is not treated as an unconditional read bypass. Its permission
decision is made before the tool reads or encodes the image.

## Runtime flow

```text
channel or REPL input
  -> attachment is saved and represented by a local path
  -> agent decides whether image.understand is needed
  -> session applies the normal path permission decision
  -> ImageAnalyzer selects vision override or agent model
  -> bounded image bytes become a temporary multimodal request
  -> model text returns as the tool result
  -> agent continues the normal tool loop
```

If the configured provider is not image-capable, the request returns a clear
model error. The session and gateway remain alive. No automatic image analysis
is performed for every incoming attachment.

## Memory and safety limits

- Default maximum image size is 4 MiB.
- Only one image is sent per tool call.
- Image bytes are released after the model request.
- Tool output uses the existing `tools.max_result_chars` bound.
- Binary image content is not included in context compaction or transcript
  payloads.

## Testing scope

Tests cover configuration fallback, multimodal request construction, image size
and MIME validation, allowed-path behavior, permission prompting for outside
paths, tool registration, and the normal session tool loop.
