# Native project format

`.orleans` is a self-contained JSON snapshot of the editor document. The current
format is **2**. There is intentionally no v1 compatibility because v1 was never
released.

The file must be saved and loaded as one atomic document, not treated as an SVG
plus a list of edits. Loading never depends on the original SVG, DXF or raster
source.

## Top level

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

`layers` is in cut order. `groups` is the editor group list.

## Layer

Each layer stores its complete state:

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
- `groups[].members` reference a layer index plus shape id.
- Manual tab positions reference the same pair, with positions expressed as a
  fraction of the flattened contour perimeter.
- `source` is provenance for display only. Loading never reads it.

## Raster layers

Raster layers use:

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

## Compatibility rules

- Loading is direct deserialization into `Document`.
- A project does not depend on importer output being deterministic.
- Unknown top-level fields should be ignored by future readers.
- Incompatible changes must bump `format`.
