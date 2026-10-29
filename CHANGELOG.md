# Changelog

All notable changes to orleans are recorded here.

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
- Linux x86_64 AppImage with desktop integration and icon.
- SHA-256 checksums included next to both artifacts.
- Release builds use thin LTO and stripped symbols.

### Upgrade notes

This is the first stable release. Projects written by pre-release builds that use
an unversioned project format are not supported; current projects use format 2.

### Known limitations

See [docs/development.md](docs/development.md#known-limitations). Upcoming work is
tracked in [docs/roadmap.md](docs/roadmap.md).
