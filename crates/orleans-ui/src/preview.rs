//! Flattened geometry used for drawing and hit-testing.
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

#[derive(Clone, Debug)]
pub struct ShapePreview {
    pub id: String,
    pub contours: Vec<PreviewContour>,
}

#[derive(Clone, Debug, Default)]
pub struct LayerPreview {
    /// The stable `Layer::index` this preview belongs to.
    pub index: usize,
    pub shapes: Vec<ShapePreview>,
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
                shapes: layer
                    .shapes
                    .iter()
                    .map(|shape| ShapePreview {
                        id: shape.id.clone(),
                        contours: flatten_path(&shape.path, tolerance)
                            .into_iter()
                            .filter(|contour| contour.points.len() >= 2)
                            .map(|contour| PreviewContour {
                                points: contour.points,
                                closed: contour.closed,
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
        Self { layers }
    }

    pub fn contour_count(&self) -> usize {
        self.layers
            .iter()
            .flat_map(|layer| layer.shapes.iter())
            .map(|shape| shape.contours.len())
            .sum()
    }

    pub fn layer(&self, index: usize) -> Option<&LayerPreview> {
        self.layers.iter().find(|layer| layer.index == index)
    }

    pub fn shape(&self, layer: usize, id: &str) -> Option<&ShapePreview> {
        self.layer(layer)?
            .shapes
            .iter()
            .find(|shape| shape.id == id)
    }

    pub fn shape_mut(&mut self, layer: usize, id: &str) -> Option<&mut ShapePreview> {
        self.layers
            .iter_mut()
            .find(|entry| entry.index == layer)?
            .shapes
            .iter_mut()
            .find(|shape| shape.id == id)
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
        assert!(preview.shape(0, "line").is_some());
        assert_eq!(
            preview.shape(0, "line").unwrap().contours[0].points.len(),
            2
        );
    }
}
