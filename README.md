# orleans

An open, clean-room CAM stack for laser cutters, written in Rust.

This is **not** a fork of LightBurn and contains no LightBurn code, assets or
proprietary file formats. It reimplements the same workflow — vector design in,
G-code out, streamed to a controller over USB or WiFi — from public
specifications: SVG, PDF, DXF, G-code and the GRBL protocol.

The commercial binary in `../LightBurn` was only used to inventory which device
families exist (GRBL, Marlin, Smoothieware, Ruida, TopWisdom, galvo). Nothing
was decompiled or copied.

---

## What works today

| Capability | Status |
|---|---|
| SVG import (shapes→paths, transforms, CSS, physical units) | ✅ |
| Image import (PNG/JPEG/BMP) | ✅ |
| Colours → layers, per-layer speed/power/passes | ✅ |
| Kerf compensation via Clipper2 (outer shrinks, holes grow) | ✅ |
| Boolean ops (union / difference / intersection) | ✅ |
| Inside-out ordering + nearest-neighbour travel optimisation | ✅ |
| Multi-pass cutting | ✅ |
| Fill/hatch layers (angle, spacing, overlap, alternate, cross-hatch, offset, outline) | ✅ |
| G-code for GRBL / GrblHAL / FluidNC / Marlin-laser (`M3`, `M4`) | ✅ |
| GRBL streaming: character-counting flow control, `ok`/`error`, progress, status, feed hold, resume, soft reset, automatic stop on failure | ✅ |
| USB serial and WiFi TCP/Telnet (Grbl_ESP32, FluidNC, ESP3D) | ✅ |
| Hardware-free GRBL simulator | ✅ |
| DXF import (common 2D entities) | ✅ |
| PDF / AI import (PDF vector paths, first page) | ✅ |
| Raster engraving (threshold, Floyd–Steinberg/Atkinson/Bayer dither, grayscale, crop) | ✅ |
| Tabs/bridges on contour cuts, automatic or manual, with canvas markers | ✅ |
| GUI viewer: layers, speed/power/passes, live stats, G-code export | ✅ |
| Layer list: selection, Show/Cut, rename, colour, cut-order reorder | ✅ |
| New layers; move/copy/delete objects between layers; delete empty layers | ✅ |
| Duplicate selected objects and rectangular arrays | ✅ |
| Paste SVG objects from Inkscape via the system clipboard | ✅ |
| Canvas: selection, drag-move, handles, live size/position, undo/redo | ✅ |
| Shape groups and align/distribute | ✅ |
| Dark or light interface and canvas theme | ✅ |
| Project save/load (`.orleans`, canonical layer/shape snapshot) | ✅ |
| Import SVG/PDF/DXF/images into the current design (hybrid jobs) | ✅ |
| Machine bed/travel from `$130`/`$131` drawn on the canvas | ✅ |
| GUI machine control: connect, status, home, unlock, stop | ✅ |
| GUI job streaming: progress, stop, automatic idle wait | ✅ |
| Persistent job history (cut/frame outcome, duration, stats) | ✅ |
| GUI laser-off framing pass | ✅ |
| GUI jog (X/Y) and work zero: temporary `G92` or stored `G54` | ✅ |
| Controller settings editor (`$32`, `$30`, `$22`, raw `$$`) | ✅ |
| GUI pause/resume (M4 + `$32=1`; framing always) | ✅ |
| Node editing: drag, insert, delete, break/join, grid and geometry snap | ✅ |
| Ruida, TopWisdom, galvo/fiber, rotary | ❌ |
| LBRN2 project files | ❌ |

153 unit and integration tests, including a full SVG → toolpath → G-code → TCP
controller run against a local GRBL server.

## Quick start

Requires a recent stable Rust toolchain and a C++ compiler (Clipper2 is built
from source).

