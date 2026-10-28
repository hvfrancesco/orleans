//! Core document model shared by every layer of the stack.
//!
//! Coordinates are millimetres, with the origin at the bottom-left of the
//! machine bed and Y pointing up (the convention used by GRBL and by most
//! laser cutters). Importers are responsible for converting whatever
//! convention their source format uses into this one.

use kurbo::Shape as _;
use kurbo::{BezPath, Point, Rect};

/// Millimetres. Used as a type alias only, to make signatures self-documenting.
pub type Mm = f64;

/// How a layer is rendered into toolpaths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerMode {
    /// Follow the outlines of the geometry.
    Line,
    /// Fill closed geometry with scanline hatching.
    Fill,
    /// Engrave a raster image.
    Raster,
}

/// Cutting parameters attached to a layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CutParams {
    /// Feed rate in mm/min.
    pub speed: f64,
    /// Laser power as a percentage of the machine's maximum, 0..=100.
    pub power: f64,
    /// How many times the contour is traced.
    pub passes: u32,
}

impl Default for CutParams {
    fn default() -> Self {
        Self {
            speed: 1000.0,
            power: 20.0,
            passes: 1,
        }
    }
}

impl CutParams {
    pub fn new(speed: f64, power: f64, passes: u32) -> Self {
        Self {
            speed,
            power,
            passes: passes.max(1),
        }
    }
}

/// Scanline fill settings for a layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FillParams {
    /// Distance between hatch lines in millimetres.
    pub spacing: f64,
    /// Hatch angle in degrees. 0 is horizontal, 90 is vertical.
    pub angle: f64,
    /// Extra distance added to both ends of every hatch segment, in
    /// millimetres. Useful for closing gaps at the boundary.
    pub overlap: f64,
    /// Reverse alternate scan lines to reduce travel.
    pub alternate: bool,
    /// Add a second hatch pass at 90° to the first.
    pub cross: bool,
    /// Also cut the closed contours after filling them.
    pub outline: bool,
}

impl Default for FillParams {
    fn default() -> Self {
        Self {
            spacing: 0.25,
            angle: 0.0,
            overlap: 0.0,
            alternate: true,
            cross: false,
            outline: false,
        }
    }
}

/// Raster engraving mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RasterMode {
    #[default]
    Threshold,
    Dither,
    Grayscale,
}

/// A grayscale image placed in document millimetres.
#[derive(Clone, Debug, PartialEq)]
pub struct RasterImage {
    pub width: u32,
    pub height: u32,
    /// Row-major grayscale, top row first.
    pub grayscale: Vec<u8>,
    pub x: f64,
    pub y: f64,
    pub width_mm: f64,
    pub height_mm: f64,
    /// Rotation around the image centre, in degrees.
    pub angle: f64,
    pub mode: RasterMode,
    /// Darkness threshold, 0..=255.
    pub threshold: u8,
    pub invert: bool,
    /// Spacing between engraved scan lines in millimetres.
    pub line_interval: f64,
    /// Reverse alternate scan lines to reduce travel.
    pub bidirectional: bool,
}

impl RasterImage {
    pub fn center(&self) -> Point {
        Point::new(self.x + self.width_mm * 0.5, self.y + self.height_mm * 0.5)
    }

    /// Convert an unrotated local point into world space.
    pub fn local_to_world(&self, local: Point) -> Point {
        let center = self.center();
        let (sin, cos) = self.angle.to_radians().sin_cos();
        let dx = local.x - center.x;
        let dy = local.y - center.y;
        Point::new(
            center.x + dx * cos - dy * sin,
            center.y + dx * sin + dy * cos,
        )
    }

    /// Convert a world point into the image's unrotated local frame.
    pub fn world_to_local(&self, world: Point) -> Point {
        let center = self.center();
        let (sin, cos) = self.angle.to_radians().sin_cos();
        let dx = world.x - center.x;
        let dy = world.y - center.y;
        Point::new(
            center.x + dx * cos + dy * sin,
            center.y - dx * sin + dy * cos,
        )
    }

    /// Corners in top-left, top-right, bottom-right, bottom-left order.
    pub fn corners(&self) -> [Point; 4] {
        [
            self.local_to_world(Point::new(self.x, self.y + self.height_mm)),
            self.local_to_world(Point::new(self.x + self.width_mm, self.y + self.height_mm)),
            self.local_to_world(Point::new(self.x + self.width_mm, self.y)),
            self.local_to_world(Point::new(self.x, self.y)),
        ]
    }

    pub fn bounds(&self) -> Rect {
        let corners = self.corners();
        let mut rect = Rect::new(corners[0].x, corners[0].y, corners[0].x, corners[0].y);
        for corner in &corners[1..] {
            rect = rect.union(Rect::new(corner.x, corner.y, corner.x, corner.y));
        }
        rect
    }

    pub fn pixel(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height {
            return 255;
        }
        self.grayscale
            .get((y * self.width + x) as usize)
            .copied()
            .unwrap_or(255)
    }
}

