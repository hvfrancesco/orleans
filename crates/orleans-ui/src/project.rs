//! Orleans project files: the embedded SVG plus layer and job settings.
//!
//! This is a clean-room format; it does not read or write LightBurn files.
//! Geometry comes from the SVG, while the project stores the per-layer cutting
//! parameters and the job-level options.

use orleans_core::{CutParams, Document, LayerMode};
use orleans_driver::gcode::LaserMode;
use serde::{Deserialize, Serialize};

pub const FORMAT: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectFile {
    pub format: u32,
    /// The original SVG document, embedded so the project is self-contained.
    pub svg: String,
    /// Where the SVG came from, for display only.
    #[serde(default)]
    pub source: Option<String>,
    pub layers: Vec<LayerSettings>,
    pub kerf: f64,
    pub optimize: bool,
    pub laser_mode: String,
    pub max_s: u32,
    /// Translations applied to individual shapes in the editor.
    #[serde(default)]
    pub transforms: Vec<ShapeTransform>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShapeTransform {
    pub layer: usize,
    pub shape: String,
    pub dx: f64,
    pub dy: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayerSettings {
    pub index: usize,
    pub name: String,
    pub color: [u8; 3],
    pub mode: String,
    pub speed: f64,
    pub power: f64,
    pub passes: u32,
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default = "default_true")]
    pub output: bool,
}

fn default_true() -> bool {
    true
}

