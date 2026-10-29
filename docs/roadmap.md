# Roadmap

This document tracks release readiness and work that is intentionally deferred.
It is a living plan, not a compatibility promise.

## 0.1.0 release checklist

- [x] Stable SVG → toolpath → G-code → GRBL pipeline
- [x] G-code export, simulator and mock controller run
- [x] USB serial and WiFi TCP/Telnet transport
- [x] Visual editor: layers, fills, tabs, transforms, node editing, booleans,
      offsets, groups, arrays, raster import
- [x] Machine panel: status, homing, unlock, jog, work zero, framing,
      pause/resume and stop
- [x] Native `.orleans` project format
- [x] Six-language interface
- [x] 166 automated tests, Clippy and formatting checks
- [x] User documentation and development docs split from the README
- [ ] Validate the complete workflow on the physical laser:
  - [ ] connection and status reporting
  - [ ] homing, jog and centre-to-work-zero
  - [ ] framing with the laser off
  - [ ] laser-off dry run and low-power test square
  - [ ] pause, resume, stop and reconnect recovery
- [ ] Add release packaging: version bump, release profile review, binary
      artifacts, checksums and a short upgrade note
- [ ] Final regression pass in all six languages and both themes

## Next milestones

### 1. Machine coverage

- Add a Marlin laser driver.
- Add Ruida over UDP.
- Evaluate TopWisdom and galvo/fiber controllers.
- Add rotary-axis setup and job planning.
- Expand machine profiles beyond GRBL `$` settings.

### 2. Transport and connectivity

- WebSocket/HTTP bridge support.
- TLS where the bridge supports it.
- mDNS discovery for Grbl_ESP32, FluidNC and ESP3D.
- Reconnect and job-resume hardening for unreliable WiFi links.

### 3. Import coverage

- HPGL import.
- DXF hatches, dimensions and block attributes.
- Better SHX/CAD text matching.
- PDF raster images, clipping paths and transparency groups.
- SVG features that currently require pre-flattening.

### 4. Geometry and toolpaths

- Optional curve-preserving booleans and offsets.
- Arc (`G2`/`G3`) output for supported controllers.
- Acceleration-aware time estimates.
- Non-uniform and skewed scaling.
- Polar/circular arrays.
- Nested groups and richer group transforms.
- Node smoothing and node-type conversion.

### 5. Performance

- Spatial index for hit-testing and culling.
- Level-of-detail canvas rendering for very large designs.
- Faster flattening and toolpath planning for complex imports.
- Incremental redraw instead of rebuilding all polylines every frame.

### 6. Job control

- Feed and power overrides during a job.
- Per-job notes and richer history filters.
- Job queueing and repeat-last-job workflow.
- Configurable framing speed and margin.
- Machine-specific safety checks and preflight warnings.

### 7. Interface and diagnostics

- Complete localisation of diagnostics and platform file dialogs.
- More keyboard navigation and accessibility labels for icon-only controls.
- Better import diagnostics with per-entity warnings.
- Guided first-connection and first-cut checklist.

## Known limitations

Current constraints are documented in
[development.md](development.md#known-limitations). They are deliberately kept
separate from this roadmap so that the release notes describe what exists today
rather than what is planned.