```bash
cargo build --release

# Open the visual editor (the input is optional)
cargo run -q -- gui examples/coaster.svg --url tcp://192.168.1.10

# Open a DXF drawing
cargo run -q -- gui drawing.dxf

# Open a PDF or a PDF-compatible Illustrator file
cargo run -q -- gui drawing.pdf

# Open a PNG/JPEG/BMP as a raster-engraving layer
cargo run -q -- gui photo.png

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
* **Light** in the toolbar switches the interface and canvas to a light theme.
  The canvas background, grid, page, bed, travel, selection handles and node
  markers change with it. The choice applies immediately and lasts for the
  session;
* every SVG colour becomes a layer. The left panel is a compact, scrollable
  layer list; select one layer to edit its name, colour, speed, power and
  passes. **Cut** decides whether the layer is planned, framed and sent;
  **Show** controls whether it is drawn. A visible layer with Cut off remains
  as a grey reference, while a hidden layer with Cut on still cuts. Move layers
  in cut order, and the selected layer is highlighted on the canvas. **+**
  beside the heading creates an empty layer and chooses it as the move/copy
  destination. With objects selected, pick any layer in the **To** row and
  press **Move** or **Copy**; transfers are undoable. Groups move with their
  members, copied grouped objects get their own group, and copies are ordinary
  shapes that follow the destination layer's cut settings. Raster layers cannot
  be destinations. **Delete** in the Selection panel or the `Delete` key removes
  the selected objects; an empty selected layer has its own **Delete** button
  next to **Up**/**Down**. **Duplicate** (`Ctrl+D`) copies the selection in
  place and selects the copies. The **Array** rows keep the original and create
  a grid of copies from column/row counts and `dx`/`dy` spacing; groups are
  duplicated as groups. `Ctrl+V` pastes SVG at the canvas pointer, including
  objects copied from Inkscape. `Ctrl+C` and `Ctrl+X` place selected objects on
  the system clipboard as SVG text; pasting that back into orleans recreates
  them, while applications that expect a vector MIME type may receive only the
  SVG source as text. Clipboard round-trips preserve paths and colours but not
  editor groups or manual tabs; outline text in Inkscape before copying;
* choose **Line** or **Fill** for each layer. Fill mode adds **Fill spacing**,
  **Angle**, **Overlap**, **Alternate**, **Cross** and **Outline**; it hatches
  closed contours and previews the fill. **Cross** adds a second pass at 90°,
  **Offset** fills inward with concentric closed loops instead of scan lines,
  and **Outline** traces the closed contours after filling them. Open contours
  are ignored by fill mode. Line mode adds
  **Tabs**: count and width of uncut holding bridges per contour, with an
  optional **Holes** setting for inner contours. Enable **Manual**, press
  **Place tabs**, then click any closed contour to put a tab there; click a
  tab marker again to remove it. `Esc` or **Done** leaves tab mode. The
  selected layer draws each tab span in cyan on the canvas, for both automatic
  and manual tabs;
* **Open…** also accepts PNG/JPEG/BMP images. An image opens as a raster layer
  with threshold, Floyd–Steinberg, Atkinson or 4×4 Bayer dithering, or
  grayscale power levels, plus line interval, angle, invert, alternate rows,
  position, physical size and DPI. The 96 DPI import size is only a starting
  point; size and DPI can be changed after import. **Crop** X/Y/W/H selects a
  pixel region to engrave; **Full** restores the whole image, while position,
  size and rotation stay fixed. **Open…** replaces the current design; use
  **Import…** to add the image as a new layer instead. The raster is shown as a
  translucent image with engraving scanlines over it. Click
  an image on the canvas or select its layer to show handles: drag inside to
  move, drag a corner to resize and drag the top handle to rotate. Use a
  separate vector layer for outlines;
* PDF files open or import as vector paths, grouped by stroke colour (or fill
  colour for fill-only paths). PDF-compatible `.ai` files work too. The first
  page is imported. Text must be converted to outlines, and raster images,
  clipping paths, gradients and patterns are not imported;
* DXF files open or import as one layer per DXF layer, using the layer's ACI or
  true colour. `LINE`, `LWPOLYLINE`/`POLYLINE` (including bulges), `CIRCLE`,
  `ARC`, `ELLIPSE` and `SPLINE` are read; `$INSUNITS` sets the millimetre
  scale. Blocks/`INSERT`, `HATCH`, text, dimensions and binary DXF are not
  expanded;
* **Import…** adds SVG, DXF or raster layers to the current document instead of
  replacing it. Imported layers are appended at the end of the cut order with
  unique names, and the action is undoable. This is how hybrid vector/raster
  jobs are built: an imported image keeps its own engraving settings while
  vector layers cut around it. Imported vector geometry is stored in the
  project as self-contained paths, so hybrid and DXF-imported projects reload
  correctly;
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
  containing the current document: page size, job settings, every layer and
  shape path, groups, fill/tab settings and PNG-compressed raster images. It is
  a canonical snapshot (format 2), not an SVG plus edit history, so loading
  never depends on re-importing a source file. **Open…** accepts SVG, DXF,
  images and `.orleans`; **Reload** reloads whichever is current;
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
  press `Delete` or **Delete node** to remove it. Ctrl/Shift-click selects
  several anchors: **Break** opens a closed contour at the selected anchor or
  splits an open path there, and **Join** connects two selected endpoint
  anchors, closing the contour when both belong to the same open path. Enable
  **Snap** to snap node drags to the canvas grid and **Geometry** to snap them
  to anchors and segments of other visible shapes. Use **Done** or `Esc` to
  leave node mode.
  **Group** makes the selection one object: clicking any member selects the
  whole group, and drags, handles and align/distribute move it as a unit.
  **Ungroup** releases its members. With two or more objects selected, the
  **Align** row aligns left/centre/right and top/middle/bottom; with
  three or more, **Distribute** spaces their centres evenly horizontally or
  vertically. Groups can span layers; ungroup before node-editing a member.
  **Undo**/`Ctrl+Z` reverses the last transform, node edit, raster move,
  group change or layer/job setting change;
  `Ctrl+Shift+Z` (or `Ctrl+Y`) redoes it. Right-drag pans;
* **Send to laser** streams the current job with a progress bar, Pause/Resume
  and a per-job Stop button (feed hold + soft reset). Sending is only enabled
  when the controller is connected, idle and the laser is off; `Ctrl+Enter`
  opens the confirmation and `Enter` confirms it. GUI jobs default to **M4
  dynamic power**; Pause is offered only when the controller reports `$32=1`,
  because a feed hold at zero velocity then switches the laser off. If you
  switch a job to M3, Pause is disabled and you must use Stop. A framing pass
  can always be paused because it holds `M5`;
* completed, stopped and failed cut/framing runs are added to **Job history**
  in the streaming panel with time, outcome, duration, cut length and line
  count. History is persisted at
  `$XDG_DATA_HOME/orleans/job-history.json` (or
  `~/.local/share/orleans/job-history.json`) and can be cleared from the panel;
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
  orleans-import/    SVG → Document (via usvg), ASCII DXF, raster images
  orleans-toolpath/  Document → Job: nesting, ordering, passes, time estimate
  orleans-driver/    Job → G-code, Transport trait, serial/TCP, GRBL client, simulator
  orleans-cli/       the `orleans` binary
```

