# Development

## Architecture

```
crates/
  orleans-core/      document model: Document → Layer → Shape, CutParams, Job
  orleans-geom/      flattening, area, containment, Clipper2 offsets/booleans
  orleans-import/    SVG/PDF/DXF → Document, system-font text, raster images
  orleans-toolpath/  Document → Job: nesting, ordering, passes, time estimate
  orleans-driver/    Job → G-code, Transport trait, serial/TCP, GRBL/Marlin clients, simulators
  orleans-cli/       the `orleans` binary
```

The layers depend only downwards. The only crate that knows about a specific
machine is `orleans-driver`. Adding a G-code dialect means adding a
`GcodeDialect` variant; a different line-oriented controller means implementing
a client behind the `Controller` enum and a `Transport`, with nothing above it
changing. The GUI and CLI use the same auto-detection path. The GUI worker can
open both `TcpTransport` and `SerialTransport`, and works only with the
boxed `Transport` trait.

## Testing

Run the full local checks before committing:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo build --release
```

The suite contains unit and integration coverage for import, geometry,
toolpaths, G-code, transport and the GUI model. Integration tests run the full
SVG → toolpath → G-code → TCP controller path against a local GRBL server and a
local Marlin server, and the driver tests cover both simulators.

## Design decisions

### Millimetres, Y up, origin bottom-left

Machine convention, not SVG's top-left/Y-down convention. `orleans-import` flips
Y using the document height, so `(0, 0)` is the bottom-left of the bed.

### Kerf is applied towards the material

The planner computes each contour's nesting depth (even = outer, odd = hole) and
offsets by half the kerf: outer contours shrink, holes grow, so the finished part
matches the drawing. This is why inside-out ordering matters: the part must still
be supported when its outer contour is cut last.

### Character-counting flow control

GRBL's receive buffer is 128 bytes and it acknowledges one `ok` per line.
`GrblClient::run` keeps at most 120 unacknowledged bytes in flight, which makes
streaming reliable at 115200 baud instead of dropping commands.

### Realtime commands are separate

`?`, `!`, `~` and Ctrl-X are single bytes handled by the controller even while
its buffer is full. `feed_hold`, `resume` and `soft_reset` therefore do not go
through the line queue.

### Marlin is line-acknowledged

Marlin has no realtime `?` status report. `MarlinClient` streams with the same
byte window as GRBL, treats `ok` as planner acceptance, and sends `M400` after
the last line so a job is not reported as finished while moves are still queued.
Status is assembled from `M114` position and `M105` temperature replies and
converted to the GRBL `<...>` shape for the editor. `M112` is the emergency stop;
`M999` restarts a halted controller.

### WiFi is Telnet, not HTTP

Grbl_ESP32, FluidNC and ESP3D expose the raw GRBL stream on TCP port 23.
`TcpTransport` keeps its own line buffer, handles partial reads and strips Telnet
IAC negotiation bytes, because a socket read has no concept of a line and some
bridges emit negotiation on connect.

### The project file is a canonical document

`.orleans` stores final geometry, layer settings, groups, manual tabs and raster
data. It is not an SVG plus an edit history. Loading therefore never depends on
re-importing or matching source indices. See
[project-format.md](project-format.md).

## Known limitations

### Machine and transport

- WiFi support is Telnet only: no WebSocket/HTTP transport, TLS or mDNS
  discovery yet.
- Ruida, TopWisdom and galvo controllers are not implemented. Marlin support is
  laser-only and has not yet been validated on physical hardware. See
  [roadmap.md](roadmap.md).
- Pause is restricted to GRBL M4 jobs with `$32=1`; M3 jobs and all Marlin jobs
  can only be stopped.
- The bed rectangle comes from the active machine profile when set, otherwise
  from GRBL `$130`/`$131`. When GRBL does not report `WCO`, orleans assumes work
  zero coincides with machine zero.
- **Set zero (G92)** is temporary: a soft reset, Stop or controller reconnect
  after a reset clears it. **Save zero (G54)** writes `G10 L20 P1`; whether the
  offset survives a reset depends on the controller. Marlin supports only the
  temporary `G92` form, so saved work zero is disabled.
- The GRBL and Marlin simulators are protocol-level, not motion models.

### Geometry and toolpaths

- Ordering is per layer. If the outer contour is on a different layer from its
  holes, layers are cut in list order, so reorder layers as needed.
- Curves are flattened at import/plan time (default 0.02 mm), so G-code grows
  with detail. Arcs (`G2`/`G3`) are not emitted.
- No acceleration model in the time estimate; it is distance divided by feed
  rate.
- Boolean and offset results are flattened polylines; source curves are not
  reconstructed after the operation.
- Fill mode supports scanline hatching or inward concentric offset loops on
  closed contours with even-odd holes, an optional 90° cross-hatch pass and an
  optional outline cut. Offset fill loops stop when the remaining region is
  narrower than the spacing, and fill outlines do not use holding tabs.
- Automatic tabs are distributed evenly by arc length from the contour seam.
  Manual tabs are stored as fractions of the flattened contour perimeter, so a
  heavy node edit that reorders or removes contours can detach a tab.

### Editing

- Interactive handles scale uniformly around the opposite corner and rotate
  around the selection centre. There is no non-uniform or skewed scaling.
- Arrays are rectangular only: there is no polar/circular array. Copies follow
  fixed `dx`/`dy` spacing. Duplicate places copies exactly on top of the
  originals, ready to be dragged away.
- Groups contain vector shapes only and cannot be nested; grouping a selection
  that already touches groups merges them. Ungroup before node-editing a member.
  Align/distribute treat each group as one object and equalise object centres
  rather than edge gaps.
- Node editing supports moving anchors and Bézier controls, inserting anchors on
  segments, deleting anchors, breaking a contour at an anchor and joining two
  endpoint anchors, with optional grid and geometry snapping while dragging.
  There is no smoothing, node-type conversion or joining to an interior point.
- Only empty vector layers can be deleted. Delete their objects first; a raster
  layer disappears only when the design is replaced.
- Imported raster images always get a new raster layer, even when a vector layer
  has the same colour.

### Import

- PDF import reads one page's vector paths and form XObjects. Text must be
  outlined; raster images, clip paths, gradients, patterns, transparency groups
  and annotations are skipped. Illustrator files must be saved with PDF
  compatibility. Imported paths are not linked to their source file.
- DXF import covers ASCII files and the listed entities only. `HATCH`,
  dimensions, meshes, binary DXF and block attributes are skipped. Imported
  vector layers are self-contained and do not track later source edits.
- SVG and DXF text use the host's system fonts. Missing or SHX fonts fall back to
  sans-serif, so outlines may not match the source exactly.
- Raster import starts at 96 DPI but size and DPI can be changed after import;
  scan lines rotate with the image angle. Cropping is numeric and
  non-destructive: the full image stays visible with a cyan crop outline, while
  only the cropped region is engraved.

### Interface and rendering

- Interface localisation covers the main editor, layer, machine, job and dialog
  controls; some diagnostics and platform file dialogs remain in English.
  Chinese requires a system CJK font such as Noto Sans CJK or Source Han Sans.
- Language preference is stored in `$XDG_CONFIG_HOME/orleans/ui.json` (or
  `~/.config/orleans/ui.json`). Translation catalogs live in
  `crates/orleans-ui/locales/*.json`; English source strings are fallback keys.
- Large designs are redrawn as polylines every frame; a spatial index and
  level-of-detail rendering will be needed for very complex files.
