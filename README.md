# orleans

An open, clean-room CAM stack for laser cutters, written in Rust.

This is **not** a fork of LightBurn and contains no LightBurn code, assets or
proprietary file formats. It reimplements the same workflow — vector design in,
G-code out, streamed to a controller over USB or WiFi — from public
specifications: SVG, G-code and the GRBL protocol.

The commercial binary in `../LightBurn` was only used to inventory which device
families exist (GRBL, Marlin, Smoothieware, Ruida, TopWisdom, galvo). Nothing
was decompiled or copied.

---

## What works today

| Capability | Status |
|---|---|
| SVG import (shapes→paths, transforms, CSS, physical units) | ✅ |
| Colours → layers, per-layer speed/power/passes | ✅ |
| Kerf compensation via Clipper2 (outer shrinks, holes grow) | ✅ |
| Boolean ops (union / difference / intersection) | ✅ |
| Inside-out ordering + nearest-neighbour travel optimisation | ✅ |
| Multi-pass cutting | ✅ |
| Fill/hatch layers (angle, spacing, overlap, alternate) | ✅ |
| G-code for GRBL / GrblHAL / FluidNC / Marlin-laser (`M3`, `M4`) | ✅ |
| GRBL streaming: character-counting flow control, `ok`/`error`, progress, status, feed hold, resume, soft reset, automatic stop on failure | ✅ |
| USB serial and WiFi TCP/Telnet (Grbl_ESP32, FluidNC, ESP3D) | ✅ |
| Hardware-free GRBL simulator | ✅ |
| DXF / PDF / AI import | ❌ |
| Raster engraving and dithering | ❌ |
| Tabs/bridges on contour cuts | ✅ |
| GUI viewer: layers, speed/power/passes, live stats, G-code export | ✅ |
| Layer list: selection, Show/Cut, rename, colour, cut-order reorder | ✅ |
| Canvas: selection, drag-move, handles, live size/position, undo | ✅ |
| Project save/load (`.orleans`, embedded SVG + settings/path edits) | ✅ |
| Machine bed/travel from `$130`/`$131` drawn on the canvas | ✅ |
| GUI machine control: connect, status, home, unlock, stop | ✅ |
| GUI job streaming: progress, stop, automatic idle wait | ✅ |
| GUI laser-off framing pass | ✅ |
| GUI jog (X/Y) and work zero: temporary `G92` or stored `G54` | ✅ |
| Controller settings editor (`$32`, `$30`, `$22`, raw `$$`) | ✅ |
| GUI pause/resume (M4 + `$32=1`; framing always) | ✅ |
| Node editing: drag, insert, delete, grid snap | ✅ |
| Ruida, TopWisdom, galvo/fiber, rotary | ❌ |
| LBRN2 project files | ❌ |

112 unit and integration tests, including a full SVG → toolpath → G-code → TCP
controller run against a local GRBL server.

## Quick start

Requires a recent stable Rust toolchain and a C++ compiler (Clipper2 is built
from source).

```bash
cargo build --release

# Open the visual editor (the SVG is optional)
cargo run -q -- gui examples/coaster.svg --url tcp://192.168.1.10

# Inspect a design: size, layers, cut length, time estimate
cargo run -q -- info examples/coaster.svg

# Generate G-code
cargo run -q -- gcode examples/coaster.svg -o coaster.gcode \
    --speed 1200 --power 35 --kerf 0.15 --passes 2

# Dry-run the whole pipeline against the built-in simulator
cargo run -q -- run examples/coaster.svg --mock

# Cut for real over USB (GRBL defaults to 115200 baud)
cargo run -q -- run examples/coaster.svg --port /dev/ttyUSB0 \
    --speed 1200 --power 35

# Cut over WiFi (Telnet on port 23 is the default)
cargo run -q -- run examples/coaster.svg --url tcp://192.168.1.50 \
    --speed 1200 --power 35

# Query a controller over either link
cargo run -q -- status --port /dev/ttyUSB0
cargo run -q -- status --url tcp://192.168.1.50

# Emergency stop: feed hold + soft reset, laser off
cargo run -q -- stop --url tcp://192.168.1.50

# Clear a GRBL alarm after a failed job
cargo run -q -- unlock --url tcp://192.168.1.50
```

