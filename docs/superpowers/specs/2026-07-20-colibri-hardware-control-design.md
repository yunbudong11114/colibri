# Colibri Hardware Control Design

Date: 2026-07-20

## Goal

Extend the read-only hardware foundation with the device, transport, tool, and
permission layers needed before CardputerZero native drivers are known.

This phase adds:

- stable device aliases backed by an explicit configuration allowlist;
- bounded newline-delimited JSON over serial;
- a deterministic standard-input/standard-output protocol simulator;
- raw serial read and write tools;
- GPIO, I2C, and SPI tools implemented through a configured serial JSON
  controller;
- device-scoped session and persistent permission grants for side effects.

This phase does not guess CardputerZero GPIO character-device ioctls, I2C bus
numbers, SPI modes, or board-specific pin mappings. Native ALSA, V4L2, DRM,
evdev, IIO, GPIO, I2C, and SPI backends remain a later real-device phase.

## Configuration

The existing hardware section gains bounded operation defaults and an array of
allowed devices:

```toml
[hardware]
enabled = false
discovery = "on_demand"
operation_timeout_seconds = 2.0
max_transfer_bytes = 4096

[[hardware.devices]]
name = "controller"
path = "/dev/ttyACM0"
transport = "serial_json"
baud_rate = 115200
capabilities = ["serial", "gpio", "i2c", "spi"]
allow_write = false
```

`name` is the only device identifier visible to the model. Names must contain
1-64 ASCII letters, digits, dots, underscores, or hyphens, and must start with
an alphanumeric character.

`path` must be an absolute path below `/dev` after lexical normalization. The
configured entries are the device allowlist: tools cannot accept arbitrary
paths.

`transport` only accepts `serial_json` in this phase. `baud_rate` accepts
standard Unix serial speeds supported by both runtimes. Capabilities are a
deduplicated subset of `serial`, `gpio`, `i2c`, and `spi`.

`allow_write` is a hard safety gate. A permission grant can never enable a
side-effecting operation while it is false.

`operation_timeout_seconds` must be positive and at most 60 seconds.
`max_transfer_bytes` must be between 1 and 65536 bytes. Both limits apply
before device I/O and cannot be enlarged by model arguments.

The existing dual exposure gate remains:

1. `hardware.enabled = true`;
2. `"hardware"` is present in `tools.enabled`.

## Serial Transport

The serial transport is synchronous and operation-scoped:

1. resolve the model-visible alias;
2. check capability and write policy;
3. acquire the process-wide hardware I/O mutex;
4. open the configured path;
5. configure raw 8N1 mode at the configured baud rate;
6. perform a bounded read, write, or request;
7. close the descriptor and release the mutex before returning.

There are no resident readers, reconnect loops, caches, or device handles.
The single small mutex prevents concurrent gateway sessions from interleaving
frames on the same controller without creating a worker thread or per-device
state cache.

Controller requests are one compact JSON object followed by `\n`:

```json
{"id":"1","cmd":"gpio_read","args":{"pin":13}}
```

Responses use the same request ID:

```json
{"id":"1","ok":true,"result":{"value":1}}
```

Errors use:

```json
{"id":"1","ok":false,"error":"invalid_pin"}
```

The client ignores blank lines, rejects malformed JSON, rejects mismatched
request IDs, and fails when the first complete response exceeds
`max_transfer_bytes`. Timeout and size errors use stable cross-runtime error
types. The controller connection must remain open until the complete response
frame has been read; PTY transport tests retain the peer descriptor through the
client assertion so an immediate peer close cannot race buffered response data.

Raw serial tools do not use JSON framing:

- `serial.read` reads up to the configured bound and returns UTF-8 with
  replacement for invalid bytes;
- `serial.write` writes the UTF-8 bytes of the provided text.

## Simulator

`colibri hardware simulate` runs an explicit foreground protocol simulator on
standard input and output. It reads one request per line and writes one response
per line. It keeps only small in-memory GPIO and I2C maps for the life of that
process.

The simulator supports:

- `gpio_read`, `gpio_write`;
- `i2c_scan`, `i2c_read`, `i2c_write`;
- `spi_transfer`.

SPI transfer echoes the provided hexadecimal bytes. Unwritten GPIO pins read
as zero. I2C reads return stored bytes padded with zeros. The simulator makes
protocol and tool behavior testable without claiming to emulate electrical
hardware.

## Agent Tools

All tool schemas reject additional properties.

Read-only tools:

- `hardware.probe`: discover host device nodes;
- `hardware.devices`: list configured aliases and capabilities;
- `serial.read(device, max_bytes?)`;
- `gpio.read(device, pin)`;
- `i2c.scan(device)`;
- `i2c.read(device, address, register?, length)`.

Side-effecting tools:

- `serial.write(device, data)`;
- `gpio.write(device, pin, value)`;
- `i2c.write(device, address, register?, data)`;
- `spi.transfer(device, data)`.

GPIO pins are non-negative integers. I2C addresses are 7-bit integers from 0
through 127. Registers, when present, are bytes from 0 through 255. Binary I2C
and SPI payloads are even-length hexadecimal strings and are returned as
lowercase hexadecimal.

GPIO, I2C, and SPI tools send the corresponding controller request through the
device's `serial_json` transport. Native Linux bus access is intentionally not
implemented in this phase.

## Permission Model

Side-effecting hardware tools create a `hardware_device` permission subject
bound to the configured device alias.

Prompts offer:

```text
1. once
2. session-device
4. user-device
0. deny
```

Session grants are held by the existing session permission policy. Persistent
grants are stored as:

```toml
[hardware]
devices = ["controller"]
```

A device grant applies to side-effecting hardware tools for that alias. It does
not make an unconfigured alias valid and does not override `allow_write =
false`, capability checks, transfer limits, or timeouts.

Read-only hardware tools continue to follow
`tools.default_permission = "allow_read_confirm_write"` and therefore run
without a prompt under the default policy.

## Errors

Python and Rust use matching error types:

- `invalid_arguments`: schema-level or range validation failed;
- `unknown_device`: alias is not configured;
- `unsupported_operation`: the device lacks the required capability;
- `permission_denied`: `allow_write` is false;
- `not_found`: configured device path does not exist;
- `io_error`: device open, configuration, read, or write failed;
- `timeout`: operation did not finish before the configured timeout;
- `result_too_large`: a response exceeded the transfer bound;
- `protocol_error`: malformed or mismatched JSON response;
- `device_error`: controller returned `ok = false`.

## Security and Resource Limits

- Models never provide device paths.
- Every operation resolves an exact configured alias.
- Side effects require both `allow_write = true` and normal permission approval.
- Payload and response sizes are checked before unbounded allocation.
- Device descriptors are opened lazily and always closed.
- No operation creates a background thread in the agent runtime.
- The simulator is only started by its explicit CLI command.
- No new Python package or Rust crate is introduced.

## Test Strategy

Both runtimes must cover:

- strict and identical configuration parsing;
- aliases, path validation, duplicate rejection, and capability validation;
- configured-device inventory without exposing arbitrary paths to tool
  arguments;
- protocol request and response validation;
- deterministic simulator state transitions;
- all tool argument, capability, hard-deny, timeout, and size boundaries;
- `hardware_device` prompt formatting, session grants, persistent grants, and
  concurrent merge-safe permission persistence;
- CLI simulator input/output;
- tool schemas and Python/Rust parity mapping.

The full existing Python and Rust suites must remain green, followed by a Rust
release build.
