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
| G-code for GRBL / GrblHAL / FluidNC / Marlin-laser (`M3`, `M4`) | ✅ |
| GRBL streaming: character-counting flow control, `ok`/`error`, progress, status, feed hold, resume, soft reset, automatic stop on failure | ✅ |
| USB serial and WiFi TCP/Telnet (Grbl_ESP32, FluidNC, ESP3D) | ✅ |
| Hardware-free GRBL simulator | ✅ |
| DXF / PDF / AI import | ❌ |
| Raster engraving, dithering, fill/hatch | ❌ |
| GUI viewer: layers, speed/power/passes, live stats, G-code export | ✅ |
| GUI machine control: connect, status, home, unlock, stop | ✅ |
| GUI job streaming: progress, stop, automatic idle wait | ✅ |
| GUI pause/resume | ❌ |
| Selection, transforms, node editing | ❌ |
| Ruida, TopWisdom, galvo/fiber, rotary | ❌ |
| LBRN2 project files | ❌ |

60 unit and integration tests, including a full SVG → toolpath → G-code → TCP
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

`orleans gui [design.svg]` opens the editor:

* wheel to zoom at the cursor, drag to pan, `F` or double-click to fit;
* every SVG colour becomes a layer; edit speed, power and passes in the left
  panel and the job statistics update immediately;
* kerf, `max S` (`$30`), travel optimisation and `M3`/`M4` live in the Job panel;
* **Export G-code…** writes a `.gcode` file of your choosing;
* `Ctrl+O` opens, `Ctrl+E` exports, and dropping an SVG onto the window loads it;
* the **Machine** panel connects over Telnet, shows the live GRBL state,
  position, feed and `S` value, and offers **Home ($H)**, **Unlock ($X)** and
  **Stop** (feed hold + soft reset). Homing asks for confirmation and is only
  enabled when the controller is idle or in alarm, with the laser off;
* long status silences (for example while a controller homes) do not drop the
  link, and if the WiFi bridge closes the socket the GUI retries and reports
  **Reconnecting…** instead of going offline;
* **Send to laser** streams the current job with a progress bar and a per-job
  Stop button (feed hold + soft reset). It is only enabled when the controller
  is connected, idle and the laser is off; `Ctrl+Enter` opens the confirmation,
  and `Enter` confirms it.

The canvas shows the design geometry; kerf compensation is applied to the
toolpath, so it affects the exported G-code and the statistics rather than the
outline drawing, as in LightBurn. Sending a job uses the current layer settings
and starts from the machine's current work origin, so set that origin before
pressing **Send to laser**. There is no pause/resume yet; **Stop** aborts the
job, resets the controller and switches the laser off.

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
* The GUI has no selection, transforms or node editing, and no pause/resume
  yet: stopping a job aborts it rather than pausing.
* Large designs are redrawn as polylines every frame; a spatial index and
  level-of-detail rendering will be needed for very complex files.

## Roadmap

1. **Finish the editor** — selection, transforms, node editing, undo, then
   job history and pause/resume for the streaming panel.
2. **Hatch fills and tabs** — Clipper2 booleans are wired and ready for them.
3. **Raster engraving** — threshold/dither (Floyd–Steinberg, Stucki, Jarvis),
   PWM mapping, run-length output.
4. **More importers** — DXF, PDF, AI; LBRN2 read/write from community specs.
5. **Second driver** — Marlin, then Ruida over UDP.
6. **Job control extras** — feed/power overrides, pause/resume, framing and
   laser-off preview, origin and rotary setup.

## License

MIT OR Apache-2.0.
