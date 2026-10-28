//! Orleans project files.
//!
//! Projects are a canonical snapshot of the editor document: every layer and
//! shape stores its current geometry, settings and groups. Loading does not
//! re-import an embedded SVG or replay edits, so layer/shape identity and
//! imported geometry are stable.

use std::io::Cursor;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use kurbo::BezPath;
use orleans_core::{
    CutParams, Document, FillParams, Layer, LayerMode, ManualTabs, RasterCrop, RasterImage,
    RasterMode, Shape, TabParams,
};
use orleans_driver::gcode::LaserMode;
use serde::{Deserialize, Serialize};

pub const FORMAT: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectFile {
    pub format: u32,
    /// Original source path, kept for display only.
    #[serde(default)]
    pub source: Option<String>,
    pub width_mm: f64,
    pub height_mm: f64,
    pub kerf: f64,
    pub optimize: bool,
    pub laser_mode: String,
    pub max_s: u32,
    pub layers: Vec<LayerData>,
    #[serde(default)]
    pub groups: Vec<GroupSettings>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayerData {
    pub index: usize,
    pub name: String,
    pub color: [u8; 3],
    pub mode: String,
    pub visible: bool,
    pub output: bool,
    pub cut: CutData,
    pub fill: FillData,
    pub tabs: TabData,
    #[serde(default)]
    pub manual_tabs: Vec<ManualTabsData>,
    #[serde(default)]
    pub raster: Option<RasterData>,
    #[serde(default)]
    pub shapes: Vec<ShapeData>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct CutData {
    pub speed: f64,
    pub power: f64,
    pub passes: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct FillData {
    pub spacing: f64,
    pub angle: f64,
    pub overlap: f64,
    pub alternate: bool,
    pub cross: bool,
    pub outline: bool,
    pub offset: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct TabData {
    pub count: u32,
    pub width: f64,
    pub inner: bool,
    pub manual: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManualTabsData {
    pub shape: String,
    pub contour: usize,
    #[serde(default)]
    pub positions: Vec<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShapeData {
    pub id: String,
    /// SVG path data in document coordinates.
    pub path: String,
    #[serde(default)]
    pub is_fill: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RasterData {
    pub width: u32,
    pub height: u32,
    /// Base64-encoded 8-bit grayscale PNG.
    pub png: String,
    pub x: f64,
    pub y: f64,
    pub width_mm: f64,
    pub height_mm: f64,
    pub angle: f64,
    pub mode: String,
    pub threshold: u8,
    pub invert: bool,
    pub line_interval: f64,
    pub bidirectional: bool,
    #[serde(default)]
    pub crop: Option<CropData>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct CropData {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
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

impl ProjectFile {
    #[allow(clippy::too_many_arguments)]
    pub fn from_document(
        doc: &Document,
        source: Option<String>,
        kerf: f64,
        optimize: bool,
        laser_mode: LaserMode,
        max_s: u32,
    ) -> Result<Self, String> {
        let layers = doc
            .layers
            .iter()
            .map(LayerData::from_layer)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            format: FORMAT,
            source,
            width_mm: doc.width.max(1.0),
            height_mm: doc.height.max(1.0),
            kerf,
            optimize,
            laser_mode: match laser_mode {
                LaserMode::M3 => "M3",
                LaserMode::M4 => "M4",
            }
            .to_owned(),
            max_s,
            layers,
            groups: Vec::new(),
        })
    }

    pub fn to_document(&self) -> Result<Document, String> {
        let mut doc = Document::new(self.width_mm.max(1.0), self.height_mm.max(1.0));
        for saved in &self.layers {
            let mut layer = Layer::new(
                saved.index,
                saved.name.clone(),
                saved.color,
                CutParams::new(saved.cut.speed, saved.cut.power, saved.cut.passes),
            );
            layer.mode = parse_mode(&saved.mode);
            layer.visible = saved.visible;
            layer.output = saved.output;
            layer.fill = FillParams {
                spacing: saved.fill.spacing.clamp(0.05, 10.0),
                angle: saved.fill.angle,
                overlap: saved.fill.overlap.clamp(0.0, 5.0),
                alternate: saved.fill.alternate,
                cross: saved.fill.cross,
                outline: saved.fill.outline,
                offset: saved.fill.offset,
            };
            layer.tabs = TabParams {
                count: saved.tabs.count.min(16),
                width: saved.tabs.width.clamp(0.5, 20.0),
                inner: saved.tabs.inner,
                manual: saved.tabs.manual,
            };
            layer.manual_tabs = saved
                .manual_tabs
                .iter()
                .map(|tabs| ManualTabs {
                    shape: tabs.shape.clone(),
                    contour: tabs.contour,
                    positions: tabs.positions.iter().map(|p| p.rem_euclid(1.0)).collect(),
                })
                .collect();
            if let Some(raster) = &saved.raster {
                layer.raster = Some(raster.to_image()?);
            }
            for shape in &saved.shapes {
                let path = BezPath::from_svg(&shape.path).map_err(|err| {
                    format!(
                        "layer {} shape {} has an invalid path: {err}",
                        saved.index, shape.id
                    )
                })?;
                if path.elements().is_empty() {
                    continue;
                }
                layer
                    .shapes
                    .push(Shape::new(shape.id.clone(), path, shape.is_fill));
            }
            doc.layers.push(layer);
        }
        Ok(doc)
    }

    pub fn laser_mode(&self) -> LaserMode {
        if self.laser_mode.eq_ignore_ascii_case("M4") {
            LaserMode::M4
        } else {
            LaserMode::M3
        }
    }
}

impl LayerData {
    fn from_layer(layer: &Layer) -> Result<Self, String> {
        let raster = layer
            .raster
            .as_ref()
            .map(RasterData::from_image)
            .transpose()?;
        Ok(Self {
            index: layer.index,
            name: layer.name.clone(),
            color: layer.color,
            mode: format!("{:?}", layer.mode),
            visible: layer.visible,
            output: layer.output,
            cut: CutData {
                speed: layer.cut.speed,
                power: layer.cut.power,
                passes: layer.cut.passes,
            },
            fill: FillData {
                spacing: layer.fill.spacing,
                angle: layer.fill.angle,
                overlap: layer.fill.overlap,
                alternate: layer.fill.alternate,
                cross: layer.fill.cross,
                outline: layer.fill.outline,
                offset: layer.fill.offset,
            },
            tabs: TabData {
                count: layer.tabs.count,
                width: layer.tabs.width,
                inner: layer.tabs.inner,
                manual: layer.tabs.manual,
            },
            manual_tabs: layer
                .manual_tabs
                .iter()
                .map(|tabs| ManualTabsData {
                    shape: tabs.shape.clone(),
                    contour: tabs.contour,
                    positions: tabs.positions.clone(),
                })
                .collect(),
            raster,
            shapes: layer
                .shapes
                .iter()
                .map(|shape| ShapeData {
                    id: shape.id.clone(),
                    path: shape.path.to_svg(),
                    is_fill: shape.is_fill,
                })
                .collect(),
        })
    }
}

impl RasterData {
    fn from_image(image: &RasterImage) -> Result<Self, String> {
        let gray = image::GrayImage::from_raw(image.width, image.height, image.grayscale.to_vec())
            .ok_or_else(|| "raster dimensions do not match its pixels".to_owned())?;
        let mut png = Vec::new();
        image::DynamicImage::ImageLuma8(gray)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|err| format!("encoding raster PNG: {err}"))?;
        Ok(Self {
            width: image.width,
            height: image.height,
            png: STANDARD.encode(png),
            x: image.x,
            y: image.y,
            width_mm: image.width_mm,
            height_mm: image.height_mm,
            angle: image.angle,
            mode: match image.mode {
                RasterMode::Threshold => "Threshold",
                RasterMode::Dither => "Dither",
                RasterMode::Atkinson => "Atkinson",
                RasterMode::Bayer => "Bayer",
                RasterMode::Grayscale => "Grayscale",
            }
            .to_owned(),
            threshold: image.threshold,
            invert: image.invert,
            line_interval: image.line_interval,
            bidirectional: image.bidirectional,
            crop: image.crop.map(|crop| CropData {
                x: crop.x,
                y: crop.y,
                width: crop.width,
                height: crop.height,
            }),
        })
    }

    fn to_image(&self) -> Result<RasterImage, String> {
        let bytes = STANDARD
            .decode(&self.png)
            .map_err(|err| format!("decoding raster base64: {err}"))?;
        let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .map_err(|err| format!("decoding raster PNG: {err}"))?
            .to_luma8();
        if image.width() != self.width || image.height() != self.height {
            return Err("raster PNG dimensions do not match the project".to_owned());
        }
        Ok(RasterImage {
            width: self.width,
            height: self.height,
            grayscale: image.into_raw().into(),
            x: self.x,
            y: self.y,
            width_mm: self.width_mm,
            height_mm: self.height_mm,
            angle: self.angle,
            mode: parse_raster_mode(&self.mode),
            threshold: self.threshold,
            invert: self.invert,
            line_interval: self.line_interval,
            bidirectional: self.bidirectional,
            crop: self.crop.map(|crop| RasterCrop {
                x: crop.x,
                y: crop.y,
                width: crop.width,
                height: crop.height,
            }),
        })
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

fn parse_raster_mode(mode: &str) -> RasterMode {
    if mode.eq_ignore_ascii_case("dither") {
        RasterMode::Dither
    } else if mode.eq_ignore_ascii_case("atkinson") {
        RasterMode::Atkinson
    } else if mode.eq_ignore_ascii_case("bayer") {
        RasterMode::Bayer
    } else if mode.eq_ignore_ascii_case("grayscale") {
        RasterMode::Grayscale
    } else {
        RasterMode::Threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape as _;
    use orleans_core::{Layer, Shape};

    fn sample_document() -> Document {
        let mut doc = Document::new(120.0, 80.0);
        let mut cut = Layer::new(3, "Cut", [200, 10, 10], CutParams::new(1200.0, 35.0, 2));
        cut.mode = LayerMode::Fill;
        cut.fill.cross = true;
        cut.fill.outline = true;
        cut.fill.offset = true;
        cut.tabs = TabParams {
            count: 3,
            width: 2.5,
            inner: true,
            manual: true,
        };
        cut.manual_tabs.push(ManualTabs {
            shape: "s1".to_owned(),
            contour: 0,
            positions: vec![0.25, 0.75],
        });
        cut.shapes.push(Shape::new(
            "s1",
            BezPath::from_svg("M0,0 L10,0 L10,10 Z").expect("path"),
            true,
        ));
        doc.layers.push(cut);

        let mut image = Layer::new(7, "Image", [0, 0, 0], CutParams::new(900.0, 20.0, 1));
        image.mode = LayerMode::Raster;
        image.visible = false;
        image.raster = Some(RasterImage {
            width: 2,
            height: 2,
            grayscale: vec![0, 64, 128, 255].into(),
            x: 4.0,
            y: 5.0,
            width_mm: 20.0,
            height_mm: 10.0,
            angle: 15.0,
            mode: RasterMode::Bayer,
            threshold: 90,
            invert: true,
            line_interval: 0.2,
            bidirectional: false,
            crop: Some(RasterCrop {
                x: 1,
                y: 0,
                width: 1,
                height: 2,
            }),
        });
        doc.layers.push(image);
        doc
    }

    #[test]
    fn canonical_project_round_trips_the_document() {
        let doc = sample_document();
        let mut project = ProjectFile::from_document(
            &doc,
            Some("/tmp/source.svg".to_owned()),
            0.15,
            false,
            LaserMode::M4,
            750,
        )
        .expect("project");
        project.groups = vec![GroupSettings {
            id: 4,
            name: "Fixture".to_owned(),
            members: vec![GroupMember {
                layer: 3,
                shape: "s1".to_owned(),
            }],
        }];

        let json = serde_json::to_string_pretty(&project).expect("serialise");
        let back: ProjectFile = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.format, FORMAT);
        assert_eq!(back.laser_mode(), LaserMode::M4);
        assert_eq!(back.kerf, 0.15);
        assert!(!back.optimize);
        assert_eq!(back.groups.len(), 1);
        assert_eq!(back.groups[0].members[0].layer, 3);

        let loaded = back.to_document().expect("document");
        assert_eq!(loaded.width, 120.0);
        assert_eq!(loaded.height, 80.0);
        assert_eq!(loaded.layers.len(), 2);

        let cut = &loaded.layers[0];
        assert_eq!(cut.index, 3);
        assert_eq!(cut.name, "Cut");
        assert_eq!(cut.mode, LayerMode::Fill);
        assert!(cut.fill.cross && cut.fill.outline && cut.fill.offset);
        assert_eq!(cut.tabs.count, 3);
        assert!(cut.tabs.manual);
        assert_eq!(cut.manual_tabs.len(), 1);
        assert_eq!(cut.manual_tabs[0].positions, vec![0.25, 0.75]);
        assert_eq!(cut.shapes.len(), 1);
        assert_eq!(
            cut.shapes[0].path.bounding_box(),
            doc.layers[0].shapes[0].path.bounding_box()
        );

        let image = &loaded.layers[1];
        assert_eq!(image.index, 7);
        assert!(!image.visible);
        let raster = image.raster.as_ref().expect("raster");
        assert_eq!(raster.grayscale.as_ref(), &[0, 64, 128, 255]);
        assert_eq!(raster.mode, RasterMode::Bayer);
        assert_eq!(
            raster.crop,
            Some(RasterCrop {
                x: 1,
                y: 0,
                width: 1,
                height: 2,
            })
        );
    }

    #[test]
    fn layer_order_is_the_saved_cut_order() {
        let doc = sample_document();
        let project = ProjectFile::from_document(&doc, None, 0.0, true, LaserMode::M3, 1000)
            .expect("project");
        let loaded = project.to_document().expect("document");
        assert_eq!(
            loaded
                .layers
                .iter()
                .map(|layer| layer.index)
                .collect::<Vec<_>>(),
            vec![3, 7]
        );
    }

    #[test]
    fn invalid_shape_paths_are_reported() {
        let doc = Document::new(10.0, 10.0);
        let mut project = ProjectFile::from_document(&doc, None, 0.0, true, LaserMode::M3, 1000)
            .expect("project");
        project.layers.push(LayerData {
            index: 0,
            name: "bad".to_owned(),
            color: [0, 0, 0],
            mode: "Line".to_owned(),
            visible: true,
            output: true,
            cut: CutData {
                speed: 1000.0,
                power: 20.0,
                passes: 1,
            },
            fill: FillData {
                spacing: 0.25,
                angle: 0.0,
                overlap: 0.0,
                alternate: true,
                cross: false,
                outline: false,
                offset: false,
            },
            tabs: TabData {
                count: 0,
                width: 3.0,
                inner: false,
                manual: false,
            },
            manual_tabs: Vec::new(),
            raster: None,
            shapes: vec![ShapeData {
                id: "bad".to_owned(),
                path: "not a path".to_owned(),
                is_fill: false,
            }],
        });
        assert!(project.to_document().is_err());
    }
}
