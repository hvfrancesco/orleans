//! Orleans project files: the embedded SVG plus layer and job settings.
//!
//! This is a clean-room format; it does not read or write LightBurn files.
//! Geometry comes from the SVG, while the project stores the per-layer cutting
//! parameters, the job-level options, shape transforms and node-edited paths.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use orleans_core::{CutParams, Document, Layer, LayerMode, RasterImage, RasterMode, Shape};
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
    /// Document page size in millimetres.
    #[serde(default = "default_page_size")]
    pub width_mm: f64,
    #[serde(default = "default_page_size")]
    pub height_mm: f64,
    /// Affine edits applied to individual shapes in the editor.
    #[serde(default)]
    pub transforms: Vec<ShapeTransform>,
    /// Complete path overrides for shapes edited with the node editor.
    #[serde(default)]
    pub edited_paths: Vec<EditedPath>,
    /// Editor groups. They affect selection and transforms, not cutting.
    #[serde(default)]
    pub groups: Vec<GroupSettings>,
    /// Shapes moved out of their original SVG layer.
    #[serde(default)]
    pub moved_shapes: Vec<MovedShape>,
    /// Shapes created by copying, with no source in the embedded SVG.
    #[serde(default)]
    pub extra_shapes: Vec<ExtraShape>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MovedShape {
    pub source_layer: usize,
    pub source_shape: String,
    pub target_layer: usize,
    pub target_shape: String,
    /// Final path in document coordinates.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub is_fill: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtraShape {
    pub layer: usize,
    pub shape: String,
    /// Final path in document coordinates.
    pub path: String,
    #[serde(default)]
    pub is_fill: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupSettings {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub members: Vec<GroupMember>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupMember {
    pub layer: usize,
    pub shape: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditedPath {
    pub layer: usize,
    pub shape: String,
    /// SVG path data in document coordinates.
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShapeTransform {
    pub layer: usize,
    pub shape: String,
    /// Full 2D affine from the imported shape to the edited shape.
    #[serde(default)]
    pub matrix: Option<[f64; 6]>,
    /// Legacy translation fields, used when `matrix` is absent.
    #[serde(default)]
    pub dx: f64,
    #[serde(default)]
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
    #[serde(default = "default_fill_spacing")]
    pub fill_spacing: f64,
    #[serde(default)]
    pub fill_angle: f64,
    #[serde(default)]
    pub fill_overlap: f64,
    #[serde(default = "default_true")]
    pub fill_alternate: bool,
    /// Embedded grayscale raster image, if this is a raster layer.
    #[serde(default)]
    pub raster: Option<RasterSettings>,
    #[serde(default)]
    pub tab_count: u32,
    #[serde(default = "default_tab_width")]
    pub tab_width: f64,
    #[serde(default)]
    pub tab_inner: bool,
}

fn default_true() -> bool {
    true
}

fn default_fill_spacing() -> f64 {
    orleans_core::FillParams::default().spacing
}

fn default_tab_width() -> f64 {
    orleans_core::TabParams::default().width
}

fn default_page_size() -> f64 {
    0.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RasterSettings {
    pub width: u32,
    pub height: u32,
    /// Base64-encoded row-major grayscale pixels, top row first.
    pub pixels: String,
    pub x: f64,
    pub y: f64,
    pub width_mm: f64,
    pub height_mm: f64,
    #[serde(default)]
    pub angle: f64,
    pub mode: String,
    pub threshold: u8,
    pub invert: bool,
    pub line_interval: f64,
    pub bidirectional: bool,
}

impl RasterSettings {
    fn from_image(image: &RasterImage) -> Self {
        Self {
            width: image.width,
            height: image.height,
            pixels: STANDARD.encode(&image.grayscale),
            x: image.x,
            y: image.y,
            width_mm: image.width_mm,
            height_mm: image.height_mm,
            angle: image.angle,
            mode: match image.mode {
                RasterMode::Threshold => "Threshold",
                RasterMode::Dither => "Dither",
                RasterMode::Grayscale => "Grayscale",
            }
            .to_owned(),
            threshold: image.threshold,
            invert: image.invert,
            line_interval: image.line_interval,
            bidirectional: image.bidirectional,
        }
    }

    fn to_image(&self) -> Option<RasterImage> {
        if self.width == 0 || self.height == 0 || self.width_mm <= 0.0 || self.height_mm <= 0.0 {
            return None;
        }
        let pixels = STANDARD.decode(&self.pixels).ok()?;
        if pixels.len() != (self.width as usize).checked_mul(self.height as usize)? {
            return None;
        }
        Some(RasterImage {
            width: self.width,
            height: self.height,
            grayscale: pixels,
            x: self.x,
            y: self.y,
            width_mm: self.width_mm,
            height_mm: self.height_mm,
            angle: self.angle,
            mode: if self.mode.eq_ignore_ascii_case("dither") {
                RasterMode::Dither
            } else if self.mode.eq_ignore_ascii_case("grayscale") {
                RasterMode::Grayscale
            } else {
                RasterMode::Threshold
            },
            threshold: self.threshold,
            invert: self.invert,
            line_interval: self.line_interval,
            bidirectional: self.bidirectional,
        })
    }
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
                    fill_spacing: layer.fill.spacing,
                    fill_angle: layer.fill.angle,
                    fill_overlap: layer.fill.overlap,
                    fill_alternate: layer.fill.alternate,
                    raster: layer.raster.as_ref().map(RasterSettings::from_image),
                    tab_count: layer.tabs.count,
                    tab_width: layer.tabs.width,
                    tab_inner: layer.tabs.inner,
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
            width_mm: doc.width.max(1.0),
            height_mm: doc.height.max(1.0),
            transforms,
            edited_paths: Vec::new(),
            groups: Vec::new(),
            moved_shapes: Vec::new(),
            extra_shapes: Vec::new(),
        }
    }

    /// Apply the stored per-layer settings and cut order to a freshly imported
    /// document, adding any raster layers that are not present in the SVG.
    pub fn apply(&self, doc: &mut Document) {
        // Saved raster layers have no SVG geometry, so create missing layers
        // before ordering the final layer list.
        for saved in &self.layers {
            if doc.layers.iter().any(|layer| layer.index == saved.index) {
                continue;
            }
            let mut layer = Layer::new(
                saved.index,
                saved.name.clone(),
                saved.color,
                CutParams::new(
                    saved.speed.max(0.0),
                    saved.power.clamp(0.0, 100.0),
                    saved.passes,
                ),
            );
            layer.visible = saved.visible;
            layer.output = saved.output;
            layer.mode = parse_mode(&saved.mode);
            layer.fill = fill_from(saved);
            layer.tabs = tabs_from(saved);
            layer.raster = saved.raster.as_ref().and_then(RasterSettings::to_image);
            doc.layers.push(layer);
        }

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
                layer.mode = parse_mode(&saved.mode);
                layer.cut = CutParams::new(
                    saved.speed.max(0.0),
                    saved.power.clamp(0.0, 100.0),
                    saved.passes,
                );
                layer.fill = fill_from(saved);
                layer.tabs = tabs_from(saved);
                if let Some(raster) = &saved.raster {
                    if let Some(image) = raster.to_image() {
                        layer.raster = Some(image);
                    }
                }
            }
        }

        if self.width_mm > 0.0 && self.height_mm > 0.0 {
            doc.width = self.width_mm;
            doc.height = self.height_mm;
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
                    let affine = transform
                        .matrix
                        .map(kurbo::Affine::new)
                        .unwrap_or_else(|| kurbo::Affine::translate((transform.dx, transform.dy)));
                    shape.path.apply_affine(affine);
                }
            }
        }

        // Node-edited paths are stored in final document coordinates. Apply
        // them after transforms so a reload restores the exact edited shape.
        for edited in &self.edited_paths {
            let Ok(path) = kurbo::BezPath::from_svg(&edited.path) else {
                continue;
            };
            if path.elements().is_empty() {
                continue;
            }
            if let Some(shape) = doc
                .layers
                .iter_mut()
                .find(|layer| layer.index == edited.layer)
                .and_then(|layer| {
                    layer
                        .shapes
                        .iter_mut()
                        .find(|shape| shape.id == edited.shape)
                })
            {
                shape.path = path;
            }
        }

        // Move shapes after transforms and node edits, so their stored final
        // path and target layer win over the original SVG geometry.
        for moved in &self.moved_shapes {
            if !doc
                .layers
                .iter()
                .any(|layer| layer.index == moved.target_layer)
            {
                continue;
            }
            let Some(source_layer) = doc
                .layers
                .iter_mut()
                .find(|layer| layer.index == moved.source_layer)
            else {
                continue;
            };
            let Some(position) = source_layer
                .shapes
                .iter()
                .position(|shape| shape.id == moved.source_shape)
            else {
                continue;
            };
            let source = source_layer.shapes.remove(position);
            let (path, is_fill) = moved
                .path
                .as_deref()
                .and_then(|path| kurbo::BezPath::from_svg(path).ok())
                .filter(|path| !path.elements().is_empty())
                .map(|path| (path, moved.is_fill))
                .unwrap_or((source.path, moved.is_fill || source.is_fill));
            if let Some(target) = doc
                .layers
                .iter_mut()
                .find(|layer| layer.index == moved.target_layer)
            {
                target
                    .shapes
                    .push(Shape::new(moved.target_shape.clone(), path, is_fill));
            }
        }

        for extra in &self.extra_shapes {
            let Ok(path) = kurbo::BezPath::from_svg(&extra.path) else {
                continue;
            };
            if path.elements().is_empty() {
                continue;
            }
            if let Some(layer) = doc
                .layers
                .iter_mut()
                .find(|layer| layer.index == extra.layer)
            {
                layer
                    .shapes
                    .push(Shape::new(extra.shape.clone(), path, extra.is_fill));
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

fn parse_mode(mode: &str) -> LayerMode {
    if mode.eq_ignore_ascii_case("fill") {
        LayerMode::Fill
    } else if mode.eq_ignore_ascii_case("raster") {
        LayerMode::Raster
    } else {
        LayerMode::Line
    }
}

fn fill_from(saved: &LayerSettings) -> orleans_core::FillParams {
    orleans_core::FillParams {
        spacing: saved.fill_spacing.clamp(0.05, 10.0),
        angle: saved.fill_angle.clamp(0.0, 180.0),
        overlap: saved.fill_overlap.clamp(0.0, 5.0),
        alternate: saved.fill_alternate,
    }
}

fn tabs_from(saved: &LayerSettings) -> orleans_core::TabParams {
    orleans_core::TabParams {
        count: saved.tab_count.min(16),
        width: saved.tab_width.clamp(0.5, 20.0),
        inner: saved.tab_inner,
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
        layer.mode = LayerMode::Fill;
        layer.fill = orleans_core::FillParams {
            spacing: 0.4,
            angle: 45.0,
            overlap: 0.1,
            alternate: false,
        };
        layer.tabs = orleans_core::TabParams {
            count: 4,
            width: 2.5,
            inner: true,
        };
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
                matrix: None,
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
        assert_eq!(back.layers[0].mode, "Fill");
        assert!((back.layers[0].fill_spacing - 0.4).abs() < 1e-9);
        assert!((back.layers[0].fill_angle - 45.0).abs() < 1e-9);
        assert!((back.layers[0].fill_overlap - 0.1).abs() < 1e-9);
        assert!(!back.layers[0].fill_alternate);
        assert_eq!(back.layers[0].tab_count, 4);
        assert!((back.layers[0].tab_width - 2.5).abs() < 1e-9);
        assert!(back.layers[0].tab_inner);
        assert_eq!(back.transforms.len(), 1);
        assert_eq!(back.transforms[0].dx, 1.5);
        assert_eq!(back.transforms[0].dy, -2.0);
    }

    #[test]
    fn json_round_trip_keeps_groups() {
        let mut project = ProjectFile::new(
            "<svg/>".to_owned(),
            None,
            &Document::new(100.0, 100.0),
            0.0,
            true,
            LaserMode::M3,
            1000,
            Vec::new(),
        );
        project.groups = vec![GroupSettings {
            id: 4,
            name: "Fixture".to_owned(),
            members: vec![
                GroupMember {
                    layer: 0,
                    shape: "a".to_owned(),
                },
                GroupMember {
                    layer: 1,
                    shape: "b".to_owned(),
                },
            ],
        }];
        let json = serde_json::to_string(&project).expect("serialise");
        let back: ProjectFile = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.groups.len(), 1);
        assert_eq!(back.groups[0].id, 4);
        assert_eq!(back.groups[0].name, "Fixture");
        assert_eq!(back.groups[0].members.len(), 2);
        assert_eq!(back.groups[0].members[1].layer, 1);
        assert_eq!(back.groups[0].members[1].shape, "b");

        // Projects saved before groups were added still load.
        let mut value: serde_json::Value = serde_json::from_str(&json).expect("json value");
        value
            .as_object_mut()
            .expect("project object")
            .remove("groups");
        let old: ProjectFile = serde_json::from_value(value).expect("old project");
        assert!(old.groups.is_empty());
    }

    #[test]
    fn apply_moves_and_adds_shapes_between_layers() {
        use kurbo::BezPath;

        let mut doc = Document::new(100.0, 100.0);
        let mut source = Layer::new(0, "source", [0, 0, 0], CutParams::default());
        source.shapes.push(Shape::new(
            "moved",
            BezPath::from_svg("M0,0 L10,0").expect("path"),
            false,
        ));
        let target = Layer::new(1, "target", [255, 0, 0], CutParams::default());
        doc.layers.push(source);
        doc.layers.push(target);

        let mut project = ProjectFile::new(
            "<svg/>".to_owned(),
            None,
            &doc,
            0.0,
            true,
            LaserMode::M3,
            1000,
            Vec::new(),
        );
        project.moved_shapes = vec![MovedShape {
            source_layer: 0,
            source_shape: "moved".to_owned(),
            target_layer: 1,
            target_shape: "moved".to_owned(),
            path: Some("M0,0 L0,20".to_owned()),
            is_fill: false,
        }];
        project.extra_shapes = vec![ExtraShape {
            layer: 1,
            shape: "copy".to_owned(),
            path: "M5,5 L15,5".to_owned(),
            is_fill: true,
        }];

        let json = serde_json::to_string(&project).expect("serialise");
        let project: ProjectFile = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(project.moved_shapes.len(), 1);
        assert_eq!(project.extra_shapes.len(), 1);

        let mut loaded = doc.clone();
        project.apply(&mut loaded);
        assert!(loaded.layers[0].shapes.is_empty());
        assert_eq!(loaded.layers[1].shapes.len(), 2);
        let moved = loaded.layers[1]
            .shapes
            .iter()
            .find(|shape| shape.id == "moved")
            .expect("moved shape");
        assert!((moved.bounds().height() - 20.0).abs() < 1e-9);
        let copy = loaded.layers[1]
            .shapes
            .iter()
            .find(|shape| shape.id == "copy")
            .expect("copied shape");
        assert!(copy.is_fill);
        assert!((copy.bounds().width() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn json_round_trip_keeps_affine_matrix() {
        use kurbo::{Affine, Point};
        let affine = Affine::translate((5.0, 3.0)) * Affine::rotate(0.5) * Affine::scale(2.0);
        let expected = [
            affine.as_coeffs()[0],
            affine.as_coeffs()[1],
            affine.as_coeffs()[2],
            affine.as_coeffs()[3],
            affine.as_coeffs()[4],
            affine.as_coeffs()[5],
        ];
        let project = ProjectFile::new(
            "<svg/>".to_owned(),
            None,
            &Document::new(100.0, 100.0),
            0.0,
            true,
            LaserMode::M4,
            1000,
            vec![ShapeTransform {
                layer: 0,
                shape: "shape".to_owned(),
                matrix: Some(expected),
                dx: 0.0,
                dy: 0.0,
            }],
        );
        let json = serde_json::to_string(&project).expect("serialise");
        let back: ProjectFile = serde_json::from_str(&json).expect("deserialise");
        let stored = back.transforms[0].matrix.expect("matrix");
        for (actual, expected) in stored.iter().zip(expected.iter()) {
            assert!((actual - expected).abs() < 1e-12);
        }
        let restored = Affine::new(stored) * Point::new(1.0, 2.0);
        let original = affine * Point::new(1.0, 2.0);
        assert!((restored - original).hypot() < 1e-12);
    }

    #[test]
    fn raster_layers_round_trip_into_empty_documents() {
        let mut doc = Document::new(20.0, 10.0);
        let mut layer = Layer::new(0, "photo", [0, 0, 0], CutParams::new(900.0, 30.0, 1));
        layer.mode = LayerMode::Raster;
        layer.raster = Some(orleans_core::RasterImage {
            width: 2,
            height: 1,
            grayscale: vec![0, 255],
            x: 1.0,
            y: 2.0,
            width_mm: 10.0,
            height_mm: 5.0,
            angle: 30.0,
            mode: orleans_core::RasterMode::Dither,
            threshold: 100,
            invert: true,
            line_interval: 0.2,
            bidirectional: false,
        });
        doc.layers.push(layer);
        let project = ProjectFile::new(
            String::new(),
            None,
            &doc,
            0.0,
            true,
            LaserMode::M4,
            1000,
            Vec::new(),
        );
        assert!(project.width_mm > 0.0);
        let json = serde_json::to_string(&project).expect("serialise");
        let back: ProjectFile = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.layers[0].mode, "Raster");
        let saved = back.layers[0].raster.as_ref().expect("raster settings");
        assert_eq!(saved.width, 2);
        assert_eq!(saved.height, 1);

        let mut loaded = Document::new(1.0, 1.0);
        back.apply(&mut loaded);
        let raster = loaded.layers[0].raster.as_ref().expect("raster image");
        assert_eq!(raster.grayscale, vec![0, 255]);
        assert_eq!(raster.mode, orleans_core::RasterMode::Dither);
        assert!(raster.invert);
        assert!((raster.width_mm - 10.0).abs() < 1e-9);
        assert!((raster.angle - 30.0).abs() < 1e-9);
        assert!((loaded.width - 20.0).abs() < 1e-9);
    }

    #[test]
    fn old_projects_without_edited_paths_still_load() {
        let json = r#"{
            "format": 1,
            "svg": "<svg/>",
            "layers": [],
            "kerf": 0.0,
            "optimize": true,
            "laser_mode": "M4",
            "max_s": 1000
        }"#;
        let project: ProjectFile = serde_json::from_str(json).expect("deserialise");
        assert!(project.transforms.is_empty());
        assert!(project.edited_paths.is_empty());
    }

    #[test]
    fn old_layer_settings_get_fill_defaults() {
        let json = r#"{
            "format": 1,
            "svg": "<svg/>",
            "layers": [{
                "index": 0,
                "name": "old",
                "color": [0, 0, 0],
                "mode": "Fill",
                "speed": 1000.0,
                "power": 20.0,
                "passes": 1
            }],
            "kerf": 0.0,
            "optimize": true,
            "laser_mode": "M4",
            "max_s": 1000
        }"#;
        let project: ProjectFile = serde_json::from_str(json).expect("deserialise");
        let layer = &project.layers[0];
        assert!(layer.visible);
        assert!(layer.output);
        assert!((layer.fill_spacing - 0.25).abs() < 1e-9);
        assert!((layer.fill_angle).abs() < 1e-9);
        assert!((layer.fill_overlap).abs() < 1e-9);
        assert!(layer.fill_alternate);
        assert_eq!(layer.tab_count, 0);
        assert!((layer.tab_width - 3.0).abs() < 1e-9);
        assert!(!layer.tab_inner);
    }

    #[test]
    fn json_round_trip_keeps_edited_paths() {
        let mut project = ProjectFile::new(
            "<svg/>".to_owned(),
            None,
            &Document::new(100.0, 100.0),
            0.0,
            true,
            LaserMode::M4,
            1000,
            Vec::new(),
        );
        project.edited_paths = vec![EditedPath {
            layer: 3,
            shape: "path-7".to_owned(),
            path: "M0,0 L10,10 Z".to_owned(),
        }];
        let json = serde_json::to_string(&project).expect("serialise");
        let back: ProjectFile = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.edited_paths.len(), 1);
        assert_eq!(back.edited_paths[0].layer, 3);
        assert_eq!(back.edited_paths[0].shape, "path-7");
        assert_eq!(back.edited_paths[0].path, "M0,0 L10,10 Z");
    }

    #[test]
    fn edited_paths_override_geometry_after_transforms() {
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
            width_mm: 100.0,
            height_mm: 100.0,
            transforms: vec![ShapeTransform {
                layer: 0,
                shape: "line".to_owned(),
                matrix: None,
                dx: 5.0,
                dy: 0.0,
            }],
            edited_paths: vec![EditedPath {
                layer: 0,
                shape: "line".to_owned(),
                path: "M0,0 L0,20".to_owned(),
            }],
            groups: Vec::new(),
            moved_shapes: Vec::new(),
            extra_shapes: Vec::new(),
        };
        project.apply(&mut doc);
        let bounds = doc.layers[0].shapes[0].path.bounding_box();
        assert!((bounds.x0 - 0.0).abs() < 1e-9);
        assert!((bounds.x1 - 0.0).abs() < 1e-9);
        assert!((bounds.y0 - 0.0).abs() < 1e-9);
        assert!((bounds.y1 - 20.0).abs() < 1e-9);
    }

    #[test]
    fn apply_matrix_transforms() {
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
            width_mm: 100.0,
            height_mm: 100.0,
            transforms: vec![ShapeTransform {
                layer: 0,
                shape: "line".to_owned(),
                matrix: Some(kurbo::Affine::scale(2.0).as_coeffs()),
                dx: 0.0,
                dy: 0.0,
            }],
            edited_paths: Vec::new(),
            groups: Vec::new(),
            moved_shapes: Vec::new(),
            extra_shapes: Vec::new(),
        };
        project.apply(&mut doc);
        let bounds = doc.layers[0].shapes[0].path.bounding_box();
        assert!((bounds.x1 - 20.0).abs() < 1e-6);
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
            width_mm: 100.0,
            height_mm: 100.0,
            transforms: vec![ShapeTransform {
                layer: 0,
                shape: "line".to_owned(),
                matrix: None,
                dx: 5.0,
                dy: 2.0,
            }],
            edited_paths: Vec::new(),
            groups: Vec::new(),
            moved_shapes: Vec::new(),
            extra_shapes: Vec::new(),
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
            fill_spacing: 0.25,
            fill_angle: 0.0,
            fill_overlap: 0.0,
            fill_alternate: true,
            raster: None,
            tab_count: 0,
            tab_width: 3.0,
            tab_inner: false,
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
            width_mm: 100.0,
            height_mm: 100.0,
            transforms: Vec::new(),
            edited_paths: Vec::new(),
            groups: Vec::new(),
            moved_shapes: Vec::new(),
            extra_shapes: Vec::new(),
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
                fill_spacing: 0.4,
                fill_angle: 45.0,
                fill_overlap: 0.1,
                fill_alternate: false,
                raster: None,
                tab_count: 3,
                tab_width: 1.5,
                tab_inner: true,
            }],
            kerf: 0.1,
            optimize: false,
            laser_mode: "M3".to_owned(),
            max_s: 500,
            width_mm: 100.0,
            height_mm: 80.0,
            transforms: Vec::new(),
            edited_paths: Vec::new(),
            groups: Vec::new(),
            moved_shapes: Vec::new(),
            extra_shapes: Vec::new(),
        };
        project.apply(&mut doc);
        let layer = &doc.layers[0];
        assert_eq!(layer.name, "holes");
        assert_eq!(layer.color, [255, 0, 0]);
        assert_eq!(layer.cut.speed, 800.0);
        assert_eq!(layer.cut.power, 12.5);
        assert_eq!(layer.cut.passes, 3);
        assert!((layer.fill.spacing - 0.4).abs() < 1e-9);
        assert!((layer.fill.angle - 45.0).abs() < 1e-9);
        assert!((layer.fill.overlap - 0.1).abs() < 1e-9);
        assert!(!layer.fill.alternate);
        assert_eq!(layer.tabs.count, 3);
        assert!((layer.tabs.width - 1.5).abs() < 1e-9);
        assert!(layer.tabs.inner);
        assert!(!layer.visible);
        assert!(layer.output);
    }
}