/// Holding tabs left uncut around closed contours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabParams {
    /// Number of tabs per contour. Zero disables tabs.
    pub count: u32,
    /// Width of each uncut tab in millimetres.
    pub width: f64,
    /// Also place tabs on inner contours (holes).
    pub inner: bool,
    /// Use the positions in [`Layer::manual_tabs`] instead of distributing
    /// `count` tabs evenly.
    pub manual: bool,
}

/// Manually placed tabs on one closed contour of a shape.
#[derive(Clone, Debug, PartialEq)]
pub struct ManualTabs {
    pub shape: String,
    /// Index of the closed contour in the flattened shape path.
    pub contour: usize,
    /// Tab centres as fractions of the contour perimeter, in `0..1`.
    pub positions: Vec<f64>,
}

impl Default for TabParams {
    fn default() -> Self {
        Self {
            count: 0,
            width: 3.0,
            inner: false,
            manual: false,
        }
    }
}

/// A group of shapes sharing cutting parameters, equivalent to a LightBurn layer.
#[derive(Clone, Debug)]
pub struct Layer {
    pub index: usize,
    pub name: String,
    /// Display colour, RGB.
    pub color: [u8; 3],
    pub mode: LayerMode,
    pub cut: CutParams,
    pub fill: FillParams,
    pub tabs: TabParams,
    pub manual_tabs: Vec<ManualTabs>,
    pub raster: Option<RasterImage>,
    pub shapes: Vec<Shape>,
    /// Whether the layer is drawn in the editor.
    pub visible: bool,
    /// Whether the layer is planned, framed and sent to the machine.
    pub output: bool,
}

impl Layer {
    pub fn new(index: usize, name: impl Into<String>, color: [u8; 3], cut: CutParams) -> Self {
        Self {
            index,
            name: name.into(),
            color,
            mode: LayerMode::Line,
            cut,
            fill: FillParams::default(),
            tabs: TabParams::default(),
            manual_tabs: Vec::new(),
            raster: None,
            shapes: Vec::new(),
            visible: true,
            output: true,
        }
    }

    /// True when the layer has geometry that can be cut.
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty() && self.raster.is_none()
    }
}

/// A single piece of geometry: one or more subpaths plus a fill flag.
#[derive(Clone, Debug)]
pub struct Shape {
    /// Stable identifier, useful for selection and later editing.
    pub id: String,
    pub path: BezPath,
    /// Whether the source geometry was filled. Filled geometry still cuts on
    /// its outline in [`LayerMode::Line`]; the flag is kept for fill planning.
    pub is_fill: bool,
}

impl Shape {
    pub fn new(id: impl Into<String>, path: BezPath, is_fill: bool) -> Self {
        Self {
            id: id.into(),
            path,
            is_fill,
        }
    }

    pub fn bounds(&self) -> Rect {
        self.path.bounding_box()
    }
}

/// A design: page size plus a list of layers.
#[derive(Clone, Debug, Default)]
pub struct Document {
    pub width: Mm,
    pub height: Mm,
    pub layers: Vec<Layer>,
}

impl Document {
    pub fn new(width: Mm, height: Mm) -> Self {
        Self {
            width,
            height,
            layers: Vec::new(),
        }
    }

    /// Bounding box of all visible geometry, if the document is not empty.
    pub fn bounds(&self) -> Option<Rect> {
        self.bounds_for(|layer| layer.visible)
    }

    /// Bounding box of the geometry that will actually be cut or framed.
    pub fn output_bounds(&self) -> Option<Rect> {
        self.bounds_for(|layer| layer.output)
    }

    fn bounds_for(&self, include: impl Fn(&Layer) -> bool) -> Option<Rect> {
        let mut bounds: Option<Rect> = None;
        for layer in self.layers.iter().filter(|layer| include(layer)) {
            if let Some(raster) = &layer.raster {
                if raster.width_mm > 0.0 && raster.height_mm > 0.0 {
                    let b = raster.bounds();
                    bounds = Some(match bounds {
                        Some(current) => current.union(b),
                        None => b,
                    });
                }
            }
            for shape in &layer.shapes {
                if shape.path.elements().is_empty() {
                    continue;
                }
                let b = shape.bounds();
                bounds = Some(match bounds {
                    Some(current) => current.union(b),
                    None => b,
                });
            }
        }
        // `BezPath::bounding_box` returns an empty rect for empty paths, which
        // would poison the union.
        bounds.filter(|b| b.width() > 0.0 || b.height() > 0.0)
    }

    pub fn shape_count(&self) -> usize {
        self.layers.iter().map(|l| l.shapes.len()).sum()
    }

    pub fn raster_count(&self) -> usize {
        self.layers
            .iter()
            .filter(|layer| layer.raster.is_some())
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.iter().all(Layer::is_empty)
    }
}

/// A single machine operation produced by the planner.
#[derive(Clone, Debug)]
pub enum Op {
    /// Move with the laser off.
    Rapid { to: Point },
    /// Trace a polyline with the laser on. The first point is where the head
    /// already is; the polyline is emitted as a sequence of `G1` moves.
    Cut {
        polyline: Vec<Point>,
        params: CutParams,
        layer: usize,
    },
}

