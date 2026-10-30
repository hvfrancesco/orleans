# Roadmap

This document tracks release readiness and work that is intentionally deferred.
It is a living plan, not a compatibility promise.

## 1.0.0 release checklist

- [x] Stable SVG → toolpath → G-code → GRBL pipeline
- [x] G-code export, simulator and mock controller run
- [x] USB serial and WiFi TCP/Telnet transport
- [x] Visual editor: layers, fills, tabs, transforms, node editing, booleans,
      offsets, groups, arrays, raster import
- [x] Machine panel: status, homing, unlock, jog, work zero, framing,
      pause/resume and stop
- [x] Native `.orleans` project format
- [x] Six-language interface
- [x] 167 automated tests, Clippy and formatting checks
- [x] User documentation and development docs split from the README
- [x] Validate the complete workflow on the physical laser:
  - [x] connection and status reporting
  - [x] homing, jog and centre-to-work-zero
  - [x] framing with the laser off
  - [x] laser-off dry run and low-power test square
  - [x] pause, resume, stop and reconnect recovery
- [x] Add release packaging: release profile review, binary archive, AppImage,
      checksums and a short upgrade note
- [x] GitHub Actions CI and tag-driven release publishing
- [x] Final regression pass in all six languages and both themes

## Next milestones

### 1. Machine coverage

- [x] Add a Marlin laser driver (shipped in `1.1.0`): auto-detection, `M114`/`M105` status,
      `M3`/`M5` streaming, `M400` drain, `G28` homing, `G92` origin,
      `M999` unlock and `M112` stop, with a Marlin simulator and TCP test.
- [x] Add a GUI **Auto / GRBL / Marlin** driver override as a detection
      fallback (`1.1.1`).
- [x] Add GUI USB serial connections and saved machine profiles (`1.2.0`).
- [x] Expand machine profiles with work area, job defaults, framing setup and
      safety expectations, and add send/frame preflight checks (`1.3.0`).
- [x] Allow `orleans gui` to prefill TCP or serial connections (`1.3.0`).
- [x] Add machine profile import, export and duplicate.
- [ ] Validate the Marlin driver on physical hardware (stop/recovery,
      PWM range, homing and long-job behaviour).
- [ ] Add Ruida over UDP.
- Evaluate TopWisdom and galvo/fiber controllers.
- Add rotary-axis setup and job planning.

### 2. Transport and connectivity

- [x] ESP3D WebSocket terminal bridge and mDNS HTTP-service discovery.
- [ ] TLS (`wss://`) where the bridge supports it.
- [ ] Raw HTTP command bridge with explicit status limitations.
- [ ] Reconnect and job-resume hardening for unreliable WiFi links.

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