The layers only depend downwards, and the only thing that knows about a
specific machine is `orleans-driver`. Adding Marlin only means adding a
`GcodeOptions` variant; adding Ruida means implementing `Transport` plus a
binary protocol streamer, with nothing above it changing.

## Project files

`.orleans` is a self-contained JSON snapshot of the editor document. The
current format is **format 2**; there is intentionally no v1 compatibility
because v1 was never released. The file must be saved and loaded as one atomic
document, not treated as an SVG plus a list of edits.

At the top level:

```json
{
  "format": 2,
  "source": "drawing.svg",
  "width_mm": 300.0,
  "height_mm": 200.0,
  "kerf": 0.15,
  "optimize": true,
  "laser_mode": "M4",
  "max_s": 1000,
  "layers": [],
  "groups": []
}
```

`layers` is in cut order; `groups` is the editor group list. Each layer stores
its complete state:

```json
{
  "index": 0,
  "name": "Cut",
  "color": [255, 0, 0],
  "mode": "Line",
  "visible": true,
  "output": true,
  "cut": { "speed": 1200.0, "power": 35.0, "passes": 1 },
  "fill": {
    "spacing": 0.25, "angle": 0.0, "overlap": 0.0,
    "alternate": true, "cross": false, "outline": false, "offset": false
  },
  "tabs": { "count": 2, "width": 3.0, "inner": false, "manual": false },
  "manual_tabs": [
    { "shape": "s1", "contour": 0, "positions": [0.25, 0.75] }
  ],
  "shapes": [
    { "id": "s1", "path": "M0,0 L10,0 L10,10 Z", "is_fill": false }
  ],
  "raster": null
}
```

- `layers` order is the cut order; each layer keeps its `index` for stable
  identity.
- Shapes store their **final** geometry as SVG path data. Transforms, node
  edits, imported layers and moved/copied objects are already baked in.
- Coordinates are millimetres with Y up, matching the machine model.
- `groups[].members` reference a layer index plus shape id. Manual tab
  positions reference the same pair, with positions expressed as a fraction of
  the flattened contour perimeter.