impl Op {
    /// Distance travelled by this operation in millimetres.
    pub fn length(&self) -> f64 {
        match self {
            Op::Rapid { .. } => 0.0,
            Op::Cut { polyline, .. } => polyline_length(polyline),
        }
    }
}

fn polyline_length(points: &[Point]) -> f64 {
    points.windows(2).map(|w| (w[1] - w[0]).hypot()).sum()
}

/// Aggregate statistics for a planned job.
#[derive(Clone, Copy, Debug, Default)]
pub struct JobStats {
    pub cut_mm: f64,
    pub travel_mm: f64,
    /// Wall-clock estimate in seconds, including acceleration-free travel.
    pub seconds: f64,
}

/// An ordered list of operations ready to be serialised or streamed.
#[derive(Clone, Debug, Default)]
pub struct Job {
    pub ops: Vec<Op>,
    pub stats: JobStats,
}

impl Job {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Cuts with the laser on, in order.
    pub fn cuts(&self) -> impl Iterator<Item = (&[Point], &CutParams, usize)> {
        self.ops.iter().filter_map(|op| match op {
            Op::Cut {
                polyline,
                params,
                layer,
            } => Some((polyline.as_slice(), params, *layer)),
            Op::Rapid { .. } => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Line;

    #[test]
    fn document_bounds_ignore_empty_paths() {
        let mut doc = Document::new(100.0, 100.0);
        let mut layer = Layer::new(0, "C00", [0, 0, 0], CutParams::default());
        layer
            .shapes
            .push(Shape::new("empty", BezPath::new(), false));
        layer.shapes.push(Shape::new(
            "line",
            Line::new((10.0, 20.0), (30.0, 40.0)).into_path(0.01),
            false,
        ));
        doc.layers.push(layer);

        let bounds = doc.bounds().expect("bounds");
        assert_eq!(bounds.x0, 10.0);
        assert_eq!(bounds.y0, 20.0);
        assert_eq!(bounds.x1, 30.0);
        assert_eq!(bounds.y1, 40.0);
    }

    #[test]
    fn raster_local_world_round_trip_with_rotation() {
        let raster = RasterImage {
            width: 2,
            height: 2,
            grayscale: vec![0; 4],
            x: 5.0,
            y: 10.0,
            width_mm: 10.0,
            height_mm: 5.0,
            angle: 37.0,
            mode: RasterMode::Threshold,
            threshold: 128,
            invert: false,
            line_interval: 0.2,
            bidirectional: true,
        };
        for point in [
            Point::new(5.0, 10.0),
            Point::new(15.0, 15.0),
            Point::new(7.5, 12.5),
        ] {
            let back = raster.world_to_local(raster.local_to_world(point));
            assert!((back - point).hypot() < 1e-9, "{back:?} != {point:?}");
        }
        let bounds = raster.bounds();
        assert!(bounds.width() > 10.0);
        assert!(bounds.height() > 5.0);
    }

    #[test]
    fn document_bounds_ignore_hidden_layers() {
        let mut doc = Document::new(100.0, 100.0);
        let mut hidden = Layer::new(0, "hidden", [0, 0, 0], CutParams::default());
        hidden.visible = false;
        hidden.shapes.push(Shape::new(
            "hidden",
            Line::new((10.0, 20.0), (30.0, 40.0)).into_path(0.01),
            false,
        ));
        doc.layers.push(hidden);

        let mut shown = Layer::new(1, "shown", [0, 0, 0], CutParams::default());
        shown.shapes.push(Shape::new(
            "shown",
            Line::new((0.0, 0.0), (5.0, 5.0)).into_path(0.01),
            false,
        ));
        doc.layers.push(shown);

        let bounds = doc.bounds().expect("bounds");
        assert_eq!(bounds.x0, 0.0);
        assert_eq!(bounds.y0, 0.0);
        assert_eq!(bounds.x1, 5.0);
        assert_eq!(bounds.y1, 5.0);
    }

    #[test]
    fn output_bounds_ignore_non_output_layers() {
        let mut doc = Document::new(100.0, 100.0);
        let mut reference = Layer::new(0, "reference", [0, 0, 0], CutParams::default());
        reference.output = false;
        reference.shapes.push(Shape::new(
            "reference",
            Line::new((40.0, 40.0), (60.0, 60.0)).into_path(0.01),
            false,
        ));
        doc.layers.push(reference);

        let mut cut = Layer::new(1, "cut", [0, 0, 0], CutParams::default());
        cut.shapes.push(Shape::new(
            "cut",
            Line::new((0.0, 0.0), (5.0, 5.0)).into_path(0.01),
            false,
        ));
        doc.layers.push(cut);

        let visible = doc.bounds().expect("visible bounds");
        assert_eq!(visible.x0, 0.0);
        assert_eq!(visible.x1, 60.0);
        let output = doc.output_bounds().expect("output bounds");
        assert_eq!(output.x0, 0.0);
        assert_eq!(output.x1, 5.0);
    }

    #[test]
    fn cut_params_clamp_passes() {
        assert_eq!(CutParams::new(100.0, 10.0, 0).passes, 1);
    }
}
