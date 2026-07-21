# Colibri Hardware Foundation Design

Date: 2026-07-20

## Goal

Prepare Colibri for CardputerZero hardware integration without guessing the
final Linux device tree, bus numbers, GPIO line mappings, or driver exposure
before the physical device is available.

This phase adds:

- a shared Python/Rust hardware configuration surface;
- a small hardware capability and device inventory model;
- a read-only `hardware.probe` agent tool;
- a `colibri hardware probe` CLI command;
- deterministic fake-root tests for both runtimes.

It does not add GPIO, I2C, SPI, serial, audio, camera, display, input, RTC, IMU,
or infrared writes.

## Design Sources

PicoClaw directly exposes Linux I2C and SPI device files and implements
cross-platform serial access. Its useful constraints are stateless operations,
strict device-name validation, bounded reads and writes, and explicit
timeouts.

ZeroClaw separates device identity, capabilities, and transport. Its useful
constraints are stable aliases, capability checks, path allowlists, lazy
device opening, protocol timeouts, and optional hardware build features.

Colibri keeps its existing synchronous, low-memory runtime and interactive
permission policy. It does not copy PicoClaw's model-supplied `confirm: true`
as an authorization boundary, and it does not copy ZeroClaw's async hardware
stack or probe/datasheet dependencies.

## Configuration

Add a top-level section:

```toml
[hardware]
enabled = false
discovery = "on_demand"
```

`enabled` controls whether the agent may receive hardware tools.
`discovery` only accepts `on_demand` in this phase. The explicit field reserves
the lifecycle contract without creating a resident hotplug task.

The `hardware` tool category must also be present in `tools.enabled` before
`hardware.probe` is exposed to the model. Requiring both settings prevents an
old configuration from unexpectedly exposing device information after an
upgrade.

The CLI command remains available while `hardware.enabled = false` because it
is an onboarding diagnostic and performs no device I/O.

Python and Rust reject unknown hardware fields and unsupported discovery modes
with matching configuration errors.

## Probe Model

The probe returns compact JSON:

```json
{
  "platform": "linux",
  "board_model": "Raspberry Pi ...",
  "capabilities": ["audio", "gpio", "i2c"],
  "devices": {
    "audio": ["/dev/snd/controlC0"],
    "gpio": ["/dev/gpiochip0"],
    "i2c": ["/dev/i2c-1"]
  }
}
```

`board_model` is `null` when unavailable. Capability names and device keys are
sorted. Empty device categories are omitted.

The Linux probe recognizes only standard kernel-facing device nodes:

- GPIO: `/dev/gpiochip*`
- I2C: `/dev/i2c-*`
- SPI: `/dev/spidev*`
- serial: `/dev/ttyS*`, `/dev/ttyUSB*`, `/dev/ttyACM*`, `/dev/ttyAMA*`,
  `/dev/rfcomm*`
- audio: children of `/dev/snd`
- camera: `/dev/video*`
- display: `/dev/fb*`, `/dev/dri/card*`
- input: `/dev/input/event*`
- IIO sensors: `/dev/iio:device*`
- RTC: `/dev/rtc*`
- infrared: `/dev/lirc*`

macOS serial discovery recognizes `/dev/tty.*` and `/dev/cu.*`. Other
categories stay empty until a platform implementation exists.

The probe reads `/proc/device-tree/model` for the board model and strips the
trailing NUL used by device tree properties. It does not run shell commands,
load kernel modules, open device nodes, or inspect their contents.

## Testability

The production probe uses `/proc` and `/dev`. Its core accepts alternate proc
and dev roots so tests can create a deterministic fake filesystem. Returned
paths are normalized back to logical `/dev/...` paths, keeping Python and Rust
outputs identical.

The test suite covers:

- default and overridden hardware configuration;
- strict rejection of unknown fields and unsupported discovery modes;
- deterministic capability and device detection;
- device-tree NUL removal;
- read-only tool metadata;
- disabled and enabled tool registration;
- CLI JSON output;
- Python/Rust parity mapping.

## Security

`hardware.probe` is read-only under the existing permission policy. It only
returns device-node names and a board model.

Future hardware operations must:

- resolve model-visible aliases to internal paths;
- enforce configuration allowlists before interactive permission grants;
- classify reads and writes separately;
- bound payload size and operation time;
- open devices per operation and close them immediately;
- never allow a user grant to override a hard-deny device or operation.

## Memory and Runtime Cost

The implementation uses standard filesystem iteration and JSON serialization.
It creates no threads, watchers, caches, background workers, or resident device
handles. Probe memory is proportional to the small number of device nodes and
is released after the command or tool call.

No Python package or Rust crate is added.

## Follow-up Phases

After real-device probing confirms the kernel interfaces:

1. add stable device aliases and explicit allowlists;
2. add a bounded newline-delimited serial JSON transport and simulator;
3. add read-only GPIO/I2C inventory operations;
4. add writes through Colibri's existing interactive permission system;
5. integrate audio, camera, display, and input through ALSA, V4L2, DRM/fb, and
   evdev rather than raw bus commands.
