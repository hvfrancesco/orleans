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
    /// Fill the geometry with a scan pattern (not implemented yet).
    Fill,
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

/// A group of shapes sharing cutting parameters, equivalent to a LightBurn layer.
#[derive(Clone, Debug)]
pub struct Layer {
    pub index: usize,
    pub name: String,
    /// Display colour, RGB.
    pub color: [u8; 3],
    pub mode: LayerMode,
    pub cut: CutParams,
    pub shapes: Vec<Shape>,
    /// Whether the layer participates in previews, job planning and framing.
    pub visible: bool,
}

impl Layer {
    pub fn new(index: usize, name: impl Into<String>, color: [u8; 3], cut: CutParams) -> Self {
        Self {
            index,
            name: name.into(),
            color,
            mode: LayerMode::Line,
            cut,
            shapes: Vec::new(),
            visible: true,
        }
    }

    /// True when the layer has geometry that can be cut.
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
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

    /// Bounding box of all geometry, if the document is not empty.
    pub fn bounds(&self) -> Option<Rect> {
        let mut bounds: Option<Rect> = None;
        for shape in self
            .layers
            .iter()
            .filter(|layer| layer.visible)
            .flat_map(|layer| layer.shapes.iter())
        {
            if shape.path.elements().is_empty() {
                continue;
            }
            let b = shape.bounds();
            bounds = Some(match bounds {
                Some(cur) => cur.union(b),
                None => b,
            });
        }
        // `BezPath::bounding_box` returns an empty rect for empty paths, which
        // would poison the union.
        bounds.filter(|b| b.width() > 0.0 || b.height() > 0.0)
    }

    pub fn shape_count(&self) -> usize {
        self.layers.iter().map(|l| l.shapes.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.shape_count() == 0
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
    fn cut_params_clamp_passes() {
        assert_eq!(CutParams::new(100.0, 10.0, 0).passes, 1);
    }
}