- `source` is provenance for display only. Loading never reads it.
- Raster layers use:
  ```json
  {
    "width": 4000, "height": 3000,
    "png": "iVBORw0KGgo...",
    "x": 10.0, "y": 20.0, "width_mm": 100.0, "height_mm": 75.0,
    "angle": 0.0, "mode": "Threshold", "threshold": 128,
    "invert": false, "line_interval": 0.1, "bidirectional": true,
    "crop": { "x": 0, "y": 0, "width": 4000, "height": 3000 }
  }
  ```
  `png` is base64-encoded 8-bit grayscale PNG data; `crop` is optional.
- Loading is direct deserialization into `Document`. A project therefore does
  not depend on the original SVG/DXF or on importer output being deterministic.
- Unknown top-level fields should be ignored by future readers; incompatible
  changes must bump `format`.

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
* `.orleans` files use format 2 and are not backward-compatible with pre-release
  v1 files. Projects store full geometry, so they are larger than the original
  SVG but no longer depend on re-import or index matching.
* Interactive handles scale uniformly around the opposite corner and rotate
  around the selection centre. There is no non-uniform/skewed scaling.
* PDF import reads the first page's vector paths and form XObjects only. Text
  must be outlined; raster images, clip paths, gradients, patterns,
  transparency groups, annotations and later pages are skipped. Illustrator
  files must be saved with PDF compatibility. Imported paths are not linked to
  their source file.
* DXF import covers ASCII files and the listed entities only. Blocks,
  `INSERT`, `HATCH`, text, dimensions, meshes and binary DXF are skipped.
  Imported vector layers are self-contained: they are not linked to their
  source file, and later source edits are not tracked.
* Only empty vector layers can be deleted. Delete all their objects first; a
  raster layer disappears only when the design is replaced.
* Arrays are rectangular only: there is no polar/circular array, and copies
  follow fixed `dx`/`dy` spacing. Duplicate places copies exactly on top of the
  originals, ready to be dragged away.
* Groups contain vector shapes only and cannot be nested; grouping a selection
  that already touches groups merges them. Ungroup before node-editing a
  member. Align/distribute treat each group as one object, and distribution
  equalises object centres rather than edge gaps.
* Node editing supports moving anchors and Bézier controls, inserting anchors
  on segments, deleting anchors, breaking a contour at an anchor and joining
  two endpoint anchors, with optional grid and geometry snapping while
  dragging. Breaking a closed contour leaves the same anchor at both open ends;
  joining different contours inserts a straight line when the endpoints differ.
  There is no smoothing, node-type conversion or joining to an interior point.
  Undo and redo cover transforms, node edits, raster placement, groups, layer
  settings and job settings.
* Fill mode supports scanline hatching or inward concentric offset loops on
  closed contours with even-odd holes, an optional 90° cross-hatch pass and an
  optional outline cut. Offset fill loops stop when the remaining region is
  narrower than the spacing, and fill outlines do not use holding tabs.
* Raster import starts at 96 DPI but size and DPI can be changed after import;
  scan lines rotate with the image angle. Cropping is numeric and
  non-destructive: the full image stays visible with a cyan crop outline, while
  only the cropped region is engraved. Raster layers with Cut disabled show the
  image preview but not the engraving scan lines; raster selection handles are
  available on the canvas.
* Automatic tabs are distributed evenly by arc length from the contour seam.
  Manual tabs are stored as fractions of the flattened contour perimeter, so a
  heavy node edit that reorders or removes contours can detach a tab. Fill
  outlines never use tabs.
* Pause is restricted to M4 jobs with `$32=1`; M3 jobs can only be stopped.
  Framing uses a fixed 3000 mm/min and the design bounds, with no margin
  setting.
* **Set zero (G92)** is temporary: a soft reset, Stop or controller reconnect
  after a reset clears it. **Save zero (G54)** writes `G10 L20 P1`; whether the
  offset survives a reset depends on the controller. Generated jobs select G54.
* Large designs are redrawn as polylines every frame; a spatial index and
  level-of-detail rendering will be needed for very complex files.

## Roadmap

1. **More importers** — PDF page selection, HPGL, DXF blocks/text.
2. **Second driver** — Marlin, then Ruida over UDP.
3. **Job control extras** — feed/power overrides, rotary setup.

## License

MIT OR Apache-2.0.
