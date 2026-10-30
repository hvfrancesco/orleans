# Changelog

All notable changes to orleans are recorded here.

## [1.2.0] - 2026-10-30

### Machine connections

- Added USB serial support to the GUI Machine panel with port entry, detected
  port list, refresh and configurable baud rate. TCP/Telnet remains available.
- The same Auto / GRBL / Marlin detection and protocol override works over both
  transports.
- Added saved machine profiles containing connection type, address, baud rate,
  driver preference and max S. Profiles are stored in
  `~/.config/orleans/machines.json`.
- Reconnects now reuse the complete TCP or serial connection settings.

### Validation

- 190 automated tests, including a GRBL serial test through a pseudo-terminal.
- Clippy with warnings denied, formatting checks and both CI workflows pass.

### Upgrade notes

- The `.orleans` project format is unchanged (format 2).
- Existing UI preferences remain compatible. Machine profiles are a new,
  separate file.
- The AppImage relies on the host `libudev` for USB serial access.

## [1.1.1] - 2026-10-30

### Interface

- Added a **Driver: Auto / GRBL / Marlin** selector to the GUI Machine panel.
  Auto-detection remains the default; the override is a recovery path when a
  controller does not reveal its firmware identity. Reconnects reuse the
  selected protocol.

### Validation

- 186 automated tests, Clippy with warnings denied and formatting checks pass.
- Forced Marlin connection is covered by a TCP test where `M115` returns no
  identity; status and settings still come from `M114`/`M105`.

## [1.1.0] - 2026-10-30

### Machine

- Added a Marlin laser driver with `M115` identity probing, `M114` position
  and `M105` temperature status, `M3`/`M5` laser streaming, `M400` planner
  drain, `G28` homing, `G92` work origin and `G91`/`G90` jogging.
- Added automatic GRBL/Marlin detection for the GUI and the CLI, plus an
  explicit `--driver auto|grbl|marlin` override for `run`, `stop`, `unlock`
  and `status`.
- Added Marlin-specific safety and recovery: Stop sends `M5` followed by
  `M112`; Unlock sends `M999`; saved `G54` work zero is disabled because Marlin
  only supports the temporary `G92` form; pause is disabled for Marlin jobs.
- Added a Marlin G-code dialect that omits `G17`/`G54` and defaults **Max S**
  to `255`, with an automatic clamp when a GRBL project is sent to Marlin.
- Added `SimulatedMarlin` and simulator/CLI/TCP tests covering identity,
  status, streaming, `M400` drain, stop and protocol detection.

### Documentation

- Documented Marlin connection, settings, stop/unlock recovery and limitations
  in the README, GUI, CLI, troubleshooting and roadmap documents.

### Validation

- 185 automated tests, Clippy with warnings denied and formatting checks pass.
- Marlin has been validated against the simulator and a local TCP test server,
  but not yet on physical hardware.

### Upgrade notes

- The `.orleans` project format is unchanged (format 2).
- GRBL streaming and controls are unchanged; auto-detection now falls through
  from a GRBL status probe to Marlin `M115` when no status arrives.
- Marlin defaults to a `0..255` PWM range. Set **Max S** or `--max-s` when the
  firmware uses a different range.

## [1.0.0] - 2026-10-29

First stable release.

### Core

- SVG, PDF/AI, DXF and PNG/JPEG/BMP import.
- Clipper2 kerf compensation, boolean operations and offset/inset editing.
- Line and fill layers, scanline and offset fills, cross-hatch and outline passes.
- Automatic and manual holding tabs.
- Inside-out ordering, nearest-neighbour travel optimisation and multi-pass jobs.
- G-code generation for GRBL, GrblHAL, FluidNC and Marlin-style laser commands.
- Hardware-free GRBL simulator and mock end-to-end runs.
- Native `.orleans` format 2 projects with full geometry, groups and raster data.

### Editor

- Dark and light themes with millimetre rulers and machine-bed display.
- Layer management, cut order, colour matching and move/copy between layers.
- Shape selection, directional window selection, groups, align and distribute.
- Uniform scaling, rotation with optional 15° Ctrl snapping, and numeric positioning.
- Node editing: insert, move, break, join and delete multiple anchors.
- Duplicate, rectangular arrays, undo/redo and Inkscape-compatible SVG clipboard.
- Raster placement, rotation, cropping and engraving preview.
- Six interface languages: English, Italian, French, Spanish, German and Chinese.
- Help > About dialog with version, author, license and icon attribution.

### Machine

- GRBL over USB serial or WiFi TCP/Telnet with character-counting flow control.
- Live status, position, feed and laser power readout.
- Homing, unlock, jog, jog cancellation and go-to-work-zero.
- Temporary `G92` and stored `G54` work zero.
- Laser-off framing and job streaming with progress.
- Pause/resume for M4 jobs, stop, automatic idle wait and reconnect handling.
- Persistent job history and controller settings editing.
- Validated on the physical laser: connection, homing, jog, framing, dry run,
  low-power test square and pause/resume/stop/reconnect recovery.

### Validation

- 167 automated tests, including SVG → toolpath → G-code → TCP controller.
- Clippy with warnings denied, formatting checks and release build.
- Hardware validation completed on 2026-10-29.

### Release packaging

- Linux x86_64 archive with the binary, docs, licenses and sample design.
- Linux x86_64 AppImage with desktop integration and icon; double-clicking it
  opens the editor, while explicit CLI commands still work.
- SHA-256 checksums included next to both artifacts.
- Release builds use thin LTO and stripped symbols.
- GitHub Actions CI and tag-driven release publishing.

### Upgrade notes

This is the first stable release. Projects written by pre-release builds that use
an unversioned project format are not supported; current projects use format 2.

### Known limitations

See [docs/development.md](docs/development.md#known-limitations). Upcoming work is
tracked in [docs/roadmap.md](docs/roadmap.md).
