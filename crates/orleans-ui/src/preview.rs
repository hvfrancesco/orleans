//! Flattened geometry used for drawing.
//!
//! Flattening every frame would be wasteful, so the curves are converted to
//! polylines once per import. Zooming and panning then only transform points.

use kurbo::Point;
use orleans_core::Document;
use orleans_geom::flatten_path;

#[derive(Clone, Debug)]
pub struct PreviewContour {
    pub points: Vec<Point>,
    pub closed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct LayerPreview {
    /// The stable `Layer::index` this preview belongs to.
    pub index: usize,
    pub contours: Vec<PreviewContour>,
}

#[derive(Clone, Debug, Default)]
pub struct Preview {
    /// One entry per document layer, in the same order.
    pub layers: Vec<LayerPreview>,
}

impl Preview {
    pub fn build(doc: &Document, tolerance: f64) -> Self {
        let layers = doc
            .layers
            .iter()
            .map(|layer| LayerPreview {
                index: layer.index,
                contours: layer
                    .shapes
                    .iter()
                    .flat_map(|shape| flatten_path(&shape.path, tolerance))
                    .filter(|c| c.points.len() >= 2)
                    .map(|c| PreviewContour {
                        points: c.points,
                        closed: c.closed,
                    })
                    .collect(),
            })
            .collect();
        Self { layers }
    }

    pub fn contour_count(&self) -> usize {
        self.layers.iter().map(|l| l.contours.len()).sum()
    }

    pub fn layer(&self, index: usize) -> Option<&LayerPreview> {
        self.layers.iter().find(|layer| layer.index == index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{BezPath, Line, Shape as _};
    use orleans_core::{CutParams, Layer, Shape};

    #[test]
    fn flattens_shapes_into_drawable_contours() {
        let mut doc = Document::new(100.0, 100.0);
        let mut layer = Layer::new(0, "C00", [0, 0, 0], CutParams::default());
        let path: BezPath = Line::new((0.0, 0.0), (10.0, 0.0)).into_path(0.01);
        layer.shapes.push(Shape::new("line", path, false));
        doc.layers.push(layer);

        let preview = Preview::build(&doc, 0.05);
        assert_eq!(preview.layers.len(), 1);
        assert_eq!(preview.contour_count(), 1);
        assert_eq!(preview.layers[0].index, 0);
        assert_eq!(preview.layers[0].contours[0].points.len(), 2);
    }
}