Installed as a binary, the commands are `orleans gui`, `orleans info`,
`orleans gcode`, `orleans run`, `orleans status`, `orleans stop` and
`orleans unlock`. `--url` accepts
`tcp://host[:port]`, `telnet://host[:port]` or a bare `host[:port]`.

The GUI is behind the `gui` Cargo feature, which is on by default. Build with
`--no-default-features` for a headless binary without the windowing stack.

## GUI

`orleans gui [design.svg|design.orleans]` opens the editor:

* wheel to zoom at the cursor, drag to pan, `F` or double-click to fit;
* every SVG colour becomes a layer. The left panel is a compact, scrollable
  layer list; select one layer to edit its name, colour, speed, power and
  passes. **Cut** decides whether the layer is planned, framed and sent;
  **Show** controls whether it is drawn. A visible layer with Cut off remains
  as a grey reference, while a hidden layer with Cut on still cuts. Move layers
  in cut order, and the selected layer is highlighted on the canvas;
* choose **Line** or **Fill** for each layer. Fill mode adds **Fill spacing**,
  **Angle**, **Overlap** and **Alternate**; it hatches closed contours and
  previews the fill. Open contours are ignored by fill mode. Line mode adds
  **Tabs**: count and width of uncut holding bridges per contour, with an
  optional **Holes** setting for inner contours;
* kerf, `max S` (`$30`), travel optimisation and `M3`/`M4` live in the Job panel
  (GUI jobs default to M4);
* **Export G-code…** writes a `.gcode` file of your choosing;
* `Ctrl+O` opens, `Ctrl+E` exports, and dropping an SVG onto the window loads it;
* the **Machine** panel connects over Telnet, shows the live GRBL state,
  position, feed and `S` value, and offers **Home ($H)**, **Unlock ($X)** and
  **Stop** (feed hold + soft reset). Homing asks for confirmation and is only
  enabled when the controller is idle or in alarm, with the laser off;
* long status silences (for example while a controller homes) do not drop the
  link, and if the WiFi bridge closes the socket the GUI retries and reports
  **Reconnecting…** instead of going offline. On connect the GUI reads `$$`
  and shows `$32` (laser mode), `$30` (max S) and `$22` (homing), warning when
  `$32` is off or `$30` differs from **Max S**;
* jog the head with the X/Y pad, an adjustable step and feed, and a jog-cancel
  button; **Set zero (G92)** is a temporary work origin and **Save zero (G54)**
  stores it with `G10 L20 P1 X0 Y0`. Jogging needs GRBL 1.1 (`$J=`); generated
  jobs select G54. The position readout shows `WPos` when the controller
  reports it, otherwise `MPos`;
* **Save project…** (`Ctrl+S`) writes a self-contained `.orleans` JSON file
  with the SVG, layer parameters, job settings, per-shape transforms and
  node-edited paths.
  **Open…** accepts both SVG and `.orleans`; **Reload** reloads whichever is
  current;
* **Settings…** opens the controller settings: laser mode `$32`, max S `$30`,
  homing `$22`, and the raw `$$` list. **Apply** writes the changed settings
  to EEPROM; editing is blocked while cutting or homing;
* the bed/travel area from `$130`/`$131` is drawn on the canvas, offset by
  `WCO` when the controller reports it, and included in **Fit**. Toggle it
  with **Bed**;
* click a shape on the canvas to select it; Ctrl/Shift-click adds or removes
  shapes. Selected shapes are yellow with their bounds and a group box. The
  live **Size** row shows the selection's bounding-box width and height. Drag
  any selected shape to move the whole selection. Drag a corner handle to
  scale uniformly around the opposite corner, or drag the circular handle above
  the box to rotate around the selection centre. The **Selection** panel still
  offers numeric Move, uniform Scale and Rotate. The **Position** row places a
  chosen reference point (corner, edge or centre) at exact X/Y coordinates
  relative to the job origin `(0, 0)`; **Move to** applies it. With one shape
  selected, **Edit nodes** shows its anchors and Bézier controls. Drag them to
  reshape
  the path; double-click a segment to insert an anchor; select an anchor and
  press `Delete` or **Delete node** to remove it; enable **Snap** to snap node
  drags to the canvas grid. Use **Done** or `Esc` to leave node mode.
  **Undo**/`Ctrl+Z` reverses the last transform, node drag, insertion or
  deletion; right-drag pans;