impl ProjectFile {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        svg: String,
        source: Option<String>,
        doc: &Document,
        kerf: f64,
        optimize: bool,
        laser_mode: LaserMode,
        max_s: u32,
        transforms: Vec<ShapeTransform>,
    ) -> Self {
        Self {
            format: FORMAT,
            svg,
            source,
            layers: doc
                .layers
                .iter()
                .map(|layer| LayerSettings {
                    index: layer.index,
                    name: layer.name.clone(),
                    color: layer.color,
                    mode: format!("{:?}", layer.mode),
                    speed: layer.cut.speed,
                    power: layer.cut.power,
                    passes: layer.cut.passes,
                    visible: layer.visible,
                    output: layer.output,
                })
                .collect(),
            kerf,
            optimize,
            laser_mode: match laser_mode {
                LaserMode::M3 => "M3",
                LaserMode::M4 => "M4",
            }
            .to_owned(),
            max_s,
            transforms,
        }
    }

    /// Apply the stored per-layer settings and cut order to a freshly imported
    /// document.
    pub fn apply(&self, doc: &mut Document) {
        // The order of `layers` in the project is the cut order.
        let mut ordered = Vec::with_capacity(doc.layers.len());
        for saved in &self.layers {
            if let Some(position) = doc
                .layers
                .iter()
                .position(|layer| layer.index == saved.index)
            {
                ordered.push(doc.layers.remove(position));
            }
        }
        ordered.append(&mut doc.layers);
        doc.layers = ordered;

        for layer in &mut doc.layers {
            if let Some(saved) = self.layers.iter().find(|saved| saved.index == layer.index) {
                layer.name = saved.name.clone();
                layer.color = saved.color;
                layer.visible = saved.visible;
                layer.output = saved.output;
                layer.mode = if saved.mode.eq_ignore_ascii_case("fill") {
                    LayerMode::Fill
                } else {
                    LayerMode::Line
                };
                layer.cut = CutParams::new(
                    saved.speed.max(0.0),
                    saved.power.clamp(0.0, 100.0),
                    saved.passes,
                );
            }
        }

        for transform in &self.transforms {
            if let Some(layer) = doc
                .layers
                .iter_mut()
                .find(|layer| layer.index == transform.layer)
            {
                if let Some(shape) = layer
                    .shapes
                    .iter_mut()
                    .find(|shape| shape.id == transform.shape)
                {
                    shape
                        .path
                        .apply_affine(kurbo::Affine::translate((transform.dx, transform.dy)));
                }
            }
        }
    }

    pub fn laser_mode(&self) -> LaserMode {
        if self.laser_mode.eq_ignore_ascii_case("M4") {
            LaserMode::M4
        } else {
            LaserMode::M3
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orleans_core::{Layer, Shape};

    #[test]
    fn json_round_trip_keeps_layer_settings() {
        let mut doc = Document::new(100.0, 80.0);
        let mut layer = Layer::new(0, "C00", [10, 20, 30], CutParams::new(1200.0, 35.0, 2));
        layer.mode = LayerMode::Line;
        doc.layers.push(layer);

        let project = ProjectFile::new(
            "<svg/>".to_owned(),
            Some("/tmp/design.svg".to_owned()),
            &doc,
            0.15,
            true,
            LaserMode::M4,
            1000,
            vec![ShapeTransform {
                layer: 0,
                shape: "line".to_owned(),
                dx: 1.5,
                dy: -2.0,
            }],
        );
        let json = serde_json::to_string(&project).expect("serialise");
        let back: ProjectFile = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.format, FORMAT);
        assert_eq!(back.kerf, 0.15);
        assert_eq!(back.laser_mode(), LaserMode::M4);
        assert_eq!(back.layers.len(), 1);
        assert_eq!(back.layers[0].speed, 1200.0);
        assert_eq!(back.layers[0].power, 35.0);
        assert_eq!(back.layers[0].passes, 2);
        assert_eq!(back.transforms.len(), 1);
        assert_eq!(back.transforms[0].dx, 1.5);
        assert_eq!(back.transforms[0].dy, -2.0);
    }

    #[test]
    fn apply_translates_shapes() {
        use kurbo::{Line, Shape as _};
        let mut doc = Document::new(100.0, 100.0);
        let mut layer = Layer::new(0, "C00", [0, 0, 0], CutParams::default());
        layer.shapes.push(Shape::new(
            "line",
            Line::new((0.0, 0.0), (10.0, 0.0)).into_path(0.01),
            false,
        ));
        doc.layers.push(layer);
        let project = ProjectFile {
            format: FORMAT,
            svg: "<svg/>".to_owned(),
            source: None,
            layers: Vec::new(),
            kerf: 0.0,
            optimize: true,
            laser_mode: "M4".to_owned(),
            max_s: 1000,
            transforms: vec![ShapeTransform {
                layer: 0,
                shape: "line".to_owned(),
                dx: 5.0,
                dy: 2.0,
            }],
        };
        project.apply(&mut doc);
        let bounds = doc.layers[0].shapes[0].path.bounding_box();
        assert!((bounds.x0 - 5.0).abs() < 1e-6);
        assert!((bounds.y0 - 2.0).abs() < 1e-6);
        assert!((bounds.x1 - 15.0).abs() < 1e-6);
    }

    #[test]
    fn apply_restores_layer_cut_order() {
        let mut doc = Document::new(10.0, 10.0);
        doc.layers
            .push(Layer::new(0, "first", [0, 0, 0], CutParams::default()));
        doc.layers
            .push(Layer::new(1, "second", [0, 0, 0], CutParams::default()));
        let saved = |index: usize| LayerSettings {
            index,
            name: format!("layer {index}"),
            color: [0, 0, 0],
            mode: "Line".to_owned(),
            speed: 1000.0,
            power: 20.0,
            passes: 1,
            visible: true,
            output: true,
        };
        let project = ProjectFile {
            format: FORMAT,
            svg: "<svg/>".to_owned(),
            source: None,
            layers: vec![saved(1), saved(0)],
            kerf: 0.0,
            optimize: true,
            laser_mode: "M4".to_owned(),
            max_s: 1000,
            transforms: Vec::new(),
        };
        project.apply(&mut doc);
        assert_eq!(
            doc.layers
                .iter()
                .map(|layer| layer.index)
                .collect::<Vec<_>>(),
            vec![1, 0]
        );
    }

    #[test]
    fn apply_overrides_imported_defaults() {
        let mut doc = Document::new(100.0, 80.0);
        doc.layers
            .push(Layer::new(0, "C00", [0, 0, 0], CutParams::default()));
        let project = ProjectFile {
            format: FORMAT,
            svg: "<svg/>".to_owned(),
            source: None,
            layers: vec![LayerSettings {
                index: 0,
                name: "holes".to_owned(),
                color: [255, 0, 0],
                mode: "Line".to_owned(),
                speed: 800.0,
                power: 12.5,
                passes: 3,
                visible: false,
                output: true,
            }],
            kerf: 0.1,
            optimize: false,
            laser_mode: "M3".to_owned(),
            max_s: 500,
            transforms: Vec::new(),
        };
        project.apply(&mut doc);
        let layer = &doc.layers[0];
        assert_eq!(layer.name, "holes");
        assert_eq!(layer.color, [255, 0, 0]);
        assert_eq!(layer.cut.speed, 800.0);
        assert_eq!(layer.cut.power, 12.5);
        assert_eq!(layer.cut.passes, 3);
        assert!(!layer.visible);
        assert!(layer.output);
    }
}
