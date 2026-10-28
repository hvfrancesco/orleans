# lightburn-rs

An open, clean-room CAM stack for laser cutters, written in Rust.

This is **not** a fork of LightBurn and contains no LightBurn code, assets or
proprietary file formats. It reimplements the same workflow — vector design in,
G-code out, streamed to a controller — from public specifications: SVG, G-code
and the GRBL protocol.

The commercial binary in `../LightBurn` was only used to inventory which device
families exist (GRBL, Marlin, Smoothieware, Ruida, TopWisdom, galvo). Nothing
was decompiled or copied.

---

## What works today

| Capability | Status |
|---|---|
| SVG import (shapes→paths, transforms, CSS, physical units) | ✅ |
| Colours → layers, per-layer speed/power/passes | ✅ |
| Kerf compensation (outer shrinks, holes grow) | ✅ |
| Inside-out ordering + nearest-neighbour travel optimisation | ✅ |
| Multi-pass cutting | ✅ |
| G-code for GRBL / GrblHAL / FluidNC / Marlin-laser (`M3`, `M4`) | ✅ |
| GRBL streaming: character-counting flow control, `ok`/`error`, status, feed hold, resume, soft reset | ✅ |
| Hardware-free GRBL simulator | ✅ |
| DXF / PDF / AI import | ❌ |
| Raster engraving, dithering, fill/hatch | ❌ |
| GUI canvas / node editing | ❌ |
| Ruida, TopWisdom, galvo/fiber, rotary | ❌ |
| LBRN2 project files | ❌ |

31 unit and integration tests, including a full SVG → toolpath → G-code →
simulated controller run.

## Quick start

```bash
cargo build --release

# Inspect a design: size, layers, cut length, time estimate
cargo run -q -- info examples/coaster.svg

# Generate G-code
cargo run -q -- gcode examples/coaster.svg -o coaster.gcode \
    --speed 1200 --power 35 --kerf 0.15 --passes 2

# Dry-run the whole pipeline against the built-in simulator
cargo run -q -- run examples/coaster.svg --mock

# Cut for real (GRBL defaults to 115200 baud)
cargo run -q -- run examples/coaster.svg --port /dev/ttyUSB0 \
    --speed 1200 --power 35

# Query a connected controller
cargo run -q -- status --port /dev/ttyUSB0
```

Installed as a binary, the commands are `lb info`, `lb gcode`, `lb run`,
`lb status`.

## Architecture

```
crates/
  lb-core/      document model: Document → Layer → Shape, CutParams, Job
  lb-geom/      flattening, area, containment, kerf/miter offset
  lb-import/    SVG → Document (via usvg)
  lb-toolpath/  Document → Job: nesting, ordering, passes, time estimate
  lb-driver/    Job → G-code, Transport trait, GRBL client, simulator
  lb-cli/       the `lb` binary
```

The layers only depend downwards, and the only thing that knows about a
specific machine is `lb-driver`. Adding Marlin only means adding a
`GcodeOptions` variant; adding Ruida means implementing `Transport` plus a
binary protocol streamer, with nothing above it changing.

## Design decisions

**Millimetres, Y up, origin bottom-left.** Machine convention, not SVG's
top-left/Y-down. `lb-import` flips Y using the document height, so 0,0 is the
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

## Known limitations

* The kerf offset is a miter offset. Strongly concave contours can
  self-intersect. Replace with a clipper-style offset before doing production
  work on organic shapes.
* Ordering is per layer. If the outer contour is on a different layer from its
  holes, layers are cut in list order — reorder layers, as in LightBurn.
* Curves are flattened at import/plan time (default 0.02 mm), so G-code grows
  with detail. Arcs (`G2`/`G3`) are not emitted.
* No acceleration model in the time estimate; it is distance / feed rate.
* Text in SVG needs a font database wired into `usvg::Options`; without it,
  text elements are dropped.
* The simulator is protocol-level, not a motion model.

## Roadmap

1. **Editing canvas** — `egui`/`wgpu`, selection, transforms, node editing, undo.
2. **Robust geometry** — `clipper2` for offsets/booleans, hatch fills, tabs.
3. **Raster engraving** — threshold/dither (Floyd–Steinberg, Stucki, Jarvis),
   PWM mapping, run-length output.
4. **More importers** — DXF, PDF, AI; LBRN2 read/write from community specs.
5. **Second driver** — Marlin, then Ruida over UDP.
6. **Job control UI** — progress, overrides, pause/resume, framing/laser-off
   preview, origin and rotary setup.

## License

MIT OR Apache-2.0.