* **Send to laser** streams the current job with a progress bar, Pause/Resume
  and a per-job Stop button (feed hold + soft reset). Sending is only enabled
  when the controller is connected, idle and the laser is off; `Ctrl+Enter`
  opens the confirmation and `Enter` confirms it. GUI jobs default to **M4
  dynamic power**; Pause is offered only when the controller reports `$32=1`,
  because a feed hold at zero velocity then switches the laser off. If you
  switch a job to M3, Pause is disabled and you must use Stop. A framing pass
  can always be paused because it holds `M5`;
* **Frame** traces the job bounds at 3000 mm/min with the laser off, so you can
  check placement before committing to a cut.

The canvas shows the design geometry; kerf compensation is applied to the
toolpath, so it affects the exported G-code and the statistics rather than the
outline drawing, as in LightBurn. Sending a job uses the current layer settings
and starts from the machine's current work origin, so set that origin before
pressing **Send to laser**. **Pause** uses feed hold and **Resume** cycle start. It is enabled only when
the job was generated with **M4** and the controller reports `$32=1`; then the
velocity-scaled laser is off at standstill. With `M3` the Pause button is
disabled (a plain feed hold could leave the laser burning in place); **Stop**
remains available. Framing can always be paused because it holds `M5`.

## Architecture

```
crates/
  orleans-core/      document model: Document → Layer → Shape, CutParams, Job
  orleans-geom/      flattening, area, containment, Clipper2 offsets/booleans
  orleans-import/    SVG → Document (via usvg)
  orleans-toolpath/  Document → Job: nesting, ordering, passes, time estimate
  orleans-driver/    Job → G-code, Transport trait, serial/TCP, GRBL client, simulator
  orleans-cli/       the `orleans` binary
```

The layers only depend downwards, and the only thing that knows about a
specific machine is `orleans-driver`. Adding Marlin only means adding a
`GcodeOptions` variant; adding Ruida means implementing `Transport` plus a
binary protocol streamer, with nothing above it changing.

## Troubleshooting WiFi

`orleans status --url tcp://IP` should print a status line such as
`<Idle|MPos:0.000,0.000,0.000|FS:0,0>`. If the TCP connection opens but the
controller stays silent, orleans retries the request, also sends a
newline-terminated query (some bridges are line-buffered), then probes `$I` and
`M115` to identify the firmware and prints everything it received.

Common causes:

* **The laser controller is off or asleep** while its WiFi bridge is still
  powered. The bridge accepts TCP; the GRBL board behind it is not listening.
* **Something else on port 23.** Check `nc -vz <ip> 23` and the router's client
  list.
* **Marlin/ESP3D, not GRBL.** Orleans only speaks GRBL for now; the probe will
  say so.
* **WebSocket-only bridge on port 81.** Not implemented yet.

`orleans run` refuses to stream when no status comes back; `--force` overrides
that.

If a job aborts while the link is still alive, `orleans` sends feed hold
followed by a soft reset, which stops the planner and switches the laser off. If
the link itself died, stop the machine at the panel. Check the machine and
re-home or re-zero before retrying; a soft reset returns GRBL to its startup
state.

`--timeout` (default 30 s) is a **no-response** timeout: the job is aborted only
if no `ok`, status or other line arrives for that long. It is not a limit on the
total job duration — a 20-minute cut with steady acknowledgements is fine. Raise
it for a controller that can stay silent for longer while executing one very
slow move.

## Design decisions

**Millimetres, Y up, origin bottom-left.** Machine convention, not SVG's
top-left/Y-down. `orleans-import` flips Y using the document height, so 0,0 is the
bottom-left of the bed.

**Kerf is applied towards the material.** The planner computes each contour's
nesting depth (even = outer, odd = hole) and offsets by half the kerf: outer
contours shrink, holes grow, so the finished part matches the drawing. This is
why inside-out ordering matters — the part must still be supported when its
outer contour is cut last.

**Character-counting flow control.** GRBL's receive buffer is 128 bytes and it
acks one `ok` per line. `GrblClient::run` keeps at most 120 unacknowledged
bytes in flight, which is what makes streaming reliable at 115200 baud instead
of dropping commands.

**Realtime commands are separate.** `?`, `!`, `~` and Ctrl-X are single bytes
handled by the controller even while its buffer is full. `feed_hold`, `resume`
and `soft_reset` therefore do not go through the line queue.

**WiFi is Telnet, not HTTP.** Grbl_ESP32, FluidNC and ESP3D expose the raw GRBL
stream on TCP port 23. `TcpTransport` keeps its own line buffer, handles partial
reads and strips Telnet IAC negotiation bytes, because a socket read has no
concept of a line and some bridges emit negotiation on connect.

## Known limitations

* WiFi support is Telnet only: no WebSocket/HTTP transport, TLS or mDNS
  discovery yet.
* Ordering is per layer. If the outer contour is on a different layer from its
  holes, layers are cut in list order — reorder layers, as in LightBurn.
* Curves are flattened at import/plan time (default 0.02 mm), so G-code grows
  with detail. Arcs (`G2`/`G3`) are not emitted.
* No acceleration model in the time estimate; it is distance / feed rate.
* Text in SVG needs a font database wired into `usvg::Options`; without it,
  text elements are dropped.
* The simulator is protocol-level, not a motion model.
* The bed rectangle uses `$130`/`$131`; when GRBL does not report `WCO`, it
  assumes work zero coincides with machine zero.
* Project layer settings, Show/Cut state and cut order are matched by layer
  index; shape transforms and node-edited paths are matched by layer index
  plus shape id. If the SVG's colour grouping changes after saving, check that
  the settings landed on the right layers.
* Interactive handles scale uniformly around the opposite corner and rotate
  around the selection centre. There is no non-uniform/skewed scaling.
* Node editing supports moving anchors and Bézier controls, inserting anchors
  on segments, deleting anchors, and optional grid snapping while dragging.
  Deleting an anchor reconnects the remaining segments directly; there is no
  join/break or smoothing, and nodes do not snap to other geometry yet. Undo
  covers transforms and node edits.
* Fill mode emits hatch strokes only, not the shape outline; put the outline on
  a separate Line layer/colour if you need both. It supports scanline hatching
  of closed contours with even-odd holes; cross-hatch and offset fill are not
  implemented yet.
* Tabs are distributed evenly by arc length and start at the contour seam; there
  is no manual tab placement yet.
* Pause is restricted to M4 jobs with `$32=1`; M3 jobs can only be stopped.
  Framing uses a fixed 3000 mm/min and the design bounds, with no margin
  setting.
* **Set zero (G92)** is temporary: a soft reset, Stop or controller reconnect
  after a reset clears it. **Save zero (G54)** writes `G10 L20 P1`; whether the
  offset survives a reset depends on the controller. Generated jobs select G54.
* Large designs are redrawn as polylines every frame; a spatial index and
  level-of-detail rendering will be needed for very complex files.

## Roadmap

1. **Finish the editor** — job history for the streaming panel, plus
   node-editing refinements such as join/break and snapping to geometry.
2. **Fill refinements and manual tabs** — cross-hatch, offset fill and
   manual tab placement.
3. **Raster engraving** — threshold/dither (Floyd–Steinberg, Stucki, Jarvis),
   PWM mapping, run-length output.
4. **More importers** — DXF, PDF, AI; LBRN2 read/write from community specs.
5. **Second driver** — Marlin, then Ruida over UDP.
6. **Job control extras** — feed/power overrides, rotary setup.

## License

MIT OR Apache-2.0.
