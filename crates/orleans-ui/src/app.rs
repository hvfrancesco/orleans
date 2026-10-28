//! The main application window: toolbar, layer panel, canvas, status bar.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, StrokeKind};
use kurbo::{Affine, Point};
use orleans_core::{Document, Job, LayerMode, Op, RasterMode};
use orleans_driver::gcode::{self, GcodeOptions, LaserMode};
use orleans_geom::{flatten_path, hatch_contours, point_in_polygon, HatchOptions};
use orleans_import::{import_image, import_svg, ImportOptions, DEFAULT_RASTER_DPI};
use orleans_toolpath::{plan, PlanOptions};

use crate::machine::{Event as MachineEvent, Machine, Settings, Status as MachineStatus};
use crate::nodes::{self, NodeKind, NodeRef};
use crate::preview::Preview;
use crate::project::{EditedPath, GroupMember, GroupSettings, ProjectFile, ShapeTransform, FORMAT};
use crate::viewport::Viewport;

const DISPLAY_TOLERANCE: f64 = 0.05;

struct Status {
    text: String,
    error: bool,
}

impl Status {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: false,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ShapeRef {
    layer: usize,
    shape: String,
}

type ShapeTransforms = BTreeMap<(usize, String), Affine>;

/// A set of shapes selected and transformed together by the editor. Groups are
/// editor metadata: every member still cuts with its own layer settings.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ShapeGroup {
    id: u64,
    name: String,
    members: BTreeSet<ShapeRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AlignAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AlignEdge {
    Start,
    Center,
    End,
}

/// One alignment/distribution target: an ungrouped shape, or all members of a
/// selected group.
#[derive(Clone, Debug)]
struct SelectionUnit {
    members: Vec<ShapeRef>,
    bounds: kurbo::Rect,
}

const UNDO_LIMIT: usize = 30;

/// Editable state captured before an edit. Each section is `None` unless that
/// kind of state is restored by the entry.
#[derive(Default)]
struct UndoSnapshot {
    shapes: Vec<(usize, String, kurbo::BezPath)>,
    transforms: Option<ShapeTransforms>,
    edited_paths: Option<BTreeSet<(usize, String)>>,
    rasters: Vec<RasterSnapshot>,
    settings: Option<SettingsSnapshot>,
    groups: Option<Vec<ShapeGroup>>,
}

#[derive(Clone, Copy, Debug)]
struct RasterSnapshot {
    layer: usize,
    x: f64,
    y: f64,
    width_mm: f64,
    height_mm: f64,
    angle: f64,
}

/// Display and cutting parameters of one layer. The geometry and raster pixels
/// are deliberately left out: those are captured separately from the edit that
/// changes them.
#[derive(Clone, Debug)]
struct LayerSettingsSnapshot {
    index: usize,
    name: String,
    color: [u8; 3],
    mode: LayerMode,
    cut: orleans_core::CutParams,
    fill: orleans_core::FillParams,
    tabs: orleans_core::TabParams,
    visible: bool,
    output: bool,
    raster: Option<RasterSettingsSnapshot>,
}

#[derive(Clone, Copy, Debug)]
struct RasterSettingsSnapshot {
    x: f64,
    y: f64,
    width_mm: f64,
    height_mm: f64,
    angle: f64,
    mode: RasterMode,
    threshold: u8,
    invert: bool,
    line_interval: f64,
    bidirectional: bool,
}

/// Everything the side panel can change: per-layer settings, cut order and the
/// job options. Cheap to clone because no path or pixel data is included.
#[derive(Clone, Debug)]
struct SettingsSnapshot {
    layers: Vec<LayerSettingsSnapshot>,
    order: Vec<usize>,
    plan: PlanOptions,
    gcode: GcodeOptions,
}

impl SettingsSnapshot {
    fn capture(doc: &Document, plan: &PlanOptions, gcode: &GcodeOptions) -> Self {
        Self {
            layers: doc
                .layers
                .iter()
                .map(|layer| LayerSettingsSnapshot {
                    index: layer.index,
                    name: layer.name.clone(),
                    color: layer.color,
                    mode: layer.mode,
                    cut: layer.cut,
                    fill: layer.fill,
                    tabs: layer.tabs,
                    visible: layer.visible,
                    output: layer.output,
                    raster: layer.raster.as_ref().map(|raster| RasterSettingsSnapshot {
                        x: raster.x,
                        y: raster.y,
                        width_mm: raster.width_mm,
                        height_mm: raster.height_mm,
                        angle: raster.angle,
                        mode: raster.mode,
                        threshold: raster.threshold,
                        invert: raster.invert,
                        line_interval: raster.line_interval,
                        bidirectional: raster.bidirectional,
                    }),
                })
                .collect(),
            order: doc.layers.iter().map(|layer| layer.index).collect(),
            plan: plan.clone(),
            gcode: gcode.clone(),
        }
    }

    fn apply(self, doc: &mut Document, plan: &mut PlanOptions, gcode: &mut GcodeOptions) {
        for layer in &mut doc.layers {
            let Some(saved) = self.layers.iter().find(|saved| saved.index == layer.index) else {
                continue;
            };
            layer.name.clone_from(&saved.name);
            layer.color = saved.color;
            layer.mode = saved.mode;
            layer.cut = saved.cut;
            layer.fill = saved.fill;
            layer.tabs = saved.tabs;
            layer.visible = saved.visible;
            layer.output = saved.output;
            if let (Some(image), Some(saved)) = (layer.raster.as_mut(), saved.raster.as_ref()) {
                image.x = saved.x;
                image.y = saved.y;
                image.width_mm = saved.width_mm;
                image.height_mm = saved.height_mm;
                image.angle = saved.angle;
                image.mode = saved.mode;
                image.threshold = saved.threshold;
                image.invert = saved.invert;
                image.line_interval = saved.line_interval;
                image.bidirectional = saved.bidirectional;
            }
        }
        let order = self.order;
        if order.len() == doc.layers.len()
            && doc.layers.iter().all(|layer| order.contains(&layer.index))
        {
            doc.layers
                .sort_by_key(|layer| order.iter().position(|&index| index == layer.index));
        }
        *plan = self.plan;
        *gcode = self.gcode;
    }
}

#[derive(Clone, Copy, Debug)]
struct NodeDrag {
    node: NodeRef,
    start_world: Point,
    start_node: Point,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandleCorner {
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    Scale(HandleCorner),
    Rotate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectionAnchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl SelectionAnchor {
    const ALL: [Self; 9] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Left,
        Self::Center,
        Self::Right,
        Self::BottomLeft,
        Self::Bottom,
        Self::BottomRight,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::TopLeft => "Top left",
            Self::Top => "Top",
            Self::TopRight => "Top right",
            Self::Left => "Left",
            Self::Center => "Centre",
            Self::Right => "Right",
            Self::BottomLeft => "Bottom left",
            Self::Bottom => "Bottom",
            Self::BottomRight => "Bottom right",
        }
    }

    fn point(self, bounds: kurbo::Rect) -> Point {
        let x = match self {
            Self::TopLeft | Self::Left | Self::BottomLeft => bounds.x0,
            Self::Top | Self::Center | Self::Bottom => bounds.center().x,
            Self::TopRight | Self::Right | Self::BottomRight => bounds.x1,
        };
        let y = match self {
            Self::TopLeft | Self::Top | Self::TopRight => bounds.y1,
            Self::Left | Self::Center | Self::Right => bounds.center().y,
            Self::BottomLeft | Self::Bottom | Self::BottomRight => bounds.y0,
        };
        Point::new(x, y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RasterHandle {
    Corner(HandleCorner),
    Rotate,
}

#[derive(Clone, Copy, Debug)]
enum RasterDrag {
    Move {
        layer: usize,
        start_pointer: Point,
        start_x: f64,
        start_y: f64,
    },
    Resize {
        layer: usize,
        corner: HandleCorner,
        anchor_world: Point,
        anchor_local: Point,
        initial_center: Point,
        angle: f64,
    },
    Rotate {
        layer: usize,
        center: Point,
        start_angle: f64,
        initial_angle: f64,
    },
}

#[derive(Clone, Copy, Debug)]
enum ActiveHandle {
    Scale { anchor: Point, last_distance: f64 },
    Rotate { center: Point, last_angle: f64 },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LinkState {
    Disconnected,
    Connecting,
    Reconnecting,
    Connected,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Open,
    Export,
    Reload,
    Fit,
    Send,
    SaveProject,
    Undo,
    Redo,
    ExitNodes,
    DeleteNode,
}

pub struct OrleansApp {
    doc: Document,
    preview: Preview,
    selected_layer: Option<usize>,
    selected_shapes: BTreeSet<ShapeRef>,
    groups: Vec<ShapeGroup>,
    shape_transforms: ShapeTransforms,
    undo_stack: Vec<UndoSnapshot>,
    redo_stack: Vec<UndoSnapshot>,
    /// Settings as they were before the current pointer/keyboard edit gesture.
    settings_edit: Option<SettingsSnapshot>,
    settings_edit_changed: bool,
    dragging_shape: bool,
    transform_dx: f64,
    transform_dy: f64,
    transform_scale: f64,
    transform_angle: f64,
    move_anchor: SelectionAnchor,
    move_to_x: f64,
    move_to_y: f64,
    move_sync: Option<(BTreeSet<ShapeRef>, SelectionAnchor, Option<[u64; 2]>)>,
    active_handle: Option<ActiveHandle>,
    selected_raster: Option<usize>,
    raster_drag: Option<RasterDrag>,
    node_edit: Option<ShapeRef>,
    selected_nodes: BTreeSet<NodeRef>,
    node_drag: Option<NodeDrag>,
    node_snap: bool,
    node_snap_geometry: bool,
    edited_paths: BTreeSet<(usize, String)>,
    raster_textures: BTreeMap<usize, egui::TextureHandle>,
    raster_texture_keys: BTreeMap<usize, (u32, u32, bool, [u8; 3])>,
    job: Job,
    path: Option<PathBuf>,
    view: Viewport,
    plan_opts: PlanOptions,
    gcode_opts: GcodeOptions,
    status: Status,
    /// Layer or job settings changed; re-plan before the next use.
    dirty: bool,
    fit_pending: bool,
    show_grid: bool,
    show_travel: bool,
    show_bed: bool,

    endpoint: String,
    machine: Machine,
    link: LinkState,
    machine_status: Option<MachineStatus>,
    /// Set right after asking for homing, to debounce the button.
    home_pending_until: Option<Instant>,
    confirm_home: bool,
    reconnect_attempt: u32,
    job_running: bool,
    job_done: usize,
    job_total: usize,
    confirm_send: bool,
    jog_step: f64,
    jog_feed: f64,
    jog_pending_until: Option<Instant>,
    homing: bool,
    saw_home: bool,
    frame_job: bool,
    job_paused: bool,
    job_uses_m4: bool,
    settings: Option<Settings>,
    source_svg: Option<String>,
    project_path: Option<PathBuf>,
    settings_open: bool,
    setting_laser: bool,
    setting_max_s: u32,
    setting_homing: bool,
}

impl OrleansApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        initial: Option<PathBuf>,
        endpoint: Option<String>,
    ) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let mut app = Self {
            doc: Document::new(100.0, 100.0),
            preview: Preview::default(),
            selected_layer: None,
            selected_shapes: BTreeSet::new(),
            groups: Vec::new(),
            shape_transforms: ShapeTransforms::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            settings_edit: None,
            settings_edit_changed: false,
            dragging_shape: false,
            transform_dx: 0.0,
            transform_dy: 0.0,
            transform_scale: 1.0,
            transform_angle: 0.0,
            move_anchor: SelectionAnchor::BottomLeft,
            move_to_x: 0.0,
            move_to_y: 0.0,
            move_sync: None,
            active_handle: None,
            selected_raster: None,
            raster_drag: None,
            node_edit: None,
            selected_nodes: BTreeSet::new(),
            node_drag: None,
            node_snap: false,
            node_snap_geometry: true,
            edited_paths: BTreeSet::new(),
            raster_textures: BTreeMap::new(),
            raster_texture_keys: BTreeMap::new(),
            job: Job::default(),
            path: None,
            view: Viewport::default(),
            plan_opts: PlanOptions::default(),
            // The GUI defaults to velocity-scaled power: it is the safer
            // choice for pausing and is what most diode controllers expect.
            gcode_opts: GcodeOptions {
                laser_mode: LaserMode::M4,
                ..GcodeOptions::default()
            },
            status: Status::ok("Open an SVG, or drop one onto the window."),
            dirty: false,
            fit_pending: true,
            show_grid: true,
            show_travel: false,
            show_bed: true,
            endpoint: endpoint.unwrap_or_default(),
            machine: Machine::spawn(),
            link: LinkState::Disconnected,
            machine_status: None,
            home_pending_until: None,
            confirm_home: false,
            reconnect_attempt: 0,
            job_running: false,
            job_done: 0,
            job_total: 0,
            confirm_send: false,
            jog_step: 1.0,
            jog_feed: 1000.0,
            jog_pending_until: None,
            homing: false,
            saw_home: false,
            frame_job: false,
            job_paused: false,
            job_uses_m4: false,
            settings: None,
            source_svg: None,
            project_path: None,
            settings_open: false,
            setting_laser: true,
            setting_max_s: 1000,
            setting_homing: false,
        };
        if let Some(path) = initial {
            app.open_path(path);
        }
        app
    }

    fn load_svg(&mut self, path: PathBuf) {
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.status = Status::error(format!("reading {}: {err}", path.display()));
                return;
            }
        };
        match import_svg(&bytes, &ImportOptions::default()) {
            Ok(doc) if doc.is_empty() => {
                self.status = Status::error(format!("{} contains no paths", path.display()));
            }
            Ok(doc) => {
                self.preview = Preview::build(&doc, DISPLAY_TOLERANCE);
                self.doc = doc;
                self.selected_layer = self.doc.layers.first().map(|layer| layer.index);
                self.selected_raster = self
                    .doc
                    .layers
                    .first()
                    .and_then(|layer| layer.raster.as_ref().map(|_| layer.index));
                self.selected_shapes.clear();
                self.groups.clear();
                self.shape_transforms.clear();
                self.clear_undo();
                self.dragging_shape = false;
                self.active_handle = None;
                self.node_edit = None;
                self.selected_nodes.clear();
                self.node_drag = None;
                self.edited_paths.clear();
                self.raster_textures.clear();
                self.raster_texture_keys.clear();
                self.replan();
                self.status = Status::ok(format!(
                    "{}: {} layers, {} shapes, {} contours",
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string()),
                    self.doc.layers.len(),
                    self.doc.shape_count(),
                    self.preview.contour_count()
                ));
                self.path = Some(path);
                self.project_path = None;
                self.source_svg = Some(String::from_utf8_lossy(&bytes).into_owned());
                self.fit_pending = true;
                self.dirty = false;
            }
            Err(err) => {
                self.status = Status::error(format!("importing {}: {err}", path.display()));
            }
        }
    }

    fn load_image(&mut self, path: PathBuf) {
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.status = Status::error(format!("reading {}: {err}", path.display()));
                return;
            }
        };
        match import_image(&bytes, DEFAULT_RASTER_DPI) {
            Ok(mut doc) => {
                let name = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Image".to_owned());
                if let Some(layer) = doc.layers.first_mut() {
                    layer.name = name;
                }
                self.preview = Preview::build(&doc, DISPLAY_TOLERANCE);
                self.doc = doc;
                self.selected_layer = self.doc.layers.first().map(|layer| layer.index);
                self.selected_raster = self
                    .doc
                    .layers
                    .first()
                    .and_then(|layer| layer.raster.as_ref().map(|_| layer.index));
                self.selected_shapes.clear();
                self.groups.clear();
                self.shape_transforms.clear();
                self.clear_undo();
                self.dragging_shape = false;
                self.active_handle = None;
                self.node_edit = None;
                self.selected_nodes.clear();
                self.node_drag = None;
                self.edited_paths.clear();
                self.raster_textures.clear();
                self.raster_texture_keys.clear();
                self.replan();
                let (width, height) = self
                    .doc
                    .layers
                    .first()
                    .and_then(|layer| layer.raster.as_ref())
                    .map(|raster| (raster.width, raster.height))
                    .unwrap_or((0, 0));
                self.status = Status::ok(format!(
                    "{}: {width}×{height} px raster",
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string())
                ));
                self.path = Some(path);
                self.project_path = None;
                self.source_svg = None;
                self.fit_pending = true;
                self.dirty = false;
            }
            Err(err) => {
                self.set_status(format!("importing image {}: {err}", path.display()), true);
            }
        }
    }

    fn open_path(&mut self, path: PathBuf) {
        if is_project_path(&path) {
            self.load_project(path);
        } else if is_image_path(&path) {
            self.load_image(path);
        } else {
            self.load_svg(path);
        }
    }

    fn load_project(&mut self, path: PathBuf) {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                self.set_status(format!("reading {}: {err}", path.display()), true);
                return;
            }
        };
        let project: ProjectFile = match serde_json::from_str(&text) {
            Ok(project) => project,
            Err(err) => {
                self.set_status(format!("parsing {}: {err}", path.display()), true);
                return;
            }
        };
        if project.format != FORMAT {
            self.set_status(
                format!("project format {} is not supported", project.format),
                true,
            );
            return;
        }
        let mut doc = if project.svg.trim().is_empty() {
            Document::new(project.width_mm.max(1.0), project.height_mm.max(1.0))
        } else {
            match import_svg(project.svg.as_bytes(), &ImportOptions::default()) {
                Ok(doc) if !doc.is_empty() => doc,
                Ok(_) => {
                    self.set_status(format!("{} contains no paths", path.display()), true);
                    return;
                }
                Err(err) => {
                    self.set_status(format!("importing project SVG: {err}"), true);
                    return;
                }
            }
        };
        project.apply(&mut doc);
        if doc.is_empty() {
            self.set_status(format!("{} contains no geometry", path.display()), true);
            return;
        }
        self.preview = Preview::build(&doc, DISPLAY_TOLERANCE);
        self.doc = doc;
        self.selected_layer = self.doc.layers.first().map(|layer| layer.index);
        self.selected_raster = self
            .doc
            .layers
            .first()
            .and_then(|layer| layer.raster.as_ref().map(|_| layer.index));
        self.selected_shapes.clear();
        self.shape_transforms = project
            .transforms
            .iter()
            .map(|transform| {
                let affine = transform
                    .matrix
                    .map(Affine::new)
                    .unwrap_or_else(|| Affine::translate((transform.dx, transform.dy)));
                ((transform.layer, transform.shape.clone()), affine)
            })
            .collect();
        self.groups = project
            .groups
            .iter()
            .filter_map(|group| {
                let members: BTreeSet<ShapeRef> = group
                    .members
                    .iter()
                    .filter_map(|member| {
                        let layer = self
                            .doc
                            .layers
                            .iter()
                            .find(|layer| layer.index == member.layer)?;
                        layer
                            .shapes
                            .iter()
                            .any(|shape| shape.id == member.shape)
                            .then(|| ShapeRef {
                                layer: member.layer,
                                shape: member.shape.clone(),
                            })
                    })
                    .collect();
                (!members.is_empty()).then(|| ShapeGroup {
                    id: group.id,
                    name: group.name.clone(),
                    members,
                })
            })
            .collect();
        self.clear_undo();
        self.dragging_shape = false;
        self.active_handle = None;
        self.node_edit = None;
        self.selected_nodes.clear();
        self.node_drag = None;
        self.edited_paths = project
            .edited_paths
            .iter()
            .map(|edited| (edited.layer, edited.shape.clone()))
            .collect();
        self.raster_textures.clear();
        self.raster_texture_keys.clear();
        self.plan_opts.kerf = project.kerf;
        self.plan_opts.optimize = project.optimize;
        self.gcode_opts.laser_mode = project.laser_mode();
        self.gcode_opts.max_s = project.max_s;
        self.source_svg = Some(project.svg.clone());
        self.project_path = Some(path.clone());
        self.path = project
            .source
            .as_ref()
            .map(PathBuf::from)
            .filter(|source| source.exists());
        self.replan();
        self.fit_pending = true;
        self.dirty = false;
        self.set_status(format!("project: {}", path.display()), false);
    }

    fn save_project(&mut self) {
        let svg = self.source_svg.clone().unwrap_or_default();
        let default_name = self
            .display_path()
            .and_then(|path| path.file_stem())
            .map(|stem| format!("{}.orleans", stem.to_string_lossy()))
            .unwrap_or_else(|| "design.orleans".to_owned());
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Orleans project", &["orleans"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };
        let source = self.path.as_ref().map(|path| path.display().to_string());
        let transforms = self
            .shape_transforms
            .iter()
            .map(|((layer, shape), affine)| ShapeTransform {
                layer: *layer,
                shape: shape.clone(),
                matrix: Some(affine.as_coeffs()),
                dx: 0.0,
                dy: 0.0,
            })
            .collect();
        let edited_paths = self
            .edited_paths
            .iter()
            .filter_map(|(layer, shape)| {
                self.doc
                    .layers
                    .iter()
                    .find(|entry| entry.index == *layer)?
                    .shapes
                    .iter()
                    .find(|entry| entry.id == *shape)
                    .map(|entry| EditedPath {
                        layer: *layer,
                        shape: shape.clone(),
                        path: entry.path.to_svg(),
                    })
            })
            .collect();
        let mut project = ProjectFile::new(
            svg,
            source,
            &self.doc,
            self.plan_opts.kerf,
            self.plan_opts.optimize,
            self.gcode_opts.laser_mode,
            self.gcode_opts.max_s,
            transforms,
        );
        project.edited_paths = edited_paths;
        project.groups = self
            .groups
            .iter()
            .map(|group| GroupSettings {
                id: group.id,
                name: group.name.clone(),
                members: group
                    .members
                    .iter()
                    .map(|member| GroupMember {
                        layer: member.layer,
                        shape: member.shape.clone(),
                    })
                    .collect(),
            })
            .collect();
        let json = match serde_json::to_string_pretty(&project) {
            Ok(json) => json,
            Err(err) => {
                self.set_status(format!("serialising project: {err}"), true);
                return;
            }
        };
        match std::fs::write(&path, json) {
            Ok(()) => {
                self.project_path = Some(path.clone());
                self.set_status(format!("project saved: {}", path.display()), false);
            }
            Err(err) => {
                self.set_status(format!("writing {}: {err}", path.display()), true);
            }
        }
    }

    fn display_path(&self) -> Option<&std::path::Path> {
        self.project_path
            .as_ref()
            .or(self.path.as_ref())
            .map(PathBuf::as_path)
    }

    fn replan(&mut self) {
        self.job = plan(&self.doc, &self.plan_opts);
    }

    fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = if error {
            Status::error(text)
        } else {
            Status::ok(text)
        };
    }

    fn pump_machine(&mut self, ctx: &egui::Context) {
        if self.homing
            && self
                .home_pending_until
                .map(|until| until <= Instant::now())
                .unwrap_or(true)
        {
            self.homing = false;
        }
        while let Some(event) = self.machine.try_recv() {
            match event {
                MachineEvent::Connecting(endpoint) => {
                    self.link = LinkState::Connecting;
                    self.reconnect_attempt = 0;
                    self.machine_status = None;
                    self.settings = None;
                    self.set_status(format!("connecting to {endpoint}…"), false);
                }
                MachineEvent::Connected(endpoint) => {
                    self.link = LinkState::Connected;
                    self.reconnect_attempt = 0;
                    self.set_status(format!("connected to {endpoint}"), false);
                }
                MachineEvent::Reconnecting(attempt) => {
                    self.link = LinkState::Reconnecting;
                    self.reconnect_attempt = attempt;
                    self.set_status(
                        format!("connection lost; reconnecting (attempt {attempt})…"),
                        true,
                    );
                }
                MachineEvent::Disconnected(reason) => {
                    self.link = LinkState::Disconnected;
                    self.reconnect_attempt = 0;
                    self.machine_status = None;
                    self.settings = None;
                    self.job_paused = false;
                    self.home_pending_until = None;
                    self.homing = false;
                    self.saw_home = false;
                    self.job_running = false;
                    self.frame_job = false;
                    self.confirm_send = false;
                    match reason {
                        Some(reason) => self.set_status(reason, true),
                        None => self.set_status("disconnected", false),
                    }
                }
                MachineEvent::Status(line) => {
                    if self.link == LinkState::Connected {
                        if let Some(status) = MachineStatus::parse(&line) {
                            if self.homing {
                                if status.state == "Home" {
                                    self.saw_home = true;
                                } else if status.is_alarm()
                                    || (self.saw_home && status.state == "Idle")
                                {
                                    self.homing = false;
                                }
                            }
                            self.machine_status = Some(status);
                        }
                    }
                }
                MachineEvent::Settings(settings) => {
                    let had_bed = self
                        .settings
                        .as_ref()
                        .and_then(|settings| settings.bed)
                        .is_some();
                    if let Some(laser) = settings.laser_mode {
                        self.setting_laser = laser;
                    }
                    if let Some(max_s) = settings.max_s {
                        self.setting_max_s = max_s;
                    }
                    if let Some(homing) = settings.homing {
                        self.setting_homing = homing;
                    }
                    if !had_bed && settings.bed.is_some() && self.show_bed {
                        self.fit_pending = true;
                    }
                    self.settings = Some(settings);
                }
                MachineEvent::JobStarted(total) => {
                    self.job_running = true;
                    self.job_paused = false;
                    self.job_done = 0;
                    self.job_total = total;
                    self.confirm_send = false;
                    if self.frame_job {
                        self.set_status("framing job bounds (laser off)…", false);
                    } else {
                        self.set_status(format!("sending job ({total} lines)…"), false);
                    }
                }
                MachineEvent::JobProgress { done, total } => {
                    self.job_done = done;
                    self.job_total = total;
                }
                MachineEvent::JobDone(acks) => {
                    self.job_running = false;
                    self.job_paused = false;
                    self.job_done = acks;
                    self.job_total = acks.max(self.job_total);
                    if self.frame_job {
                        self.set_status("framing complete", false);
                    } else {
                        self.set_status(format!("job complete: {acks} lines acknowledged"), false);
                    }
                    self.frame_job = false;
                }
                MachineEvent::JobStopped => {
                    self.job_running = false;
                    self.job_paused = false;
                    if self.frame_job {
                        self.set_status("framing stopped", true);
                    } else {
                        self.set_status(
                            "job stopped; controller reset — re-zero before retrying",
                            true,
                        );
                    }
                    self.frame_job = false;
                }
                MachineEvent::JobFailed(reason) => {
                    self.job_running = false;
                    self.job_paused = false;
                    if self.frame_job {
                        self.set_status(format!("framing failed: {reason}"), true);
                    } else {
                        self.set_status(format!("job failed: {reason}"), true);
                    }
                    self.frame_job = false;
                }
                MachineEvent::Message { text, error } => self.set_status(text, error),
            }
        }
        if self.link != LinkState::Disconnected {
            let interval = if self.job_running { 100 } else { 250 };
            ctx.request_repaint_after(Duration::from_millis(interval));
        }
    }

    fn begin_connect(&mut self) {
        let endpoint = self.endpoint.trim().to_owned();
        if endpoint.is_empty() {
            self.set_status("enter an endpoint, e.g. tcp://192.168.1.10", true);
            return;
        }
        self.endpoint = endpoint.clone();
        if self.machine.connect(&endpoint) {
            self.link = LinkState::Connecting;
            self.reconnect_attempt = 0;
            self.machine_status = None;
            self.confirm_home = false;
            self.set_status(format!("connecting to {endpoint}…"), false);
        } else {
            self.set_status("machine thread stopped", true);
        }
    }

    fn can_send(&self) -> bool {
        self.link == LinkState::Connected
            && !self.job_running
            && !self.homing
            && !self.doc.is_empty()
            && !self.job.is_empty()
            && !self
                .home_pending_until
                .map(|until| until > Instant::now())
                .unwrap_or(false)
            && self
                .machine_status
                .as_ref()
                .map(|status| status.state == "Idle" && status.spindle.unwrap_or(0) == 0)
                .unwrap_or(false)
    }

    fn send_job(&mut self) {
        if !self.can_send() {
            self.set_status(
                "cannot send: connect to an idle controller with the laser off",
                true,
            );
            return;
        }
        if self.dirty {
            self.replan();
            self.dirty = false;
        }
        if self.job.cuts().count() == 0 {
            self.set_status("nothing to send: the job has no cuts", true);
            return;
        }
        self.job_uses_m4 = self.gcode_opts.laser_mode == LaserMode::M4;
        let program = gcode::write(&self.job, &self.gcode_opts);
        if self.machine.run(&program) {
            self.job_running = true;
            self.frame_job = false;
            self.job_paused = false;
            self.job_done = 0;
            self.job_total = 0;
            self.set_status("sending job…", false);
        } else {
            self.set_status("machine thread stopped", true);
        }
    }

    fn start_frame(&mut self) {
        if !self.can_send() {
            self.set_status(
                "cannot frame: connect to an idle controller with the laser off",
                true,
            );
            return;
        }
        let Some(bounds) = self.doc.output_bounds() else {
            self.set_status("nothing to frame: no layers are set to Cut", true);
            return;
        };
        let program = frame_program(bounds, FRAME_FEED);
        if self.machine.run(&program) {
            self.job_running = true;
            self.frame_job = true;
            self.job_uses_m4 = false;
            self.job_paused = false;
            self.job_done = 0;
            self.job_total = 0;
            self.set_status("framing job bounds (laser off)…", false);
        } else {
            self.set_status("machine thread stopped", true);
        }
    }

    fn open_settings(&mut self) {
        if let Some(settings) = &self.settings {
            if let Some(laser) = settings.laser_mode {
                self.setting_laser = laser;
            }
            if let Some(max_s) = settings.max_s {
                self.setting_max_s = max_s;
            }
            if let Some(homing) = settings.homing {
                self.setting_homing = homing;
            }
        }
        self.settings_open = true;
    }

    fn apply_settings(&mut self) {
        let Some(settings) = self.settings.clone() else {
            self.set_status("refresh controller settings first", true);
            return;
        };
        let current_laser = settings.laser_mode.unwrap_or(true);
        let current_max_s = settings.max_s.unwrap_or(1000);
        let current_homing = settings.homing.unwrap_or(false);

        let mut changed = false;
        if self.setting_laser != current_laser {
            changed = true;
            self.machine
                .set_setting(32, if self.setting_laser { "1" } else { "0" });
        }
        if self.setting_max_s != current_max_s {
            changed = true;
            self.machine
                .set_setting(30, &self.setting_max_s.to_string());
        }
        if self.setting_homing != current_homing {
            changed = true;
            self.machine
                .set_setting(22, if self.setting_homing { "1" } else { "0" });
        }
        if changed {
            self.set_status("applying controller settings…", false);
        } else {
            self.set_status("no controller settings changed", false);
        }
    }

    fn bed_rect(&self) -> Option<kurbo::Rect> {
        let [width, height] = self.settings.as_ref()?.bed?;
        let [ox, oy, _] = self
            .machine_status
            .as_ref()
            .and_then(|status| status.work_offset())
            .unwrap_or([0.0, 0.0, 0.0]);
        Some(kurbo::Rect::new(-ox, -oy, width - ox, height - oy))
    }

    fn hit_test(&self, p: Point) -> Option<ShapeRef> {
        let tolerance = (6.0 / f64::from(self.view.zoom)).max(0.25);
        for layer in self.doc.layers.iter().rev() {
            if !layer.visible {
                continue;
            }
            let Some(preview) = self.preview.layer(layer.index) else {
                continue;
            };
            for shape in preview.shapes.iter().rev() {
                let filled = layer
                    .shapes
                    .iter()
                    .find(|core| core.id == shape.id)
                    .map(|core| core.is_fill)
                    .unwrap_or(false);
                for contour in &shape.contours {
                    if filled && contour.closed && point_in_polygon(p, &contour.points) {
                        return Some(ShapeRef {
                            layer: layer.index,
                            shape: shape.id.clone(),
                        });
                    }
                    if distance_to_contour(contour, p) <= tolerance {
                        return Some(ShapeRef {
                            layer: layer.index,
                            shape: shape.id.clone(),
                        });
                    }
                }
            }
        }
        None
    }

    fn selection_bounds(&self) -> Option<kurbo::Rect> {
        let mut bounds: Option<kurbo::Rect> = None;
        for selected in &self.selected_shapes {
            let Some(shape) = self
                .doc
                .layers
                .iter()
                .find(|layer| layer.index == selected.layer)
                .and_then(|layer| layer.shapes.iter().find(|shape| shape.id == selected.shape))
            else {
                continue;
            };
            let shape_bounds = shape.bounds();
            bounds = Some(match bounds {
                Some(current) => current.union(shape_bounds),
                None => shape_bounds,
            });
        }
        bounds
    }

    fn press_on_selection(&self, point: Point) -> bool {
        self.selected_shapes.iter().any(|selected| {
            self.doc
                .layers
                .iter()
                .find(|layer| layer.index == selected.layer)
                .filter(|layer| layer.visible)
                .and_then(|layer| layer.shapes.iter().find(|shape| shape.id == selected.shape))
                .map(|shape| shape.bounds().contains(point))
                .unwrap_or(false)
        })
    }

    fn shape_by_ref(&self, shape: &ShapeRef) -> Option<&orleans_core::Shape> {
        self.doc
            .layers
            .iter()
            .find(|layer| layer.index == shape.layer)?
            .shapes
            .iter()
            .find(|entry| entry.id == shape.shape)
    }

    /// The whole group containing `shape`, or just the shape when ungrouped.
    fn unit_members(&self, shape: &ShapeRef) -> BTreeSet<ShapeRef> {
        self.groups
            .iter()
            .find(|group| group.members.contains(shape))
            .map(|group| group.members.clone())
            .unwrap_or_else(|| std::iter::once(shape.clone()).collect())
    }

    /// Select a shape or its whole group. With `toggle`, an already fully
    /// selected unit is removed instead of added (Ctrl/Shift-click).
    fn select_shape_unit(&mut self, shape: &ShapeRef, additive: bool, toggle: bool) {
        let members = self.unit_members(shape);
        if additive {
            let all_selected = members
                .iter()
                .all(|member| self.selected_shapes.contains(member));
            if toggle && all_selected {
                for member in &members {
                    self.selected_shapes.remove(member);
                }
            } else {
                self.selected_shapes.extend(members);
            }
        } else {
            self.selected_shapes = members;
        }
        self.selected_layer = Some(shape.layer);
        self.selected_raster = None;
    }

    fn push_groups_undo(&mut self) {
        self.push_undo(UndoSnapshot {
            groups: Some(self.groups.clone()),
            ..UndoSnapshot::default()
        });
    }

    fn group_selection(&mut self) {
        if self.selected_shapes.len() < 2 {
            self.set_status("select at least two shapes to group", false);
            return;
        }
        let selected = self.selected_shapes.clone();
        let merge: Vec<u64> = self
            .groups
            .iter()
            .filter(|group| group.members.iter().any(|member| selected.contains(member)))
            .map(|group| group.id)
            .collect();
        let mut members = selected;
        for group in &self.groups {
            if merge.contains(&group.id) {
                members.extend(group.members.iter().cloned());
            }
        }
        self.push_groups_undo();
        self.groups.retain(|group| !merge.contains(&group.id));
        let id = self.groups.iter().map(|group| group.id).max().unwrap_or(0) + 1;
        self.groups.push(ShapeGroup {
            id,
            name: format!("Group {id}"),
            members: members.clone(),
        });
        self.selected_shapes = members;
        self.set_status(
            format!("grouped {} shapes", self.selected_shapes.len()),
            false,
        );
    }

    fn ungroup_selection(&mut self) {
        let selected = self.selected_shapes.clone();
        let affected = self
            .groups
            .iter()
            .filter(|group| group.members.iter().any(|member| selected.contains(member)))
            .count();
        if affected == 0 {
            self.set_status("selection is not grouped", false);
            return;
        }
        self.push_groups_undo();
        self.groups
            .retain(|group| !group.members.iter().any(|member| selected.contains(member)));
        self.set_status(format!("ungrouped {affected} group(s)"), false);
    }

    /// Alignment targets: one per selected group, plus every ungrouped shape.
    fn selection_units(&self) -> Vec<SelectionUnit> {
        let mut units: Vec<SelectionUnit> = Vec::new();
        let mut slots: BTreeMap<u64, usize> = BTreeMap::new();
        for selected in &self.selected_shapes {
            let Some(shape) = self.shape_by_ref(selected) else {
                continue;
            };
            let group = self
                .groups
                .iter()
                .find(|group| group.members.contains(selected));
            let Some(group) = group else {
                units.push(SelectionUnit {
                    members: vec![selected.clone()],
                    bounds: shape.bounds(),
                });
                continue;
            };
            let slot = *slots.entry(group.id).or_insert_with(|| {
                units.push(SelectionUnit {
                    members: Vec::new(),
                    bounds: shape.bounds(),
                });
                units.len() - 1
            });
            let unit = &mut units[slot];
            unit.members.push(selected.clone());
            unit.bounds = unit.bounds.union(shape.bounds());
        }
        units
    }

    fn align_selection(&mut self, axis: AlignAxis, edge: AlignEdge) {
        let units = self.selection_units();
        if units.len() < 2 {
            return;
        }
        let overall = units
            .iter()
            .skip(1)
            .fold(units[0].bounds, |bounds, unit| bounds.union(unit.bounds));
        let deltas: Vec<f64> = units
            .iter()
            .map(|unit| align_delta(unit.bounds, overall, axis, edge))
            .collect();
        if deltas.iter().all(|delta| delta.abs() < 1e-9) {
            self.set_status("already aligned", false);
            return;
        }
        self.push_selection_undo();
        for (unit, delta) in units.iter().zip(deltas) {
            if delta.abs() < 1e-9 {
                continue;
            }
            let affine = match axis {
                AlignAxis::Horizontal => Affine::translate((delta, 0.0)),
                AlignAxis::Vertical => Affine::translate((0.0, delta)),
            };
            let members: BTreeSet<ShapeRef> = unit.members.iter().cloned().collect();
            self.apply_affine_to(&members, affine);
        }
        self.set_status(format!("aligned {} object(s)", units.len()), false);
    }

    fn distribute_selection(&mut self, axis: AlignAxis) {
        let units = self.selection_units();
        if units.len() < 3 {
            self.set_status("select at least three objects to distribute", false);
            return;
        }
        let bounds: Vec<kurbo::Rect> = units.iter().map(|unit| unit.bounds).collect();
        let deltas = distribute_deltas(&bounds, axis);
        if deltas.iter().all(|delta| delta.abs() < 1e-9) {
            self.set_status("already evenly spaced", false);
            return;
        }
        self.push_selection_undo();
        for (unit, delta) in units.iter().zip(deltas) {
            if delta.abs() < 1e-9 {
                continue;
            }
            let affine = match axis {
                AlignAxis::Horizontal => Affine::translate((delta, 0.0)),
                AlignAxis::Vertical => Affine::translate((0.0, delta)),
            };
            let members: BTreeSet<ShapeRef> = unit.members.iter().cloned().collect();
            self.apply_affine_to(&members, affine);
        }
        self.set_status(format!("distributed {} objects", units.len()), false);
    }

    fn clear_undo(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.settings_edit = None;
        self.settings_edit_changed = false;
    }

    fn push_undo(&mut self, snapshot: UndoSnapshot) {
        self.undo_stack.push(snapshot);
        self.redo_stack.clear();
        if self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
    }

    fn capture_settings(&self) -> SettingsSnapshot {
        SettingsSnapshot::capture(&self.doc, &self.plan_opts, &self.gcode_opts)
    }

    /// Capture the settings at the start of the frame, before the panels can
    /// change them. A pointer or keyboard gesture keeps the snapshot from its
    /// first frame, so a whole drag undoes as one step; a click that starts and
    /// ends within one frame still has its pre-change state.
    fn begin_settings_edit(&mut self) {
        if self.settings_edit.is_none() {
            self.settings_edit = Some(self.capture_settings());
            self.settings_edit_changed = false;
        }
    }

    fn end_settings_edit(&mut self, ctx: &egui::Context) {
        let interacting = ctx.input(|input| input.pointer.any_down())
            || ctx.memory(|memory| memory.focused().is_some());
        if !interacting && self.settings_edit_changed {
            self.commit_settings_edit();
        }
    }

    fn commit_settings_edit(&mut self) {
        if let Some(before) = self.settings_edit.take() {
            if self.settings_edit_changed {
                self.push_undo(UndoSnapshot {
                    settings: Some(before),
                    ..UndoSnapshot::default()
                });
            }
        }
        self.settings_edit_changed = false;
    }

    fn push_selection_undo(&mut self) {
        let mut shapes = Vec::new();
        for selected in &self.selected_shapes {
            if let Some(shape) = self
                .doc
                .layers
                .iter()
                .find(|layer| layer.index == selected.layer)
                .and_then(|layer| layer.shapes.iter().find(|shape| shape.id == selected.shape))
            {
                shapes.push((selected.layer, selected.shape.clone(), shape.path.clone()));
            }
        }
        if shapes.is_empty() {
            return;
        }
        self.push_undo(UndoSnapshot {
            shapes,
            transforms: Some(self.shape_transforms.clone()),
            edited_paths: Some(self.edited_paths.clone()),
            rasters: Vec::new(),
            settings: None,
            groups: None,
        });
    }

    fn push_raster_undo(&mut self, layer_index: usize) {
        let Some(raster) = self
            .doc
            .layers
            .iter()
            .find(|layer| layer.index == layer_index)
            .and_then(|layer| layer.raster.as_ref())
        else {
            return;
        };
        self.push_undo(UndoSnapshot {
            shapes: Vec::new(),
            transforms: Some(self.shape_transforms.clone()),
            edited_paths: Some(self.edited_paths.clone()),
            rasters: vec![RasterSnapshot {
                layer: layer_index,
                x: raster.x,
                y: raster.y,
                width_mm: raster.width_mm,
                height_mm: raster.height_mm,
                angle: raster.angle,
            }],
            settings: None,
            groups: None,
        });
    }

    /// Capture the current state in the same shape as `snapshot`, for redo.
    fn capture_for(&self, snapshot: &UndoSnapshot) -> UndoSnapshot {
        let mut shapes = Vec::new();
        for (layer_index, shape_id, _) in &snapshot.shapes {
            if let Some(shape) = self
                .doc
                .layers
                .iter()
                .find(|layer| layer.index == *layer_index)
                .and_then(|layer| layer.shapes.iter().find(|shape| shape.id == *shape_id))
            {
                shapes.push((*layer_index, shape_id.clone(), shape.path.clone()));
            }
        }
        let rasters = snapshot
            .rasters
            .iter()
            .filter_map(|saved| {
                let raster = self
                    .doc
                    .layers
                    .iter()
                    .find(|layer| layer.index == saved.layer)?
                    .raster
                    .as_ref()?;
                Some(RasterSnapshot {
                    layer: saved.layer,
                    x: raster.x,
                    y: raster.y,
                    width_mm: raster.width_mm,
                    height_mm: raster.height_mm,
                    angle: raster.angle,
                })
            })
            .collect();
        UndoSnapshot {
            shapes,
            transforms: snapshot
                .transforms
                .as_ref()
                .map(|_| self.shape_transforms.clone()),
            edited_paths: snapshot
                .edited_paths
                .as_ref()
                .map(|_| self.edited_paths.clone()),
            rasters,
            settings: snapshot.settings.as_ref().map(|_| self.capture_settings()),
            groups: snapshot.groups.as_ref().map(|_| self.groups.clone()),
        }
    }

    fn restore_snapshot(&mut self, snapshot: UndoSnapshot) {
        let has_geometry = snapshot.transforms.is_some()
            || snapshot.edited_paths.is_some()
            || !snapshot.shapes.is_empty()
            || !snapshot.rasters.is_empty();

        for (layer_index, shape_id, path) in snapshot.shapes {
            if let Some(shape) = self
                .doc
                .layers
                .iter_mut()
                .find(|layer| layer.index == layer_index)
                .and_then(|layer| layer.shapes.iter_mut().find(|shape| shape.id == shape_id))
            {
                shape.path = path;
            }
        }
        if let Some(transforms) = snapshot.transforms {
            self.shape_transforms = transforms;
        }
        if let Some(edited_paths) = snapshot.edited_paths {
            self.edited_paths = edited_paths;
        }
        for raster in snapshot.rasters {
            if let Some(layer) = self
                .doc
                .layers
                .iter_mut()
                .find(|layer| layer.index == raster.layer)
            {
                if let Some(image) = layer.raster.as_mut() {
                    image.x = raster.x;
                    image.y = raster.y;
                    image.width_mm = raster.width_mm;
                    image.height_mm = raster.height_mm;
                    image.angle = raster.angle;
                }
            }
        }
        if let Some(settings) = snapshot.settings {
            settings.apply(&mut self.doc, &mut self.plan_opts, &mut self.gcode_opts);
        }
        if let Some(groups) = snapshot.groups {
            self.groups = groups;
        }

        self.settings_edit = None;
        self.settings_edit_changed = false;
        self.raster_drag = None;
        self.node_drag = None;
        self.selected_nodes.clear();
        self.dragging_shape = false;
        self.active_handle = None;
        if has_geometry {
            self.preview = Preview::build(&self.doc, DISPLAY_TOLERANCE);
        }
        self.replan();
        self.dirty = false;

        let existing: BTreeSet<(usize, String)> = self
            .doc
            .layers
            .iter()
            .flat_map(|layer| {
                layer
                    .shapes
                    .iter()
                    .map(move |shape| (layer.index, shape.id.clone()))
            })
            .collect();
        self.selected_shapes
            .retain(|selected| existing.contains(&(selected.layer, selected.shape.clone())));

        if let Some(index) = self.selected_layer {
            if !self.doc.layers.iter().any(|layer| layer.index == index) {
                self.selected_layer = self.doc.layers.first().map(|layer| layer.index);
            }
        }
    }

    fn undo_edit(&mut self) {
        self.commit_settings_edit();
        let Some(snapshot) = self.undo_stack.pop() else {
            self.set_status("nothing to undo", false);
            return;
        };
        let current = self.capture_for(&snapshot);
        self.redo_stack.push(current);
        if self.redo_stack.len() > UNDO_LIMIT {
            self.redo_stack.remove(0);
        }
        self.restore_snapshot(snapshot);
        self.set_status("undid edit", false);
    }

    fn redo_edit(&mut self) {
        self.commit_settings_edit();
        let Some(snapshot) = self.redo_stack.pop() else {
            self.set_status("nothing to redo", false);
            return;
        };
        let current = self.capture_for(&snapshot);
        self.undo_stack.push(current);
        if self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
        self.restore_snapshot(snapshot);
        self.set_status("redid edit", false);
    }

    fn apply_affine_to_selection(&mut self, affine: Affine) {
        let selected = self.selected_shapes.clone();
        self.apply_affine_to(&selected, affine);
    }

    fn apply_affine_to(&mut self, shapes: &BTreeSet<ShapeRef>, affine: Affine) {
        for selection in shapes {
            {
                let Some(layer) = self
                    .doc
                    .layers
                    .iter_mut()
                    .find(|layer| layer.index == selection.layer)
                else {
                    continue;
                };
                let Some(shape) = layer
                    .shapes
                    .iter_mut()
                    .find(|shape| shape.id == selection.shape)
                else {
                    continue;
                };
                shape.path.apply_affine(affine);
            }
            let entry = self
                .shape_transforms
                .entry((selection.layer, selection.shape.clone()))
                .or_insert(Affine::IDENTITY);
            *entry = affine * *entry;
            if let Some(preview) = self.preview.shape_mut(selection.layer, &selection.shape) {
                for contour in &mut preview.contours {
                    for point in &mut contour.points {
                        *point = affine * *point;
                    }
                }
            }
        }
        self.dirty = true;
    }

    fn translate_selection(&mut self, dx: f64, dy: f64) {
        if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
            return;
        }
        self.apply_affine_to_selection(Affine::translate((dx, dy)));
    }

    fn move_selection(&mut self) {
        let dx = self.transform_dx;
        let dy = self.transform_dy;
        if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
            return;
        }
        self.push_selection_undo();
        self.translate_selection(dx, dy);
        self.transform_dx = 0.0;
        self.transform_dy = 0.0;
        let count = self.selected_shapes.len();
        self.set_status(format!("moved {count} shape(s)"), false);
    }

    fn ensure_raster_textures(&mut self, ctx: &egui::Context) {
        let mut keep = BTreeSet::new();
        for layer in &self.doc.layers {
            let Some(raster) = &layer.raster else {
                continue;
            };
            keep.insert(layer.index);
            let key = (raster.width, raster.height, raster.invert, layer.color);
            if self.raster_texture_keys.get(&layer.index) == Some(&key) {
                continue;
            }
            if raster.grayscale.len() != (raster.width * raster.height) as usize {
                continue;
            }
            let color = display_color(layer.color);
            let pixels = raster
                .grayscale
                .iter()
                .map(|&value| {
                    let darkness = if raster.invert { value } else { 255 - value };
                    let alpha = (f32::from(darkness) * 0.38).round().clamp(0.0, 255.0) as u8;
                    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
                })
                .collect();
            let image =
                egui::ColorImage::new([raster.width as usize, raster.height as usize], pixels);
            let texture = ctx.load_texture(
                format!("raster-{}", layer.index),
                image,
                egui::TextureOptions::LINEAR,
            );
            self.raster_textures.insert(layer.index, texture);
            self.raster_texture_keys.insert(layer.index, key);
        }
        self.raster_textures.retain(|index, _| keep.contains(index));
        self.raster_texture_keys
            .retain(|index, _| keep.contains(index));
    }

    fn sync_move_target(&mut self) {
        let current = self.selection_anchor_point();
        let current_key = current.map(|point| [point.x.to_bits(), point.y.to_bits()]);
        let key = (self.selected_shapes.clone(), self.move_anchor, current_key);
        if self.move_sync.as_ref() == Some(&key) {
            return;
        }
        self.move_sync = Some(key);
        if let Some(point) = current {
            self.move_to_x = point.x;
            self.move_to_y = point.y;
        } else {
            self.move_to_x = 0.0;
            self.move_to_y = 0.0;
        }
    }

    fn selection_anchor_point(&self) -> Option<Point> {
        let bounds = self.selection_bounds()?;
        Some(self.move_anchor.point(bounds))
    }

    fn move_selection_to(&mut self) {
        let Some(bounds) = self.selection_bounds() else {
            return;
        };
        let current = self.move_anchor.point(bounds);
        let dx = self.move_to_x - current.x;
        let dy = self.move_to_y - current.y;
        if !dx.is_finite() || !dy.is_finite() {
            self.set_status("target position is not a finite number", true);
            return;
        }
        if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
            self.set_status("selection is already at that position", false);
            return;
        }
        self.push_selection_undo();
        self.translate_selection(dx, dy);
        let count = self.selected_shapes.len();
        self.set_status(
            format!(
                "moved {count} shape(s): {} at X {:.3}, Y {:.3}",
                self.move_anchor.label(),
                self.move_to_x,
                self.move_to_y
            ),
            false,
        );
    }

    fn scale_selection(&mut self) {
        let factor = self.transform_scale;
        if !factor.is_finite() || factor <= 0.0 || (factor - 1.0).abs() < 1e-9 {
            return;
        }
        let Some(bounds) = self.selection_bounds() else {
            return;
        };
        let center = bounds.center();
        let affine = scale_about(center, factor);
        self.push_selection_undo();
        self.apply_affine_to_selection(affine);
        self.transform_scale = 1.0;
        let count = self.selected_shapes.len();
        self.set_status(format!("scaled {count} shape(s) ×{factor:.2}"), false);
    }

    fn rotate_selection(&mut self) {
        let degrees = self.transform_angle;
        if !degrees.is_finite() || degrees.abs() < 1e-9 {
            return;
        }
        let Some(bounds) = self.selection_bounds() else {
            return;
        };
        let center = bounds.center();
        let affine = rotate_about(center, degrees.to_radians());
        self.push_selection_undo();
        self.apply_affine_to_selection(affine);
        self.transform_angle = 0.0;
        let count = self.selected_shapes.len();
        self.set_status(format!("rotated {count} shape(s) by {degrees:.1}°"), false);
    }

    fn selection_handle_at(&self, screen: Pos2, area: Rect) -> Option<Handle> {
        let bounds = self.selection_bounds()?;
        let rect = Rect::from_two_pos(
            self.view
                .world_to_screen(area, Point::new(bounds.x0, bounds.y0)),
            self.view
                .world_to_screen(area, Point::new(bounds.x1, bounds.y1)),
        );
        for (handle, position) in selection_handle_positions(rect) {
            let radius = if matches!(handle, Handle::Rotate) {
                10.0
            } else {
                9.0
            };
            if position.distance(screen) <= radius {
                return Some(handle);
            }
        }
        None
    }

    fn begin_handle_drag(&mut self, handle: Handle, world: Point) {
        let Some(bounds) = self.selection_bounds() else {
            return;
        };
        match handle {
            Handle::Scale(corner) => {
                let anchor = corner_point(bounds, opposite_corner(corner));
                let distance = (world - anchor).hypot();
                if distance < 1e-6 {
                    return;
                }
                self.push_selection_undo();
                self.active_handle = Some(ActiveHandle::Scale {
                    anchor,
                    last_distance: distance,
                });
                self.dragging_shape = false;
            }
            Handle::Rotate => {
                let center = bounds.center();
                let angle = (world.y - center.y).atan2(world.x - center.x);
                self.push_selection_undo();
                self.active_handle = Some(ActiveHandle::Rotate {
                    center,
                    last_angle: angle,
                });
                self.dragging_shape = false;
            }
        }
    }

    fn update_handle_drag(&mut self, world: Point) {
        let Some(mut active) = self.active_handle else {
            return;
        };
        match &mut active {
            ActiveHandle::Scale {
                anchor,
                last_distance,
            } => {
                let distance = (world - *anchor).hypot();
                if distance > 1e-6 && *last_distance > 1e-6 {
                    let step = (distance / *last_distance).clamp(0.05, 20.0);
                    if (step - 1.0).abs() > 1e-9 {
                        self.apply_affine_to_selection(scale_about(*anchor, step));
                    }
                    *last_distance = distance;
                }
            }
            ActiveHandle::Rotate { center, last_angle } => {
                let angle = (world.y - center.y).atan2(world.x - center.x);
                let delta = normalize_angle(angle - *last_angle);
                if delta.abs() > 1e-9 {
                    self.apply_affine_to_selection(rotate_about(*center, delta));
                }
                *last_angle = angle;
            }
        }
        self.active_handle = Some(active);
    }

    fn select_raster_layer(&mut self, layer_index: usize) {
        self.selected_raster = Some(layer_index);
        self.selected_layer = Some(layer_index);
        self.selected_shapes.clear();
        self.selected_nodes.clear();
        self.node_edit = None;
        self.node_drag = None;
        self.active_handle = None;
        self.dragging_shape = false;
    }

    fn raster_at(&self, world: Point) -> Option<usize> {
        for layer in self.doc.layers.iter().rev() {
            if !layer.visible {
                continue;
            }
            let Some(raster) = &layer.raster else {
                continue;
            };
            let local = raster.world_to_local(world);
            if local.x >= raster.x
                && local.x <= raster.x + raster.width_mm
                && local.y >= raster.y
                && local.y <= raster.y + raster.height_mm
            {
                return Some(layer.index);
            }
        }
        None
    }

    fn raster_handle_positions(
        &self,
        raster: &orleans_core::RasterImage,
        area: Rect,
    ) -> [(RasterHandle, Pos2); 5] {
        let corners = raster.corners();
        let rotate_offset = 22.0 / f64::from(self.view.zoom);
        let rotate = raster.local_to_world(Point::new(
            raster.x + raster.width_mm * 0.5,
            raster.y + raster.height_mm + rotate_offset,
        ));
        [
            (
                RasterHandle::Corner(HandleCorner::TopLeft),
                self.view.world_to_screen(area, corners[0]),
            ),
            (
                RasterHandle::Corner(HandleCorner::TopRight),
                self.view.world_to_screen(area, corners[1]),
            ),
            (
                RasterHandle::Corner(HandleCorner::BottomRight),
                self.view.world_to_screen(area, corners[2]),
            ),
            (
                RasterHandle::Corner(HandleCorner::BottomLeft),
                self.view.world_to_screen(area, corners[3]),
            ),
            (
                RasterHandle::Rotate,
                self.view.world_to_screen(area, rotate),
            ),
        ]
    }

    fn raster_handle_at(&self, screen: Pos2, area: Rect) -> Option<RasterHandle> {
        let layer_index = self.selected_raster?;
        let raster = self
            .doc
            .layers
            .iter()
            .find(|layer| layer.index == layer_index && layer.visible)?
            .raster
            .as_ref()?;
        for (handle, position) in self.raster_handle_positions(raster, area) {
            let radius = if handle == RasterHandle::Rotate {
                10.0
            } else {
                9.0
            };
            if position.distance(screen) <= radius {
                return Some(handle);
            }
        }
        None
    }

    fn begin_raster_move(&mut self, layer_index: usize, pointer: Point) {
        let Some(raster) = self
            .doc
            .layers
            .iter()
            .find(|layer| layer.index == layer_index)
            .and_then(|layer| layer.raster.as_ref())
        else {
            return;
        };
        let start_x = raster.x;
        let start_y = raster.y;
        self.select_raster_layer(layer_index);
        self.push_raster_undo(layer_index);
        self.raster_drag = Some(RasterDrag::Move {
            layer: layer_index,
            start_pointer: pointer,
            start_x,
            start_y,
        });
    }

    fn begin_raster_resize(&mut self, layer_index: usize, corner: HandleCorner) {
        let (anchor_local, anchor_world, initial_center, angle) = {
            let Some(raster) = self
                .doc
                .layers
                .iter()
                .find(|layer| layer.index == layer_index)
                .and_then(|layer| layer.raster.as_ref())
            else {
                return;
            };
            let anchor_local = match corner {
                HandleCorner::TopLeft => Point::new(raster.x + raster.width_mm, raster.y),
                HandleCorner::TopRight => Point::new(raster.x, raster.y),
                HandleCorner::BottomRight => Point::new(raster.x, raster.y + raster.height_mm),
                HandleCorner::BottomLeft => {
                    Point::new(raster.x + raster.width_mm, raster.y + raster.height_mm)
                }
            };
            (
                anchor_local,
                raster.local_to_world(anchor_local),
                raster.center(),
                raster.angle,
            )
        };
        self.select_raster_layer(layer_index);
        self.push_raster_undo(layer_index);
        self.raster_drag = Some(RasterDrag::Resize {
            layer: layer_index,
            corner,
            anchor_world,
            anchor_local,
            initial_center,
            angle,
        });
    }

    fn begin_raster_rotate(&mut self, layer_index: usize, pointer: Point) {
        let Some(raster) = self
            .doc
            .layers
            .iter()
            .find(|layer| layer.index == layer_index)
            .and_then(|layer| layer.raster.as_ref())
        else {
            return;
        };
        let center = raster.center();
        let start_angle = (pointer.y - center.y).atan2(pointer.x - center.x);
        let initial_angle = raster.angle;
        self.select_raster_layer(layer_index);
        self.push_raster_undo(layer_index);
        self.raster_drag = Some(RasterDrag::Rotate {
            layer: layer_index,
            center,
            start_angle,
            initial_angle,
        });
    }

    fn update_raster_drag(&mut self, pointer: Point) {
        let Some(drag) = self.raster_drag else {
            return;
        };
        match drag {
            RasterDrag::Move {
                layer,
                start_pointer,
                start_x,
                start_y,
            } => {
                if let Some(raster) = self
                    .doc
                    .layers
                    .iter_mut()
                    .find(|entry| entry.index == layer)
                    .and_then(|entry| entry.raster.as_mut())
                {
                    raster.x = start_x + (pointer.x - start_pointer.x);
                    raster.y = start_y + (pointer.y - start_pointer.y);
                    self.dirty = true;
                }
            }
            RasterDrag::Resize {
                layer,
                corner,
                anchor_world,
                anchor_local,
                initial_center,
                angle,
            } => {
                let (x, y, width, height) = raster_resize_frame(
                    corner,
                    pointer,
                    initial_center,
                    angle,
                    anchor_world,
                    anchor_local,
                );
                let new_center = Point::new(x + width * 0.5, y + height * 0.5);
                let (sin, cos) = angle.to_radians().sin_cos();
                let dx = anchor_local.x - new_center.x;
                let dy = anchor_local.y - new_center.y;
                let rotated = Point::new(dx * cos - dy * sin, dx * sin + dy * cos);
                let center_world =
                    Point::new(anchor_world.x - rotated.x, anchor_world.y - rotated.y);
                if let Some(raster) = self
                    .doc
                    .layers
                    .iter_mut()
                    .find(|entry| entry.index == layer)
                    .and_then(|entry| entry.raster.as_mut())
                {
                    raster.x = center_world.x - width * 0.5;
                    raster.y = center_world.y - height * 0.5;
                    raster.width_mm = width;
                    raster.height_mm = height;
                    raster.angle = angle;
                    self.dirty = true;
                }
            }
            RasterDrag::Rotate {
                layer,
                center,
                start_angle,
                initial_angle,
            } => {
                let angle = (pointer.y - center.y).atan2(pointer.x - center.x);
                let delta = normalize_angle(angle - start_angle);
                if let Some(raster) = self
                    .doc
                    .layers
                    .iter_mut()
                    .find(|entry| entry.index == layer)
                    .and_then(|entry| entry.raster.as_mut())
                {
                    raster.angle = initial_angle + delta.to_degrees();
                    self.dirty = true;
                }
            }
        }
    }

    fn draw_raster_selection(&self, painter: &egui::Painter, area: Rect) {
        let Some(layer_index) = self.selected_raster else {
            return;
        };
        let Some(raster) = self
            .doc
            .layers
            .iter()
            .find(|layer| layer.index == layer_index && layer.visible)
            .and_then(|layer| layer.raster.as_ref())
        else {
            return;
        };
        let corners: Vec<Pos2> = raster
            .corners()
            .iter()
            .map(|point| self.view.world_to_screen(area, *point))
            .collect();
        painter.add(egui::Shape::closed_line(
            corners.clone(),
            Stroke::new(2.0, Color32::from_rgb(255, 230, 120)),
        ));
        let top_center = raster.local_to_world(Point::new(
            raster.x + raster.width_mm * 0.5,
            raster.y + raster.height_mm,
        ));
        let top_center_screen = self.view.world_to_screen(area, top_center);
        for (handle, position) in self.raster_handle_positions(raster, area) {
            if handle == RasterHandle::Rotate {
                painter.line_segment(
                    [top_center_screen, position],
                    Stroke::new(1.0, Color32::from_rgb(220, 185, 70)),
                );
                painter.circle_filled(position, 5.0, Color32::from_rgb(255, 225, 120));
                painter.circle_stroke(
                    position,
                    5.0,
                    Stroke::new(1.0, Color32::from_rgb(120, 90, 30)),
                );
            } else {
                let square = Rect::from_center_size(position, egui::vec2(8.0, 8.0));
                painter.rect_filled(square, 1.0, Color32::from_rgb(255, 232, 140));
                painter.rect_stroke(
                    square,
                    1.0,
                    Stroke::new(1.0, Color32::from_rgb(120, 90, 30)),
                    StrokeKind::Inside,
                );
            }
        }
    }

    fn node_shape(&self) -> Option<&orleans_core::Shape> {
        let target = self.node_edit.as_ref()?;
        self.doc
            .layers
            .iter()
            .find(|layer| layer.index == target.layer && layer.visible)?
            .shapes
            .iter()
            .find(|shape| shape.id == target.shape)
    }

    fn node_at(&self, screen: Pos2, area: Rect) -> Option<NodeRef> {
        let shape = self.node_shape()?;
        let mut best: Option<(bool, f32, NodeRef)> = None;
        for node in nodes::nodes(&shape.path) {
            let position = self.view.world_to_screen(area, node.point);
            let distance = position.distance(screen);
            if distance > 8.0 {
                continue;
            }
            let anchor = node.is_anchor();
            let replace = match best {
                None => true,
                Some((best_anchor, best_distance, _)) => {
                    if (distance - best_distance).abs() < 0.5 {
                        anchor && !best_anchor
                    } else {
                        distance < best_distance
                    }
                }
            };
            if replace {
                best = Some((anchor, distance, node.node));
            }
        }
        best.map(|(_, _, node)| node)
    }

    fn begin_node_drag(&mut self, node: NodeRef, world: Point) {
        if self.node_edit.is_none() {
            return;
        }
        let Some(start_node) = self
            .node_shape()
            .and_then(|shape| nodes::node_position(&shape.path, node))
        else {
            return;
        };
        if !self.selected_nodes.contains(&node) {
            self.selected_nodes.clear();
            self.selected_nodes.insert(node);
        }
        self.push_selection_undo();
        self.node_drag = Some(NodeDrag {
            node,
            start_world: world,
            start_node,
        });
        self.dragging_shape = false;
    }

    /// Nearest anchor (preferred) or point on a segment of another visible
    /// shape, within `tolerance` world millimetres.
    fn snap_node_to_geometry(
        &self,
        target: &ShapeRef,
        desired: Point,
        tolerance: f64,
    ) -> Option<Point> {
        let mut paths: Vec<&kurbo::BezPath> = Vec::new();
        for layer in &self.doc.layers {
            if !layer.visible {
                continue;
            }
            for shape in &layer.shapes {
                if layer.index == target.layer && shape.id == target.shape {
                    continue;
                }
                paths.push(&shape.path);
            }
        }
        nearest_geometry_point(&paths, desired, tolerance)
    }

    fn update_node_drag(&mut self, world: Point) {
        let Some(drag) = self.node_drag else {
            return;
        };
        let Some(target) = self.node_edit.clone() else {
            self.node_drag = None;
            return;
        };
        let snap = self.node_snap;
        let step = self.grid_step();
        let geometry_tolerance = (8.0 / f64::from(self.view.zoom)).max(0.05);

        let mut desired = drag.start_node + (world - drag.start_world);
        if self.node_snap_geometry {
            if let Some(point) = self.snap_node_to_geometry(&target, desired, geometry_tolerance) {
                desired = point;
            } else if snap {
                desired = snap_point(desired, step);
            }
        } else if snap {
            desired = snap_point(desired, step);
        }

        let (path, moved) = {
            let Some(shape) = self
                .doc
                .layers
                .iter_mut()
                .find(|layer| layer.index == target.layer)
                .and_then(|layer| {
                    layer
                        .shapes
                        .iter_mut()
                        .find(|shape| shape.id == target.shape)
                })
            else {
                return;
            };
            let Some(current) = nodes::node_position(&shape.path, drag.node) else {
                return;
            };
            let delta = desired - current;
            let moved = delta.hypot() > 1e-9 && nodes::move_node(&mut shape.path, drag.node, delta);
            (shape.path.clone(), moved)
        };

        if moved {
            self.preview
                .update_shape(target.layer, &target.shape, &path, DISPLAY_TOLERANCE);
            self.edited_paths
                .insert((target.layer, target.shape.clone()));
            self.dirty = true;
        }
    }

    fn insert_node_at(&mut self, screen: Pos2, area: Rect) -> bool {
        let Some(target) = self.node_edit.clone() else {
            return false;
        };
        if self.node_at(screen, area).is_some() {
            return false;
        }
        let world = self.view.screen_to_world(area, screen);
        let (path, hit) = {
            let Some(shape) = self
                .doc
                .layers
                .iter()
                .find(|layer| layer.index == target.layer)
                .and_then(|layer| layer.shapes.iter().find(|shape| shape.id == target.shape))
            else {
                return false;
            };
            let Some(hit) = nodes::nearest_segment(&shape.path, world) else {
                return false;
            };
            if self.view.world_to_screen(area, hit.point).distance(screen) > 8.0 {
                return false;
            }
            (shape.path.clone(), hit)
        };

        let mut path = path;
        let Some(node) = nodes::insert_node(&mut path, hit.element, hit.t) else {
            return false;
        };
        self.push_selection_undo();
        if let Some(shape) = self
            .doc
            .layers
            .iter_mut()
            .find(|layer| layer.index == target.layer)
            .and_then(|layer| {
                layer
                    .shapes
                    .iter_mut()
                    .find(|shape| shape.id == target.shape)
            })
        {
            shape.path = path.clone();
        }
        self.preview
            .update_shape(target.layer, &target.shape, &path, DISPLAY_TOLERANCE);
        self.edited_paths
            .insert((target.layer, target.shape.clone()));
        self.selected_nodes.clear();
        self.selected_nodes.insert(node);
        self.dirty = true;
        self.set_status("added node", false);
        true
    }

    fn delete_selected_node(&mut self) {
        let Some(node) = self
            .selected_nodes
            .iter()
            .next()
            .copied()
            .filter(|_| self.selected_nodes.len() == 1)
        else {
            self.set_status("select one anchor to delete", true);
            return;
        };
        if node.kind != NodeKind::Anchor {
            self.set_status("select an anchor to delete", true);
            return;
        }
        let Some(target) = self.node_edit.clone() else {
            return;
        };
        let path = {
            let Some(shape) = self
                .doc
                .layers
                .iter()
                .find(|layer| layer.index == target.layer)
                .and_then(|layer| layer.shapes.iter().find(|shape| shape.id == target.shape))
            else {
                return;
            };
            shape.path.clone()
        };
        let mut path = path;
        if !nodes::delete_anchor(&mut path, node) {
            self.set_status("cannot delete this node", true);
            return;
        }
        self.push_selection_undo();
        if let Some(shape) = self
            .doc
            .layers
            .iter_mut()
            .find(|layer| layer.index == target.layer)
            .and_then(|layer| {
                layer
                    .shapes
                    .iter_mut()
                    .find(|shape| shape.id == target.shape)
            })
        {
            shape.path = path.clone();
        }
        self.preview
            .update_shape(target.layer, &target.shape, &path, DISPLAY_TOLERANCE);
        self.edited_paths
            .insert((target.layer, target.shape.clone()));
        self.selected_nodes.clear();
        self.node_drag = None;
        self.dirty = true;
        self.set_status("deleted node", false);
    }

    fn node_path(&self, target: &ShapeRef) -> Option<kurbo::BezPath> {
        self.doc
            .layers
            .iter()
            .find(|layer| layer.index == target.layer)?
            .shapes
            .iter()
            .find(|shape| shape.id == target.shape)
            .map(|shape| shape.path.clone())
    }

    /// Replace the path being node-edited, with one undo entry and a refreshed
    /// preview. Node indices usually change, so the node selection is cleared.
    fn apply_node_edit(&mut self, target: &ShapeRef, path: kurbo::BezPath, status: &str) {
        self.push_selection_undo();
        if let Some(shape) = self
            .doc
            .layers
            .iter_mut()
            .find(|layer| layer.index == target.layer)
            .and_then(|layer| {
                layer
                    .shapes
                    .iter_mut()
                    .find(|shape| shape.id == target.shape)
            })
        {
            shape.path = path.clone();
        }
        self.preview
            .update_shape(target.layer, &target.shape, &path, DISPLAY_TOLERANCE);
        self.edited_paths
            .insert((target.layer, target.shape.clone()));
        self.selected_nodes.clear();
        self.node_drag = None;
        self.dirty = true;
        self.set_status(status, false);
    }

    fn break_selected_node(&mut self) {
        let Some(node) = self
            .selected_nodes
            .iter()
            .next()
            .copied()
            .filter(|_| self.selected_nodes.len() == 1)
        else {
            self.set_status("select one anchor to break", true);
            return;
        };
        if node.kind != NodeKind::Anchor {
            self.set_status("select one anchor to break", true);
            return;
        }
        let Some(target) = self.node_edit.clone() else {
            return;
        };
        let Some(mut path) = self.node_path(&target) else {
            return;
        };
        if !nodes::break_path(&mut path, node) {
            self.set_status("cannot break at this node", true);
            return;
        }
        self.apply_node_edit(&target, path, "broke path at node");
    }

    fn join_selected_nodes(&mut self) {
        let selected: Vec<NodeRef> = self.selected_nodes.iter().copied().collect();
        if selected.len() != 2 || selected.iter().any(|node| node.kind != NodeKind::Anchor) {
            self.set_status("select two endpoint anchors to join", true);
            return;
        }
        let Some(target) = self.node_edit.clone() else {
            return;
        };
        let Some(mut path) = self.node_path(&target) else {
            return;
        };
        if !nodes::join_nodes(&mut path, selected[0], selected[1]) {
            self.set_status("cannot join these nodes", true);
            return;
        }
        self.apply_node_edit(&target, path, "joined nodes");
    }

    fn toggle_node_edit(&mut self) {
        if self.node_edit.is_some() {
            self.exit_node_edit();
            return;
        }
        if self.selected_shapes.len() != 1 {
            self.set_status("select exactly one shape to edit its nodes", true);
            return;
        }
        self.node_edit = self.selected_shapes.iter().next().cloned();
        self.selected_nodes.clear();
        self.node_drag = None;
        self.selected_raster = None;
        self.active_handle = None;
        self.dragging_shape = false;
        self.set_status("node editing: drag anchors and control points", false);
    }

    fn exit_node_edit(&mut self) {
        if self.node_edit.take().is_some() {
            self.selected_nodes.clear();
            self.node_drag = None;
            self.set_status("node editing finished", false);
        }
    }

    fn pause_is_safe(&self) -> bool {
        self.job_uses_m4
            && self
                .settings
                .as_ref()
                .and_then(|settings| settings.laser_mode)
                == Some(true)
    }

    fn disconnect_machine(&mut self) {
        let _ = self.machine.disconnect();
        self.link = LinkState::Disconnected;
        self.reconnect_attempt = 0;
        self.machine_status = None;
        self.settings = None;
        self.job_paused = false;
        self.set_status("disconnected", false);
    }

    fn status_contents(&mut self, ui: &mut egui::Ui) {
        match &self.machine_status {
            Some(status) => {
                ui.horizontal(|ui| {
                    ui.label("State");
                    if self.job_running {
                        let (label, color) = if self.job_paused {
                            ("Paused", Color32::from_rgb(120, 170, 240))
                        } else if self.frame_job {
                            ("Framing", Color32::from_rgb(230, 190, 90))
                        } else {
                            ("Streaming", Color32::from_rgb(230, 190, 90))
                        };
                        ui.colored_label(color, RichText::new(label).strong());
                    } else {
                        ui.colored_label(
                            state_color(&status.state),
                            RichText::new(&status.state).strong(),
                        );
                    }
                });
                if self.job_running {
                    let note = if self.job_paused {
                        "feed hold active — press Resume"
                    } else if self.frame_job {
                        "live status paused while framing"
                    } else {
                        "live status paused while sending"
                    };
                    ui.label(RichText::new(note).small().weak());
                }
                if let Some([x, y, z]) = status.work_position().or(status.mpos) {
                    let label = if status.work_position().is_some() {
                        "WPos"
                    } else {
                        "MPos"
                    };
                    ui.horizontal(|ui| {
                        ui.label(label);
                        ui.monospace(format!("X {x:.2}  Y {y:.2}  Z {z:.2}"));
                    });
                }
                if status.feed.is_some() || status.spindle.is_some() {
                    ui.label(format!(
                        "F {} mm/min   S {}",
                        status.feed.unwrap_or(0),
                        status.spindle.unwrap_or(0)
                    ));
                }
                ui.add_space(2.0);
                let raw = ui.add(
                    egui::Label::new(RichText::new(&status.raw).small().weak().monospace())
                        .truncate(),
                );
                raw.on_hover_text(&status.raw);
            }
            None => {
                let text = match self.link {
                    LinkState::Connecting => "waiting for status…",
                    LinkState::Reconnecting => "waiting for the link…",
                    _ => "not connected",
                };
                ui.label(RichText::new(text).weak());
            }
        }

        if let Some(settings) = &self.settings {
            let mut parts = Vec::new();
            if let Some(laser) = settings.laser_mode {
                parts.push(format!("$32={}", if laser { "1" } else { "0" }));
            }
            if let Some(max_s) = settings.max_s {
                parts.push(format!("$30={max_s}"));
            }
            if let Some(homing) = settings.homing {
                parts.push(format!("$22={}", if homing { "1" } else { "0" }));
            }
            if parts.is_empty() {
                ui.label(
                    RichText::new("controller settings unavailable ($$)")
                        .small()
                        .weak(),
                );
            } else {
                ui.add(
                    egui::Label::new(RichText::new(parts.join(" · ")).small().weak()).truncate(),
                );
            }
            if settings.laser_mode == Some(false) {
                ui.colored_label(
                    Color32::from_rgb(235, 120, 110),
                    "laser mode is off ($32=0)",
                );
            }
            if let Some(max_s) = settings.max_s {
                if max_s != self.gcode_opts.max_s {
                    ui.colored_label(
                        Color32::from_rgb(230, 190, 90),
                        format!("Max S {} differs from $30={max_s}", self.gcode_opts.max_s),
                    );
                }
            }
        } else if self.link == LinkState::Connected {
            ui.label(
                RichText::new("controller settings unavailable ($$)")
                    .small()
                    .weak(),
            );
        }
    }

    fn machine_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("machine")
            .resizable(true)
            .default_size(300.0)
            .min_size(260.0)
            .max_size(420.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.heading("Machine");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (text, color) = match self.link {
                            LinkState::Connected => ("connected", Color32::from_rgb(120, 200, 130)),
                            LinkState::Connecting | LinkState::Reconnecting => {
                                ("connecting", Color32::from_rgb(230, 190, 90))
                            }
                            LinkState::Disconnected => ("offline", Color32::from_gray(140)),
                        };
                        ui.label(RichText::new(text).color(color).small());
                    });
                });
                ui.add_space(4.0);

                ui.label("Endpoint");
                let field = ui.add_enabled(
                    self.link == LinkState::Disconnected,
                    egui::TextEdit::singleline(&mut self.endpoint)
                        .hint_text("tcp://192.168.1.10")
                        .desired_width(f32::INFINITY),
                );
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

                ui.horizontal(|ui| match self.link {
                    LinkState::Disconnected => {
                        if ui.button("Connect").clicked() || enter {
                            self.begin_connect();
                        }
                    }
                    LinkState::Connecting => {
                        ui.spinner();
                        ui.label("Connecting…");
                        if ui.button("Cancel").clicked() {
                            self.disconnect_machine();
                        }
                    }
                    LinkState::Reconnecting => {
                        ui.spinner();
                        ui.label(format!("Reconnecting… ({})", self.reconnect_attempt));
                        if ui.button("Cancel").clicked() {
                            self.disconnect_machine();
                        }
                    }
                    LinkState::Connected => {
                        if ui
                            .add_enabled(!self.job_running, egui::Button::new("Disconnect"))
                            .clicked()
                        {
                            self.disconnect_machine();
                        }
                        if ui.button("Settings…").clicked() {
                            self.open_settings();
                        }
                    }
                });

                ui.separator();
                ui.heading("Status");
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 160.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("machine-status-scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.status_contents(ui));
                    },
                );
                let connected = self.link == LinkState::Connected;
                let pending = self.homing
                    || self
                        .home_pending_until
                        .map(|until| until > Instant::now())
                        .unwrap_or(false);

                ui.separator();
                ui.heading("Jog");
                let idle = self
                    .machine_status
                    .as_ref()
                    .map(|status| status.state == "Idle" && status.spindle.unwrap_or(0) == 0)
                    .unwrap_or(false);
                let jog_pending = self
                    .jog_pending_until
                    .map(|until| until > Instant::now())
                    .unwrap_or(false);
                let can_jog = connected && !self.job_running && !pending && !jog_pending && idle;
                let can_cancel_jog = connected && !self.job_running;

                ui.horizontal(|ui| {
                    ui.label("Step");
                    ui.add(
                        egui::DragValue::new(&mut self.jog_step)
                            .speed(0.1)
                            .range(0.1..=100.0)
                            .suffix(" mm"),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("Feed");
                    ui.add(
                        egui::DragValue::new(&mut self.jog_feed)
                            .speed(10.0)
                            .range(100.0..=10_000.0)
                            .suffix(" mm/min"),
                    );
                });

                let pad = |label: &str| egui::Button::new(label).min_size(egui::vec2(46.0, 26.0));
                egui::Grid::new("jog-pad")
                    .num_columns(3)
                    .spacing([4.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("");
                        if ui
                            .add_enabled(can_jog, pad("Y+"))
                            .on_hover_text("Jog +Y")
                            .clicked()
                        {
                            self.machine.jog(0.0, self.jog_step, self.jog_feed);
                            self.jog_pending_until =
                                Some(Instant::now() + Duration::from_millis(250));
                        }
                        ui.label("");
                        ui.end_row();

                        if ui
                            .add_enabled(can_jog, pad("X-"))
                            .on_hover_text("Jog -X")
                            .clicked()
                        {
                            self.machine.jog(-self.jog_step, 0.0, self.jog_feed);
                            self.jog_pending_until =
                                Some(Instant::now() + Duration::from_millis(250));
                        }
                        if ui
                            .add_enabled(can_cancel_jog, pad("■"))
                            .on_hover_text("Cancel jog (realtime 0x85)")
                            .clicked()
                        {
                            self.machine.jog_cancel();
                        }
                        if ui
                            .add_enabled(can_jog, pad("X+"))
                            .on_hover_text("Jog +X")
                            .clicked()
                        {
                            self.machine.jog(self.jog_step, 0.0, self.jog_feed);
                            self.jog_pending_until =
                                Some(Instant::now() + Duration::from_millis(250));
                        }
                        ui.end_row();

                        ui.label("");
                        if ui
                            .add_enabled(can_jog, pad("Y-"))
                            .on_hover_text("Jog -Y")
                            .clicked()
                        {
                            self.machine.jog(0.0, -self.jog_step, self.jog_feed);
                            self.jog_pending_until =
                                Some(Instant::now() + Duration::from_millis(250));
                        }
                        ui.label("");
                        ui.end_row();
                    });

                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(can_jog, egui::Button::new("Set zero (G92)"))
                        .on_hover_text(
                            "Temporary work zero at the current position. \
                             A controller reset clears it.",
                        )
                        .clicked()
                        && !self.machine.set_origin()
                    {
                        self.set_status("machine thread stopped", true);
                    }
                    if ui
                        .add_enabled(can_jog, egui::Button::new("Save zero (G54)"))
                        .on_hover_text(
                            "Stored G54 work zero (G10 L20 P1) at the current position. \
                             Survives a reset on most GRBL builds.",
                        )
                        .clicked()
                        && !self.machine.save_origin()
                    {
                        self.set_status("machine thread stopped", true);
                    }
                });

                ui.separator();
                ui.heading("Job");
                if self.job_running {
                    let fraction = self.job_done as f32 / self.job_total.max(1) as f32;
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .show_percentage()
                            .text(format!("{} / {}", self.job_done, self.job_total)),
                    );
                    ui.horizontal(|ui| {
                        let pause_safe = self.frame_job || self.pause_is_safe();
                        if ui
                            .add_enabled(!self.job_paused && pause_safe, egui::Button::new("Pause"))
                            .on_hover_text(if self.frame_job {
                                "Pause the framing move"
                            } else {
                                "Feed hold. Requires M4 dynamic power and $32=1."
                            })
                            .clicked()
                        {
                            self.job_paused = true;
                            self.machine.pause_job();
                            self.set_status("pausing…", false);
                        }
                        if ui
                            .add_enabled(self.job_paused, egui::Button::new("Resume"))
                            .clicked()
                        {
                            self.job_paused = false;
                            self.machine.resume_job();
                            self.set_status("resuming…", false);
                        }
                    });
                    if !self.frame_job && !self.pause_is_safe() {
                        ui.label(
                            RichText::new("Pause requires M4 + $32=1; use Stop.")
                                .small()
                                .weak(),
                        );
                    }
                    ui.add_space(2.0);
                }
                let can_send = self.can_send();
                let send_label = if self.job_running {
                    if self.frame_job {
                        "Framing…"
                    } else {
                        "Sending…"
                    }
                } else {
                    "Send to laser"
                };
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(can_send, egui::Button::new(send_label))
                        .on_hover_text(
                            "Stream the current job. The controller must be idle \
                             with the laser off.",
                        )
                        .clicked()
                    {
                        self.confirm_send = true;
                    }
                    if ui
                        .add_enabled(can_send, egui::Button::new("Frame"))
                        .on_hover_text("Trace the job bounds at 3000 mm/min with the laser off.")
                        .clicked()
                    {
                        self.start_frame();
                    }
                });
                ui.label(
                    RichText::new(format!(
                        "{} cuts · {:.1} mm · ~{}",
                        self.job.cuts().count(),
                        self.job.stats.cut_mm,
                        format_duration(self.job.stats.seconds)
                    ))
                    .small()
                    .weak(),
                );

                ui.separator();
                ui.heading("Control");
                let homing_enabled = self
                    .settings
                    .as_ref()
                    .map(|settings| settings.homing != Some(false))
                    .unwrap_or(true);
                let can_home = connected
                    && !self.job_running
                    && !self.homing
                    && homing_enabled
                    && !pending
                    && self
                        .settings
                        .as_ref()
                        .map(|settings| settings.homing != Some(false))
                        .unwrap_or(true)
                    && self
                        .machine_status
                        .as_ref()
                        .map(|status| status.can_home())
                        .unwrap_or(false);

                if ui
                    .add_enabled(can_home, egui::Button::new("Home ($H)"))
                    .on_hover_text(if homing_enabled {
                        "Move the head to the limit switches. \
                         Enabled when the controller is idle or in alarm."
                    } else {
                        "Homing is disabled ($22=0); enable it in Settings."
                    })
                    .clicked()
                {
                    self.confirm_home = true;
                }
                if ui
                    .add_enabled(
                        connected && !self.job_running,
                        egui::Button::new("Unlock ($X)"),
                    )
                    .on_hover_text("Clear a GRBL alarm.")
                    .clicked()
                    && !self.machine.unlock()
                {
                    self.set_status("machine thread stopped", true);
                }
                ui.add_space(6.0);
                let stop = egui::Button::new(RichText::new("Stop").strong().color(Color32::WHITE))
                    .fill(Color32::from_rgb(145, 40, 40));
                if ui
                    .add_enabled(connected, stop)
                    .on_hover_text("Feed hold and soft reset; switches the laser off.")
                    .clicked()
                {
                    if self.job_running {
                        self.machine.stop_job();
                        self.job_paused = false;
                        self.set_status("stopping job…", false);
                    } else if !self.machine.stop() {
                        self.set_status("machine thread stopped", true);
                    }
                    self.home_pending_until = None;
                    self.homing = false;
                    self.saw_home = false;
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Jogging and homing move the head. Clear the bed first.")
                        .small()
                        .weak(),
                );
            });
    }

    fn home_dialog(&mut self, ctx: &egui::Context) {
        if !self.confirm_home {
            return;
        }
        egui::Window::new("Home the machine?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(
                    "The head will move to the limit switches. Make sure the machine is clear.",
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Home").clicked() {
                        if self.machine.home() {
                            self.home_pending_until = Some(Instant::now() + Duration::from_secs(3));
                            self.set_status("homing…", false);
                        } else {
                            self.set_status("machine thread stopped", true);
                        }
                        self.confirm_home = false;
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_home = false;
                    }
                });
            });
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let connected = self.link == LinkState::Connected;
        let can_edit = connected && !self.job_running && !self.homing && self.settings.is_some();
        let mut close = false;

        egui::Window::new("Controller settings")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "GRBL stores these settings in EEPROM. \
                         Only change them while the machine is not cutting.",
                    )
                    .small()
                    .weak(),
                );
                ui.add_space(4.0);
                ui.add_enabled(
                    can_edit,
                    egui::Checkbox::new(&mut self.setting_laser, "Laser mode ($32)"),
                );
                ui.horizontal(|ui| {
                    ui.label("$30 max S");
                    ui.add_enabled(
                        can_edit,
                        egui::DragValue::new(&mut self.setting_max_s)
                            .speed(10.0)
                            .range(1..=100_000),
                    );
                });
                ui.add_enabled(
                    can_edit,
                    egui::Checkbox::new(&mut self.setting_homing, "Homing enabled ($22)"),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(can_edit, egui::Button::new("Apply"))
                        .clicked()
                    {
                        self.apply_settings();
                    }
                    if ui
                        .add_enabled(connected, egui::Button::new("Refresh"))
                        .clicked()
                    {
                        self.machine.refresh_settings();
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
                if !connected {
                    ui.label(RichText::new("connect to edit controller settings").weak());
                } else if self.settings.is_none() {
                    ui.label(RichText::new("settings unavailable; press Refresh").weak());
                }
                if let Some(settings) = &self.settings {
                    if let Some([w, h]) = settings.bed {
                        ui.label(format!("Bed / max travel: {w:.1} × {h:.1} mm"));
                    }
                    if settings.laser_mode == Some(false) {
                        ui.colored_label(
                            Color32::from_rgb(235, 120, 110),
                            "laser mode is off; M4 pause will stay disabled",
                        );
                    }
                    egui::CollapsingHeader::new("All settings")
                        .default_open(false)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(180.0)
                                .show(ui, |ui| {
                                    for (key, value) in &settings.all {
                                        ui.monospace(format!("${key}={value}"));
                                    }
                                });
                        });
                }
            });
        if close {
            self.settings_open = false;
        }
    }

    fn send_dialog(&mut self, ctx: &egui::Context) {
        if !self.confirm_send {
            return;
        }
        egui::Window::new("Send job to the laser?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                // Ignore Ctrl+Enter here: that is the shortcut that opened
                // this dialog, and it must not also confirm it.
                let enter = ui.input(|i| !i.modifiers.command && i.key_pressed(egui::Key::Enter));
                let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
                let cuts = self.job.cuts().count();
                let max_power = self
                    .doc
                    .layers
                    .iter()
                    .map(|layer| layer.cut.power)
                    .fold(0.0, f64::max);
                ui.label(format!(
                    "{cuts} cuts · {:.1} mm of cutting · ~{}",
                    self.job.stats.cut_mm,
                    format_duration(self.job.stats.seconds)
                ));
                ui.label(format!(
                    "{} layers · highest power {max_power:.0}% · max S {}",
                    self.doc.layers.len(),
                    self.gcode_opts.max_s
                ));
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "The machine will move and the laser will fire. Check the material, \
                         focus and work origin first.",
                    )
                    .color(Color32::from_rgb(235, 190, 90)),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if enter || ui.button("Send").clicked() {
                        self.send_job();
                        self.confirm_send = false;
                    }
                    if escape || ui.button("Cancel").clicked() {
                        self.confirm_send = false;
                    }
                });
            });
    }

    fn export(&mut self) {
        if self.doc.is_empty() {
            self.status = Status::error("nothing to export");
            return;
        }
        if self.dirty {
            self.replan();
            self.dirty = false;
        }

        let default_name = self
            .display_path()
            .and_then(|path| path.file_stem())
            .map(|stem| format!("{}.gcode", stem.to_string_lossy()))
            .unwrap_or_else(|| "orleans.gcode".to_owned());
        let Some(path) = rfd::FileDialog::new()
            .add_filter("G-code", &["gcode", "nc", "gc"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };

        let program = gcode::write(&self.job, &self.gcode_opts);
        match std::fs::write(&path, program) {
            Ok(()) => {
                self.status = Status::ok(format!(
                    "wrote {} · {} cuts · {:.1} mm · ~{}",
                    path.display(),
                    self.job.cuts().count(),
                    self.job.stats.cut_mm,
                    format_duration(self.job.stats.seconds)
                ));
            }
            Err(err) => {
                self.status = Status::error(format!("writing {}: {err}", path.display()));
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Open…").clicked() {
                    *action = Some(Action::Open);
                }
                let has_doc = !self.doc.is_empty();
                if ui
                    .add_enabled(has_doc, egui::Button::new("Export G-code…"))
                    .clicked()
                {
                    *action = Some(Action::Export);
                }
                if ui
                    .add_enabled(has_doc, egui::Button::new("Save project…"))
                    .clicked()
                {
                    *action = Some(Action::SaveProject);
                }
                if ui
                    .add_enabled(has_doc, egui::Button::new("Reload"))
                    .clicked()
                {
                    *action = Some(Action::Reload);
                }
                if ui.button("Fit").clicked() {
                    *action = Some(Action::Fit);
                }
                if ui
                    .add_enabled(!self.undo_stack.is_empty(), egui::Button::new("Undo"))
                    .on_hover_text("Undo last edit (Ctrl+Z)")
                    .clicked()
                {
                    *action = Some(Action::Undo);
                }
                if ui
                    .add_enabled(!self.redo_stack.is_empty(), egui::Button::new("Redo"))
                    .on_hover_text("Redo last undone edit (Ctrl+Shift+Z)")
                    .clicked()
                {
                    *action = Some(Action::Redo);
                }
                ui.separator();
                ui.checkbox(&mut self.show_grid, "Grid");
                ui.checkbox(&mut self.show_travel, "Travel");
                ui.checkbox(&mut self.show_bed, "Bed");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let name = self
                        .display_path()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "no file".to_owned());
                    ui.label(RichText::new(name).weak());
                });
            });
            ui.add_space(4.0);
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        self.sync_move_target();
        egui::Panel::left("layers")
            .resizable(true)
            .default_size(340.0)
            .min_size(280.0)
            .max_size(560.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                let total = self.doc.layers.len();
                let visible = self.doc.layers.iter().filter(|layer| layer.visible).count();
                let cutting = self.doc.layers.iter().filter(|layer| layer.output).count();
                ui.horizontal(|ui| {
                    ui.heading("Layers");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if total > 0 {
                            ui.label(
                                RichText::new(format!("{cutting} cut · {visible} shown"))
                                    .small()
                                    .weak(),
                            );
                        }
                    });
                });
                ui.add_space(2.0);

                let mut changed = false;
                if total == 0 {
                    ui.label(RichText::new("No layers yet. Open an SVG or image, or drop one here.").weak());
                } else {
                    let transform_height = if self.selected_shapes.is_empty() {
                        0.0
                    } else {
                        104.0
                    };
                    let list_height =
                        (ui.available_height() - 350.0 - transform_height).clamp(80.0, 360.0);
                    let mut pick = None;
                    let mut output_changed = false;
                    egui::ScrollArea::vertical()
                        .id_salt("layer-list")
                        .max_height(list_height)
                        .show(ui, |ui| {
                            for layer in &mut self.doc.layers {
                                let selected = self.selected_layer == Some(layer.index);
                                let name = if layer.name.is_empty() {
                                    format!("#{}", layer.index)
                                } else {
                                    layer.name.clone()
                                };
                                let detail = if let Some(raster) = &layer.raster {
                                    format!("{}×{} px", raster.width, raster.height)
                                } else {
                                    let shapes = layer.shapes.len();
                                    format!("{shapes} shape{}", if shapes == 1 { "" } else { "s" })
                                };
                                let mode = match layer.mode {
                                    LayerMode::Line => "line",
                                    LayerMode::Fill => "fill",
                                    LayerMode::Raster => "raster",
                                };
                                let summary = format!(
                                    "{:.0} mm/min  ·  {:.0}%  ·  ×{}  ·  {mode}",
                                    layer.cut.speed, layer.cut.power, layer.cut.passes
                                );
                                let row = egui::Frame::new()
                                    .fill(if selected {
                                        ui.visuals().selection.bg_fill
                                    } else {
                                        Color32::TRANSPARENT
                                    })
                                    .corner_radius(3.0)
                                    .inner_margin(egui::Margin::symmetric(4, 2))
                                    .show(ui, |ui| {
                                        ui.set_min_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            if ui
                                                .checkbox(&mut layer.output, "Cut")
                                                .on_hover_text("Include this layer in the job")
                                                .changed()
                                            {
                                                output_changed = true;
                                            }
                                            let (chip, _) = ui.allocate_exact_size(
                                                egui::vec2(10.0, 10.0),
                                                egui::Sense::hover(),
                                            );
                                            ui.painter().rect_filled(
                                                chip,
                                                2.0,
                                                if layer.output {
                                                    display_color(layer.color)
                                                } else {
                                                    Color32::from_gray(90)
                                                },
                                            );
                                            let text = if layer.visible {
                                                RichText::new(name).strong()
                                            } else {
                                                RichText::new(name).weak()
                                            };
                                            ui.label(text);
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        RichText::new(detail.clone()).small().weak(),
                                                    );
                                                },
                                            );
                                        });
                                        ui.horizontal(|ui| {
                                            ui.add_space(30.0);
                                            let text = if layer.visible && layer.output {
                                                RichText::new(summary).small().weak()
                                            } else {
                                                RichText::new(summary)
                                                    .small()
                                                    .color(Color32::from_gray(95))
                                            };
                                            ui.label(text);
                                        });
                                    });
                                if row.response.interact(egui::Sense::click()).clicked() {
                                    pick = Some(layer.index);
                                }
                                ui.add_space(2.0);
                            }
                        });
                    if let Some(index) = pick {
                        let is_raster = self
                            .doc
                            .layers
                            .iter()
                            .find(|layer| layer.index == index)
                            .is_some_and(|layer| layer.raster.is_some());
                        if is_raster {
                            self.select_raster_layer(index);
                        } else {
                            self.selected_raster = None;
                            if !self
                                .selected_shapes
                                .iter()
                                .any(|shape| shape.layer == index)
                            {
                                self.selected_shapes.clear();
                            }
                            self.selected_layer = Some(index);
                        }
                    }
                    changed |= output_changed;
                    if self.selected_layer.is_none() {
                        self.selected_layer = self.doc.layers.first().map(|layer| layer.index);
                    }
                }

                ui.separator();
                let selected_index = self
                    .selected_layer
                    .or_else(|| self.doc.layers.first().map(|layer| layer.index));
                self.selected_layer = selected_index;
                let selected_pos = selected_index.and_then(|index| {
                    self.doc
                        .layers
                        .iter()
                        .position(|layer| layer.index == index)
                });
                if let Some(pos) = selected_pos {
                    let layer_count = self.doc.layers.len();
                    let mut move_up = false;
                    let mut move_down = false;
                    // Name and colour do not affect the toolpath, so they are
                    // undoable without forcing a replan on every keystroke.
                    let mut display_changed = false;
                    {
                        let layer = &mut self.doc.layers[pos];
                        ui.horizontal(|ui| {
                            ui.strong("Selected layer");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(format!("#{}", layer.index)).small().weak(),
                                    );
                                },
                            );
                        });
                        ui.horizontal(|ui| {
                            display_changed |= ui.color_edit_button_srgb(&mut layer.color).changed();
                            display_changed |= ui
                                .add(
                                    egui::TextEdit::singleline(&mut layer.name)
                                        .hint_text("Layer name")
                                        .desired_width(f32::INFINITY),
                                )
                                .changed();
                        });
                        egui::Grid::new("selected-layer-params")
                            .num_columns(2)
                            .spacing([8.0, 4.0])
                            .show(ui, |ui| {
                                ui.label("Speed");
                                changed |= ui
                                    .add(
                                        egui::DragValue::new(&mut layer.cut.speed)
                                            .speed(10.0)
                                            .range(0.0..=30000.0)
                                            .suffix(" mm/min"),
                                    )
                                    .changed();
                                ui.end_row();

                                ui.label("Power");
                                changed |= ui
                                    .add(
                                        egui::DragValue::new(&mut layer.cut.power)
                                            .speed(1.0)
                                            .range(0.0..=100.0)
                                            .suffix(" %"),
                                    )
                                    .changed();
                                ui.end_row();

                                ui.label("Passes");
                                changed |= ui
                                    .add(
                                        egui::DragValue::new(&mut layer.cut.passes)
                                            .speed(1.0)
                                            .range(1..=50),
                                    )
                                    .changed();
                                ui.end_row();
                            });
                        ui.horizontal(|ui| {
                            changed |= ui
                                .checkbox(&mut layer.output, "Cut")
                                .on_hover_text("Include this layer in the job and frame")
                                .changed();
                            changed |= ui
                                .checkbox(&mut layer.visible, "Show")
                                .on_hover_text("Draw this layer in the editor")
                                .changed();
                        });
                        if let Some(raster) = layer.raster.as_mut() {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "Raster image · {}×{} px",
                                        raster.width, raster.height
                                    ))
                                    .small()
                                    .weak(),
                                );
                                let mut dpi = f64::from(raster.width) / raster.width_mm * 25.4;
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut dpi)
                                            .speed(1.0)
                                            .range(36.0..=1200.0)
                                            .suffix(" dpi"),
                                    )
                                    .on_hover_text(
                                        "Pixel resolution; changing it scales both dimensions",
                                    )
                                    .changed()
                                {
                                    let dpi = dpi.clamp(1.0, 10_000.0);
                                    raster.width_mm = f64::from(raster.width) / dpi * 25.4;
                                    raster.height_mm = f64::from(raster.height) / dpi * 25.4;
                                    changed = true;
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("Raster");
                                egui::ComboBox::from_id_salt("raster-mode")
                                    .selected_text(match raster.mode {
                                        RasterMode::Threshold => "Threshold",
                                        RasterMode::Dither => "Dither",
                                        RasterMode::Grayscale => "Grayscale",
                                    })
                                    .show_ui(ui, |ui| {
                                        changed |= ui
                                            .selectable_value(
                                                &mut raster.mode,
                                                RasterMode::Threshold,
                                                "Threshold",
                                            )
                                            .changed();
                                        changed |= ui
                                            .selectable_value(
                                                &mut raster.mode,
                                                RasterMode::Dither,
                                                "Dither",
                                            )
                                            .changed();
                                        changed |= ui
                                            .selectable_value(
                                                &mut raster.mode,
                                                RasterMode::Grayscale,
                                                "Grayscale",
                                            )
                                            .changed();
                                    });
                                if raster.mode != RasterMode::Grayscale {
                                    ui.label("Threshold");
                                    changed |= ui
                                        .add(
                                            egui::DragValue::new(&mut raster.threshold)
                                                .speed(1.0)
                                                .range(0..=255),
                                        )
                                        .on_hover_text("Darkness threshold; lower values burn more")
                                        .changed();
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("Line interval");
                                changed |= ui
                                    .add(
                                        egui::DragValue::new(&mut raster.line_interval)
                                            .speed(0.01)
                                            .range(0.05..=5.0)
                                            .suffix(" mm"),
                                    )
                                    .on_hover_text("Distance between raster scan lines")
                                    .changed();
                                ui.label("Angle");
                                changed |= ui
                                    .add(
                                        egui::DragValue::new(&mut raster.angle)
                                            .speed(1.0)
                                            .range(-180.0..=180.0)
                                            .suffix("°"),
                                    )
                                    .on_hover_text("Rotate the image around its centre")
                                    .changed();
                                changed |= ui
                                    .checkbox(&mut raster.invert, "Invert")
                                    .on_hover_text("Engrave white instead of dark pixels")
                                    .changed();
                            });
                            ui.horizontal(|ui| {
                                changed |= ui
                                    .checkbox(&mut raster.bidirectional, "Alternate rows")
                                    .on_hover_text("Reverse alternate scan lines to reduce travel")
                                    .changed();
                                ui.label("Position");
                                changed |= ui
                                    .add(
                                        egui::DragValue::new(&mut raster.x)
                                            .speed(0.1)
                                            .max_decimals(3)
                                            .prefix("X "),
                                    )
                                    .changed();
                                changed |= ui
                                    .add(
                                        egui::DragValue::new(&mut raster.y)
                                            .speed(0.1)
                                            .max_decimals(3)
                                            .prefix("Y "),
                                    )
                                    .changed();
                            });
                            ui.horizontal(|ui| {
                                ui.label("Size");
                                changed |= ui
                                    .add(
                                        egui::DragValue::new(&mut raster.width_mm)
                                            .speed(0.1)
                                            .range(0.1..=100_000.0)
                                            .suffix(" W"),
                                    )
                                    .changed();
                                changed |= ui
                                    .add(
                                        egui::DragValue::new(&mut raster.height_mm)
                                            .speed(0.1)
                                            .range(0.1..=100_000.0)
                                            .suffix(" H"),
                                    )
                                    .changed();
                            });
                        } else {
                            ui.horizontal(|ui| {
                                ui.label("Mode");
                                changed |= ui
                                    .selectable_value(&mut layer.mode, LayerMode::Line, "Line")
                                    .changed();
                                changed |= ui
                                    .selectable_value(&mut layer.mode, LayerMode::Fill, "Fill")
                                    .changed();
                            });
                            if layer.mode == LayerMode::Line {
                                ui.horizontal(|ui| {
                                    ui.label("Tabs");
                                    changed |= ui
                                        .add(
                                            egui::DragValue::new(&mut layer.tabs.count)
                                                .speed(1.0)
                                                .range(0..=16),
                                        )
                                        .on_hover_text(
                                            "Number of holding tabs per outer contour; 0 disables",
                                        )
                                        .changed();
                                    if layer.tabs.count > 0 {
                                        ui.label("Width");
                                        changed |= ui
                                            .add(
                                                egui::DragValue::new(&mut layer.tabs.width)
                                                    .speed(0.1)
                                                    .range(0.5..=20.0)
                                                    .suffix(" mm"),
                                            )
                                            .on_hover_text("Width of each uncut tab")
                                            .changed();
                                        changed |= ui
                                            .checkbox(&mut layer.tabs.inner, "Holes")
                                            .on_hover_text("Also place tabs on inner contours")
                                            .changed();
                                    }
                                });
                            }
                            if layer.mode == LayerMode::Fill {
                                egui::Grid::new("layer-fill-params")
                                .num_columns(2)
                                .spacing([8.0, 4.0])
                                .show(ui, |ui| {
                                    ui.label("Fill spacing");
                                    changed |= ui
                                        .add(
                                            egui::DragValue::new(&mut layer.fill.spacing)
                                                .speed(0.05)
                                                .range(0.05..=10.0)
                                                .suffix(" mm"),
                                        )
                                        .on_hover_text("Distance between hatch lines")
                                        .changed();
                                    ui.end_row();

                                    ui.label("Angle");
                                    changed |= ui
                                        .add(
                                            egui::DragValue::new(&mut layer.fill.angle)
                                                .speed(1.0)
                                                .range(0.0..=180.0)
                                                .suffix("°"),
                                        )
                                        .on_hover_text("0° is horizontal, 90° is vertical")
                                        .changed();
                                    ui.end_row();

                                    ui.label("Overlap");
                                    changed |= ui
                                        .add(
                                            egui::DragValue::new(&mut layer.fill.overlap)
                                                .speed(0.05)
                                                .range(0.0..=5.0)
                                                .suffix(" mm"),
                                        )
                                        .on_hover_text(
                                            "Extend each hatch line at both ends by this distance",
                                        )
                                        .changed();
                                    ui.end_row();

                                    ui.label("Alternate");
                                    changed |= ui
                                        .checkbox(&mut layer.fill.alternate, "Reverse each line")
                                        .on_hover_text("Alternate hatch direction to reduce travel")
                                        .changed();
                                    ui.end_row();
                                });
                            }
                        }
                        ui.horizontal(|ui| {
                            move_up = ui
                                .add_enabled(pos > 0, egui::Button::new("Up"))
                                .on_hover_text("Cut earlier")
                                .clicked();
                            move_down = ui
                                .add_enabled(pos + 1 < layer_count, egui::Button::new("Down"))
                                .on_hover_text("Cut later")
                                .clicked();
                            ui.label(RichText::new("cut order").small().weak());
                        });
                    }
                    self.settings_edit_changed |= display_changed;
                    if move_up {
                        self.doc.layers.swap(pos, pos - 1);
                        changed = true;
                    }
                    if move_down {
                        self.doc.layers.swap(pos, pos + 1);
                        changed = true;
                    }
                } else {
                    ui.label(RichText::new("Select a layer to edit it.").weak());
                }

                let selected_count = self.selected_shapes.len();
                let hint = if self.node_edit.is_some() {
                    "Drag nodes · Ctrl-click adds · dbl-click segment inserts · Break/Join · Esc finishes"
                } else if self.selected_raster.is_some() {
                    "Drag image to move · corners resize · top handle rotates"
                } else if selected_count == 0 {
                    "Click a shape or image · Ctrl/Shift-click for multiple · right-drag to pan"
                } else {
                    "Corner handles scale · top handle rotates · Ctrl+Z/Ctrl+Shift+Z undo/redo"
                };
                ui.label(RichText::new(hint).small().weak());

                if selected_count > 0 {
                    ui.horizontal(|ui| {
                        ui.strong(format!("Selection ({selected_count})"));
                    });
                    if let Some(bounds) = self.selection_bounds() {
                        ui.horizontal(|ui| {
                            ui.label("Size").on_hover_text(
                                "Bounding-box width and height of the selected shapes",
                            );
                            ui.label("W");
                            ui.label(
                                RichText::new(format!("{:.3} mm", bounds.width())).monospace(),
                            );
                            ui.label("H");
                            ui.label(
                                RichText::new(format!("{:.3} mm", bounds.height())).monospace(),
                            );
                        });
                    }
                    let units = self.selection_units();
                    let has_group = self.groups.iter().any(|group| {
                        group
                            .members
                            .iter()
                            .any(|member| self.selected_shapes.contains(member))
                    });
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(selected_count >= 2, egui::Button::new("Group"))
                            .on_hover_text("Select and transform these shapes together")
                            .clicked()
                        {
                            self.group_selection();
                        }
                        if ui
                            .add_enabled(has_group, egui::Button::new("Ungroup"))
                            .on_hover_text("Remove the groups these shapes belong to")
                            .clicked()
                        {
                            self.ungroup_selection();
                        }
                        if has_group {
                            ui.label(
                                RichText::new(format!("{} object(s)", units.len()))
                                    .small()
                                    .weak(),
                            );
                        }
                    });
                    if units.len() >= 2 {
                        ui.horizontal(|ui| {
                            ui.label("Align");
                            if ui
                                .small_button("L")
                                .on_hover_text("Align left edges")
                                .clicked()
                            {
                                self.align_selection(AlignAxis::Horizontal, AlignEdge::Start);
                            }
                            if ui
                                .small_button("C")
                                .on_hover_text("Align horizontal centres")
                                .clicked()
                            {
                                self.align_selection(AlignAxis::Horizontal, AlignEdge::Center);
                            }
                            if ui
                                .small_button("R")
                                .on_hover_text("Align right edges")
                                .clicked()
                            {
                                self.align_selection(AlignAxis::Horizontal, AlignEdge::End);
                            }
                            if ui
                                .small_button("T")
                                .on_hover_text("Align top edges")
                                .clicked()
                            {
                                self.align_selection(AlignAxis::Vertical, AlignEdge::End);
                            }
                            if ui
                                .small_button("M")
                                .on_hover_text("Align vertical centres")
                                .clicked()
                            {
                                self.align_selection(AlignAxis::Vertical, AlignEdge::Center);
                            }
                            if ui
                                .small_button("B")
                                .on_hover_text("Align bottom edges")
                                .clicked()
                            {
                                self.align_selection(AlignAxis::Vertical, AlignEdge::Start);
                            }
                        });
                    }
                    if units.len() >= 3 {
                        ui.horizontal(|ui| {
                            ui.label("Distribute");
                            if ui
                                .small_button("H")
                                .on_hover_text("Space centres evenly left to right")
                                .clicked()
                            {
                                self.distribute_selection(AlignAxis::Horizontal);
                            }
                            if ui
                                .small_button("V")
                                .on_hover_text("Space centres evenly bottom to top")
                                .clicked()
                            {
                                self.distribute_selection(AlignAxis::Vertical);
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        ui.label("Move");
                        ui.label("X");
                        ui.add(
                            egui::DragValue::new(&mut self.transform_dx)
                                .speed(0.1)
                                .suffix(" mm"),
                        );
                        ui.label("Y");
                        ui.add(
                            egui::DragValue::new(&mut self.transform_dy)
                                .speed(0.1)
                                .suffix(" mm"),
                        );
                        if ui.button("Move").clicked() {
                            self.move_selection();
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Position").on_hover_text(
                            "Move the reference point to absolute coordinates relative to the job origin (0, 0)",
                        );
                        ui.label("X");
                        ui.add(
                            egui::DragValue::new(&mut self.move_to_x)
                                .speed(0.1)
                                .max_decimals(3)
                                .suffix(" mm"),
                        );
                        ui.label("Y");
                        ui.add(
                            egui::DragValue::new(&mut self.move_to_y)
                                .speed(0.1)
                                .max_decimals(3)
                                .suffix(" mm"),
                        );
                        if ui
                            .button("Move to")
                            .on_hover_text("Move the selected reference point to these coordinates")
                            .clicked()
                        {
                            self.move_selection_to();
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Ref");
                        egui::ComboBox::from_id_salt("selection-anchor")
                            .selected_text(self.move_anchor.label())
                            .show_ui(ui, |ui| {
                                for anchor in SelectionAnchor::ALL {
                                    ui.selectable_value(
                                        &mut self.move_anchor,
                                        anchor,
                                        anchor.label(),
                                    );
                                }
                            });
                        if let Some(point) = self.selection_anchor_point() {
                            ui.label(
                                RichText::new(format!("now {:.3}, {:.3}", point.x, point.y))
                                    .small()
                                    .weak(),
                            )
                            .on_hover_text(
                                "Current reference point relative to the job origin (0, 0)",
                            );
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Scale");
                        ui.add(
                            egui::DragValue::new(&mut self.transform_scale)
                                .speed(0.01)
                                .range(0.05..=10.0),
                        );
                        if ui.button("Scale").clicked() {
                            self.scale_selection();
                        }
                        ui.label("Rotate");
                        ui.add(
                            egui::DragValue::new(&mut self.transform_angle)
                                .speed(1.0)
                                .suffix("°"),
                        );
                        if ui.button("Rotate").clicked() {
                            self.rotate_selection();
                        }
                    });
                    if selected_count == 1 {
                        ui.horizontal(|ui| {
                            let editing = self.node_edit.is_some();
                            if ui
                                .button(if editing { "Done" } else { "Edit nodes" })
                                .on_hover_text(
                                    "Edit path anchors and Bézier control points of the selected shape",
                                )
                                .clicked()
                            {
                                self.toggle_node_edit();
                            }
                            if editing {
                                ui.label(RichText::new("drag anchors / controls").small().weak());
                            }
                        });
                        if self.node_edit.is_some() {
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut self.node_snap, "Snap")
                                    .on_hover_text("Snap dragged nodes to the canvas grid");
                                ui.checkbox(&mut self.node_snap_geometry, "Geometry")
                                    .on_hover_text(
                                        "Snap dragged nodes to anchors and segments of other shapes",
                                    );
                            });
                            ui.horizontal(|ui| {
                                let single_anchor = self.selected_nodes.len() == 1
                                    && self
                                        .selected_nodes
                                        .iter()
                                        .all(|node| node.kind == NodeKind::Anchor);
                                let two_anchors = self.selected_nodes.len() == 2
                                    && self
                                        .selected_nodes
                                        .iter()
                                        .all(|node| node.kind == NodeKind::Anchor);
                                if ui
                                    .add_enabled(single_anchor, egui::Button::new("Delete node"))
                                    .on_hover_text("Delete the selected anchor (Del)")
                                    .clicked()
                                {
                                    self.delete_selected_node();
                                }
                                if ui
                                    .add_enabled(single_anchor, egui::Button::new("Break"))
                                    .on_hover_text(
                                        "Open a closed contour at this anchor, or split an open path in two",
                                    )
                                    .clicked()
                                {
                                    self.break_selected_node();
                                }
                                if ui
                                    .add_enabled(two_anchors, egui::Button::new("Join"))
                                    .on_hover_text(
                                        "Join two endpoint anchors; Ctrl/Shift-click selects multiple nodes",
                                    )
                                    .clicked()
                                {
                                    self.join_selected_nodes();
                                }
                            });
                        }
                    }
                }

                ui.separator();
                ui.heading("Job");
                egui::Grid::new("job-settings")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("Kerf");
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.plan_opts.kerf)
                                    .speed(0.01)
                                    .range(0.0..=3.0)
                                    .suffix(" mm"),
                            )
                            .changed();
                        ui.end_row();

                        ui.label("Max S");
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.gcode_opts.max_s)
                                    .speed(10.0)
                                    .range(1..=100_000),
                            )
                            .changed();
                        ui.end_row();
                    });
                changed |= ui
                    .checkbox(&mut self.plan_opts.optimize, "Optimise travel")
                    .changed();

                let mut m4 = self.gcode_opts.laser_mode == LaserMode::M4;
                if ui
                    .checkbox(&mut m4, "Dynamic power (M4)")
                    .on_hover_text("Velocity-scaled power. Required for a safe pause when $32=1.")
                    .changed()
                {
                    self.gcode_opts.laser_mode = if m4 { LaserMode::M4 } else { LaserMode::M3 };
                    changed = true;
                }

                if changed {
                    self.dirty = true;
                    self.settings_edit_changed = true;
                }
            });
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                if self.doc.is_empty() {
                    ui.label("No design loaded");
                } else {
                    let rasters = self.doc.raster_count();
                    let images = if rasters > 0 {
                        format!(" · {rasters} image{}", if rasters == 1 { "" } else { "s" })
                    } else {
                        String::new()
                    };
                    ui.label(format!(
                        "{} layers · {} shapes{images} · {} cuts",
                        self.doc.layers.len(),
                        self.doc.shape_count(),
                        self.job.cuts().count()
                    ));
                    ui.separator();
                    ui.label(format!(
                        "cut {:.1} mm · travel {:.1} mm · ~{}",
                        self.job.stats.cut_mm,
                        self.job.stats.travel_mm,
                        format_duration(self.job.stats.seconds)
                    ));
                    ui.separator();
                    ui.label(format!("{:.1} px/mm", self.view.zoom));
                }
            });
            let color = if self.status.error {
                Color32::from_rgb(235, 120, 110)
            } else {
                Color32::from_gray(170)
            };
            ui.label(RichText::new(&self.status.text).color(color).small());
            ui.add_space(3.0);
        });
    }

    fn canvas_panel(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| self.canvas(ui));
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let area = response.rect;
        painter.rect_filled(area, 0.0, Color32::from_gray(16));

        if self.fit_pending {
            let mut bounds = self.doc.bounds().unwrap_or_else(|| {
                kurbo::Rect::new(0.0, 0.0, self.doc.width.max(1.0), self.doc.height.max(1.0))
            });
            if self.show_bed {
                if let Some(bed) = self.bed_rect() {
                    bounds = bounds.union(bed);
                }
            }
            self.view.fit(bounds, area, 48.0);
            self.fit_pending = false;
        }

        if self.show_bed {
            self.draw_bed(&painter, area);
        }
        if self.show_grid {
            self.draw_grid(&painter, area);
        }
        self.draw_page(&painter, area);
        if self.show_travel {
            self.draw_travel(&painter, area);
        }
        self.draw_layers(&painter, area);
        self.draw_selected_shape(&painter, area);
        self.draw_nodes(&painter, area);
        self.draw_raster_selection(&painter, area);
        self.draw_origin(&painter, area);

        let additive = ui.input(|input| input.modifiers.command || input.modifiers.shift);
        let node_editing = self.node_edit.is_some();

        if response.clicked_by(egui::PointerButton::Primary) {
            if let Some(position) = response.interact_pointer_pos() {
                if node_editing {
                    if let Some(node) = self.node_at(position, area) {
                        if additive {
                            if !self.selected_nodes.remove(&node) {
                                self.selected_nodes.insert(node);
                            }
                        } else {
                            self.selected_nodes.clear();
                            self.selected_nodes.insert(node);
                        }
                    } else if !additive {
                        self.selected_nodes.clear();
                    }
                } else if self.selection_handle_at(position, area).is_none()
                    && self.raster_handle_at(position, area).is_none()
                {
                    let world = self.view.screen_to_world(area, position);
                    let hit = self.hit_test(world);
                    if let Some(hit) = hit {
                        self.select_shape_unit(&hit, additive, true);
                    } else if let Some(layer) = self.raster_at(world) {
                        self.select_raster_layer(layer);
                    } else if !additive {
                        self.selected_shapes.clear();
                        self.selected_raster = None;
                    }
                }
            }
        }

        if response.drag_started_by(egui::PointerButton::Primary) {
            let origin = ui
                .ctx()
                .input(|input| input.pointer.press_origin())
                .or_else(|| response.interact_pointer_pos());
            if node_editing {
                self.dragging_shape = false;
                if let Some(position) = origin {
                    if let Some(node) = self.node_at(position, area) {
                        let world = self.view.screen_to_world(area, position);
                        self.begin_node_drag(node, world);
                    }
                }
            } else {
                let mut started = false;
                if let Some(position) = origin {
                    if let Some(handle) = self.raster_handle_at(position, area) {
                        let layer = self.selected_raster.expect("raster handle has a layer");
                        let world = self.view.screen_to_world(area, position);
                        match handle {
                            RasterHandle::Corner(corner) => {
                                self.begin_raster_resize(layer, corner);
                            }
                            RasterHandle::Rotate => {
                                self.begin_raster_rotate(layer, world);
                            }
                        }
                        started = true;
                    } else if let Some(handle) = self.selection_handle_at(position, area) {
                        let world = self.view.screen_to_world(area, position);
                        self.begin_handle_drag(handle, world);
                        started = true;
                    }
                }
                if !started {
                    let world = origin.map(|position| self.view.screen_to_world(area, position));
                    let vector_hit = world.and_then(|world| self.hit_test(world));
                    let mut drag = false;
                    if let Some(world) = world {
                        if self.press_on_selection(world) {
                            drag = true;
                        } else if let Some(hit) = vector_hit {
                            self.select_shape_unit(&hit, additive, false);
                            drag = true;
                        } else if let Some(layer) = self.raster_at(world) {
                            self.begin_raster_move(layer, world);
                            started = true;
                        }
                    }
                    if drag {
                        self.push_selection_undo();
                        self.dragging_shape = true;
                    } else if !started {
                        self.dragging_shape = false;
                    }
                }
            }
        }
        if response.drag_stopped() {
            self.dragging_shape = false;
            if let Some(active) = self.active_handle.take() {
                let count = self.selected_shapes.len();
                let action = match active {
                    ActiveHandle::Scale { .. } => "scaled",
                    ActiveHandle::Rotate { .. } => "rotated",
                };
                self.set_status(format!("{action} {count} shape(s)"), false);
            }
            if self.node_drag.take().is_some() {
                self.set_status("moved node", false);
            }
            if let Some(drag) = self.raster_drag.take() {
                let action = match drag {
                    RasterDrag::Move { .. } => "moved image",
                    RasterDrag::Resize { .. } => "resized image",
                    RasterDrag::Rotate { .. } => "rotated image",
                };
                self.set_status(action, false);
            }
        }

        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.view.pan(response.drag_delta());
        } else if response.dragged_by(egui::PointerButton::Primary) {
            if self.raster_drag.is_some() {
                if let Some(position) = response.interact_pointer_pos() {
                    let world = self.view.screen_to_world(area, position);
                    self.update_raster_drag(world);
                }
            } else if self.node_drag.is_some() {
                if let Some(position) = response.interact_pointer_pos() {
                    let world = self.view.screen_to_world(area, position);
                    self.update_node_drag(world);
                }
            } else if self.active_handle.is_some() {
                if let Some(position) = response.interact_pointer_pos() {
                    let world = self.view.screen_to_world(area, position);
                    self.update_handle_drag(world);
                }
            } else if self.dragging_shape {
                let delta = response.drag_delta();
                let zoom = f64::from(self.view.zoom);
                self.translate_selection(f64::from(delta.x) / zoom, -f64::from(delta.y) / zoom);
            } else {
                self.view.pan(response.drag_delta());
            }
        }

        if let Some(position) = response.hover_pos() {
            if node_editing {
                if self.node_at(position, area).is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            } else if let Some(handle) = self.raster_handle_at(position, area) {
                let icon = match handle {
                    RasterHandle::Rotate => egui::CursorIcon::Crosshair,
                    RasterHandle::Corner(HandleCorner::TopLeft | HandleCorner::BottomRight) => {
                        egui::CursorIcon::ResizeNwSe
                    }
                    RasterHandle::Corner(HandleCorner::TopRight | HandleCorner::BottomLeft) => {
                        egui::CursorIcon::ResizeNeSw
                    }
                };
                ui.ctx().set_cursor_icon(icon);
            } else if self
                .raster_at(self.view.screen_to_world(area, position))
                .is_some()
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
            } else if let Some(handle) = self.selection_handle_at(position, area) {
                let icon = match handle {
                    Handle::Rotate => egui::CursorIcon::Crosshair,
                    Handle::Scale(HandleCorner::TopLeft | HandleCorner::BottomRight) => {
                        egui::CursorIcon::ResizeNwSe
                    }
                    Handle::Scale(HandleCorner::TopRight | HandleCorner::BottomLeft) => {
                        egui::CursorIcon::ResizeNeSw
                    }
                };
                ui.ctx().set_cursor_icon(icon);
            }
        }

        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta().y);
            if scroll.abs() > 0.5 {
                if let Some(cursor) = response.hover_pos() {
                    let factor = (scroll * 0.002).exp();
                    self.view.zoom_at(area, cursor, factor);
                }
            }
        }
        if response.double_clicked() {
            if node_editing {
                if let Some(position) = response.interact_pointer_pos() {
                    if self.node_at(position, area).is_none()
                        && !self.insert_node_at(position, area)
                    {
                        self.fit_pending = true;
                    }
                }
            } else {
                self.fit_pending = true;
            }
        }
    }

    fn grid_step(&self) -> f64 {
        nice_step(80.0 / f64::from(self.view.zoom))
    }

    fn draw_grid(&self, painter: &egui::Painter, area: Rect) {
        let visible = self.view.visible_world(area);
        let step = self.grid_step();
        if !(step.is_finite() && step > 0.0) {
            return;
        }
        let stroke = Stroke::new(1.0, Color32::from_gray(34));

        let mut guard = 0;
        let mut x = (visible.x0 / step).floor() * step;
        while x <= visible.x1 && guard < 2000 {
            let sx = self.view.world_to_screen(area, Point::new(x, 0.0)).x;
            if sx >= area.left() && sx <= area.right() {
                painter.line_segment(
                    [Pos2::new(sx, area.top()), Pos2::new(sx, area.bottom())],
                    stroke,
                );
            }
            x += step;
            guard += 1;
        }

        let mut y = (visible.y0 / step).floor() * step;
        while y <= visible.y1 && guard < 4000 {
            let sy = self.view.world_to_screen(area, Point::new(0.0, y)).y;
            if sy >= area.top() && sy <= area.bottom() {
                painter.line_segment(
                    [Pos2::new(area.left(), sy), Pos2::new(area.right(), sy)],
                    stroke,
                );
            }
            y += step;
            guard += 1;
        }

        if visible.x0 <= 0.0 && visible.x1 >= 0.0 {
            let sx = self.view.world_to_screen(area, Point::new(0.0, 0.0)).x;
            painter.line_segment(
                [Pos2::new(sx, area.top()), Pos2::new(sx, area.bottom())],
                Stroke::new(1.2, Color32::from_rgb(110, 55, 55)),
            );
        }
        if visible.y0 <= 0.0 && visible.y1 >= 0.0 {
            let sy = self.view.world_to_screen(area, Point::new(0.0, 0.0)).y;
            painter.line_segment(
                [Pos2::new(area.left(), sy), Pos2::new(area.right(), sy)],
                Stroke::new(1.2, Color32::from_rgb(55, 110, 55)),
            );
        }
    }

    fn draw_bed(&self, painter: &egui::Painter, area: Rect) {
        let Some(bed) = self.bed_rect() else {
            return;
        };
        let rect = Rect::from_two_pos(
            self.view.world_to_screen(area, Point::new(bed.x0, bed.y0)),
            self.view.world_to_screen(area, Point::new(bed.x1, bed.y1)),
        );
        painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(60, 100, 160, 18));
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0, Color32::from_rgb(75, 110, 160)),
            StrokeKind::Inside,
        );
        painter.text(
            rect.left_top() + egui::vec2(4.0, 2.0),
            egui::Align2::LEFT_TOP,
            format!("bed {:.0}×{:.0} mm", bed.width(), bed.height()),
            egui::FontId::proportional(10.0),
            Color32::from_rgb(110, 140, 180),
        );
    }

    fn draw_page(&self, painter: &egui::Painter, area: Rect) {
        if self.doc.width <= 0.0 || self.doc.height <= 0.0 {
            return;
        }
        let a = self.view.world_to_screen(area, Point::new(0.0, 0.0));
        let b = self
            .view
            .world_to_screen(area, Point::new(self.doc.width, self.doc.height));
        let rect = Rect::from_two_pos(a, b);
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0, Color32::from_gray(90)),
            StrokeKind::Inside,
        );
    }

    fn draw_travel(&self, painter: &egui::Painter, area: Rect) {
        let stroke = Stroke::new(1.0, Color32::from_gray(55));
        let mut pos = self.plan_opts.start;
        for op in &self.job.ops {
            match op {
                Op::Rapid { to } => {
                    painter.line_segment(
                        [
                            self.view.world_to_screen(area, pos),
                            self.view.world_to_screen(area, *to),
                        ],
                        stroke,
                    );
                    pos = *to;
                }
                Op::Cut { polyline, .. } => {
                    if let Some(last) = polyline.last() {
                        pos = *last;
                    }
                }
            }
        }
    }

    fn draw_layers(&self, painter: &egui::Painter, area: Rect) {
        for layer in &self.doc.layers {
            if !layer.visible {
                continue;
            }
            let Some(preview) = self.preview.layer(layer.index) else {
                continue;
            };
            let color = if layer.output {
                display_color(layer.color)
            } else {
                Color32::from_gray(85)
            };
            let width = if self.selected_layer == Some(layer.index) {
                2.4
            } else {
                1.4
            };
            if layer.mode == LayerMode::Fill {
                let fill_stroke = Stroke::new(0.8, color.gamma_multiply(0.72));
                if layer.output {
                    for op in &self.job.ops {
                        let Op::Cut {
                            polyline,
                            layer: cut_layer,
                            ..
                        } = op
                        else {
                            continue;
                        };
                        if *cut_layer != layer.index || polyline.len() < 2 {
                            continue;
                        }
                        let points: Vec<Pos2> = polyline
                            .iter()
                            .map(|point| self.view.world_to_screen(area, *point))
                            .collect();
                        painter.add(egui::Shape::line(points, fill_stroke));
                    }
                } else {
                    // Keep the fill visible when Cut is disabled; the job does
                    // not contain toolpaths for this layer.
                    let contours: Vec<_> = layer
                        .shapes
                        .iter()
                        .flat_map(|shape| flatten_path(&shape.path, DISPLAY_TOLERANCE))
                        .collect();
                    let segments = hatch_contours(
                        &contours,
                        &HatchOptions {
                            spacing: layer.fill.spacing,
                            angle_deg: layer.fill.angle,
                            overlap: layer.fill.overlap,
                            alternate: layer.fill.alternate,
                        },
                    );
                    for segment in segments {
                        if segment.len() < 2 {
                            continue;
                        }
                        let points: Vec<Pos2> = segment
                            .iter()
                            .map(|point| self.view.world_to_screen(area, *point))
                            .collect();
                        painter.add(egui::Shape::line(points, fill_stroke));
                    }
                }
            }
            if let Some(raster) = &layer.raster {
                if let Some(texture) = self.raster_textures.get(&layer.index) {
                    let corners = raster.corners();
                    let points = corners.map(|point| self.view.world_to_screen(area, point));
                    let mut mesh = egui::Mesh::with_texture(texture.id());
                    mesh.vertices = vec![
                        egui::epaint::Vertex {
                            pos: points[0],
                            uv: egui::Pos2::new(0.0, 0.0),
                            color: Color32::WHITE,
                        },
                        egui::epaint::Vertex {
                            pos: points[1],
                            uv: egui::Pos2::new(1.0, 0.0),
                            color: Color32::WHITE,
                        },
                        egui::epaint::Vertex {
                            pos: points[2],
                            uv: egui::Pos2::new(1.0, 1.0),
                            color: Color32::WHITE,
                        },
                        egui::epaint::Vertex {
                            pos: points[3],
                            uv: egui::Pos2::new(0.0, 1.0),
                            color: Color32::WHITE,
                        },
                    ];
                    mesh.indices = vec![0, 1, 2, 0, 2, 3];
                    painter.add(egui::Shape::mesh(mesh));
                }
                let border_points: Vec<Pos2> = raster
                    .corners()
                    .iter()
                    .map(|point| self.view.world_to_screen(area, *point))
                    .collect();
                painter.add(egui::Shape::closed_line(
                    border_points.clone(),
                    Stroke::new(1.0, color.gamma_multiply(0.8)),
                ));
                if layer.output {
                    for op in &self.job.ops {
                        let Op::Cut {
                            polyline,
                            params,
                            layer: cut_layer,
                        } = op
                        else {
                            continue;
                        };
                        if *cut_layer != layer.index || polyline.len() < 2 {
                            continue;
                        }
                        let strength = if layer.cut.power > 0.0 {
                            (params.power / layer.cut.power).clamp(0.15, 1.0) as f32
                        } else {
                            1.0
                        };
                        let stroke = Stroke::new(0.8, color.gamma_multiply(strength));
                        let points: Vec<Pos2> = polyline
                            .iter()
                            .map(|point| self.view.world_to_screen(area, *point))
                            .collect();
                        painter.add(egui::Shape::line(points, stroke));
                    }
                } else {
                    painter.text(
                        border_points[0] + egui::vec2(4.0, 2.0),
                        egui::Align2::LEFT_TOP,
                        "image · not cut",
                        egui::FontId::proportional(10.0),
                        color,
                    );
                }
            }
            let stroke = Stroke::new(width, color);
            for shape in &preview.shapes {
                for contour in &shape.contours {
                    let mut points: Vec<Pos2> = contour
                        .points
                        .iter()
                        .map(|p| self.view.world_to_screen(area, *p))
                        .collect();
                    if contour.closed && points.len() > 1 {
                        points.push(points[0]);
                    }
                    if points.len() >= 2 {
                        painter.add(egui::Shape::line(points, stroke));
                    }
                }
            }
        }
    }

    fn draw_selected_shape(&self, painter: &egui::Painter, area: Rect) {
        for selected in &self.selected_shapes {
            let Some(layer) = self
                .doc
                .layers
                .iter()
                .find(|layer| layer.index == selected.layer)
            else {
                continue;
            };
            if !layer.visible {
                continue;
            }
            let Some(shape) = layer.shapes.iter().find(|shape| shape.id == selected.shape) else {
                continue;
            };
            let Some(preview) = self.preview.shape(selected.layer, &selected.shape) else {
                continue;
            };

            let stroke = Stroke::new(2.4, Color32::from_rgb(255, 214, 90));
            for contour in &preview.contours {
                let mut points: Vec<Pos2> = contour
                    .points
                    .iter()
                    .map(|point| self.view.world_to_screen(area, *point))
                    .collect();
                if contour.closed && points.len() > 1 {
                    points.push(points[0]);
                }
                if points.len() >= 2 {
                    painter.add(egui::Shape::line(points, stroke));
                }
            }

            if self.node_edit.is_none() {
                let bounds = shape.bounds();
                let rect = Rect::from_two_pos(
                    self.view
                        .world_to_screen(area, Point::new(bounds.x0, bounds.y0)),
                    self.view
                        .world_to_screen(area, Point::new(bounds.x1, bounds.y1)),
                )
                .expand(4.0);
                painter.rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(1.0, Color32::from_rgb(220, 185, 70)),
                    StrokeKind::Inside,
                );
            }
        }

        if self.node_edit.is_none() {
            if let Some(bounds) = self.selection_bounds() {
                let rect = Rect::from_two_pos(
                    self.view
                        .world_to_screen(area, Point::new(bounds.x0, bounds.y0)),
                    self.view
                        .world_to_screen(area, Point::new(bounds.x1, bounds.y1)),
                );
                painter.rect_stroke(
                    rect.expand(4.0),
                    0.0,
                    Stroke::new(1.0, Color32::from_rgb(255, 230, 150)),
                    StrokeKind::Inside,
                );

                let handles = selection_handle_positions(rect);
                let rotate = handles[0].1;
                painter.line_segment(
                    [Pos2::new(rect.center().x, rect.top()), rotate],
                    Stroke::new(1.0, Color32::from_rgb(220, 185, 70)),
                );
                for (handle, position) in handles {
                    match handle {
                        Handle::Rotate => {
                            painter.circle_filled(position, 5.0, Color32::from_rgb(255, 225, 120));
                            painter.circle_stroke(
                                position,
                                5.0,
                                Stroke::new(1.0, Color32::from_rgb(120, 90, 30)),
                            );
                        }
                        Handle::Scale(_) => {
                            let square = Rect::from_center_size(position, egui::vec2(8.0, 8.0));
                            painter.rect_filled(square, 1.0, Color32::from_rgb(255, 232, 140));
                            painter.rect_stroke(
                                square,
                                1.0,
                                Stroke::new(1.0, Color32::from_rgb(120, 90, 30)),
                                StrokeKind::Inside,
                            );
                        }
                    }
                }
            }
        }
    }

    fn draw_nodes(&self, painter: &egui::Painter, area: Rect) {
        let Some(shape) = self.node_shape() else {
            return;
        };

        for (start, end) in nodes::control_links(&shape.path) {
            painter.line_segment(
                [
                    self.view.world_to_screen(area, start),
                    self.view.world_to_screen(area, end),
                ],
                Stroke::new(1.0, Color32::from_rgb(120, 110, 70)),
            );
        }

        let path_nodes = nodes::nodes(&shape.path);
        for node in &path_nodes {
            let position = self.view.world_to_screen(area, node.point);
            if node.is_anchor() {
                let square = Rect::from_center_size(position, egui::vec2(8.0, 8.0));
                painter.rect_filled(square, 1.0, Color32::from_rgb(255, 232, 140));
                painter.rect_stroke(
                    square,
                    1.0,
                    Stroke::new(1.0, Color32::from_rgb(120, 90, 30)),
                    StrokeKind::Inside,
                );
            } else {
                painter.circle_filled(position, 3.5, Color32::from_rgb(35, 35, 35));
                painter.circle_stroke(
                    position,
                    3.5,
                    Stroke::new(1.0, Color32::from_rgb(230, 220, 160)),
                );
            }
        }

        for selected in &self.selected_nodes {
            if let Some(node) = path_nodes.iter().find(|node| node.node == *selected) {
                let position = self.view.world_to_screen(area, node.point);
                painter.circle_stroke(position, 7.0, Stroke::new(2.0, Color32::WHITE));
            }
        }
    }

    fn draw_origin(&self, painter: &egui::Painter, area: Rect) {
        if self.doc.is_empty() {
            return;
        }
        let origin = self.view.world_to_screen(area, Point::ORIGIN);
        painter.circle_filled(origin, 3.0, Color32::from_rgb(230, 190, 90));
    }
}

fn selection_handle_positions(rect: Rect) -> [(Handle, Pos2); 5] {
    [
        (
            Handle::Rotate,
            Pos2::new(rect.center().x, rect.top() - 22.0),
        ),
        (Handle::Scale(HandleCorner::TopLeft), rect.left_top()),
        (Handle::Scale(HandleCorner::TopRight), rect.right_top()),
        (
            Handle::Scale(HandleCorner::BottomRight),
            rect.right_bottom(),
        ),
        (Handle::Scale(HandleCorner::BottomLeft), rect.left_bottom()),
    ]
}

/// Nearest anchor or segment point among `paths`, preferring anchors when both
/// are within `tolerance`.
fn nearest_geometry_point(
    paths: &[&kurbo::BezPath],
    desired: Point,
    tolerance: f64,
) -> Option<Point> {
    let mut anchor: Option<(f64, Point)> = None;
    let mut segment: Option<(f64, Point)> = None;
    for path in paths {
        for node in nodes::nodes(path) {
            if !node.is_anchor() {
                continue;
            }
            let distance = (node.point - desired).hypot();
            if distance <= tolerance && anchor.is_none_or(|(best, _)| distance < best) {
                anchor = Some((distance, node.point));
            }
        }
        if let Some(hit) = nodes::nearest_segment(path, desired) {
            if hit.distance <= tolerance && segment.is_none_or(|(best, _)| hit.distance < best) {
                segment = Some((hit.distance, hit.point));
            }
        }
    }
    anchor.or(segment).map(|(_, point)| point)
}

/// Translation needed to align `bounds` with the selection bounds `target`.
fn align_delta(bounds: kurbo::Rect, target: kurbo::Rect, axis: AlignAxis, edge: AlignEdge) -> f64 {
    match (axis, edge) {
        (AlignAxis::Horizontal, AlignEdge::Start) => target.x0 - bounds.x0,
        (AlignAxis::Horizontal, AlignEdge::Center) => target.center().x - bounds.center().x,
        (AlignAxis::Horizontal, AlignEdge::End) => target.x1 - bounds.x1,
        (AlignAxis::Vertical, AlignEdge::Start) => target.y0 - bounds.y0,
        (AlignAxis::Vertical, AlignEdge::Center) => target.center().y - bounds.center().y,
        (AlignAxis::Vertical, AlignEdge::End) => target.y1 - bounds.y1,
    }
}

/// Per-item translations that keep the first and last centres fixed and space
/// the intermediate centres evenly. Returned in the same order as `bounds`.
fn distribute_deltas(bounds: &[kurbo::Rect], axis: AlignAxis) -> Vec<f64> {
    let mut deltas = vec![0.0; bounds.len()];
    if bounds.len() < 3 {
        return deltas;
    }
    let center = |rect: &kurbo::Rect| match axis {
        AlignAxis::Horizontal => rect.center().x,
        AlignAxis::Vertical => rect.center().y,
    };
    let mut order: Vec<usize> = (0..bounds.len()).collect();
    order.sort_by(|&a, &b| center(&bounds[a]).total_cmp(&center(&bounds[b])));
    let first = center(&bounds[order[0]]);
    let last = center(&bounds[order[order.len() - 1]]);
    let step = (last - first) / (order.len() - 1) as f64;
    for (rank, &index) in order.iter().enumerate() {
        let target = first + step * rank as f64;
        deltas[index] = target - center(&bounds[index]);
    }
    deltas
}

fn opposite_corner(corner: HandleCorner) -> HandleCorner {
    match corner {
        HandleCorner::TopLeft => HandleCorner::BottomRight,
        HandleCorner::TopRight => HandleCorner::BottomLeft,
        HandleCorner::BottomRight => HandleCorner::TopLeft,
        HandleCorner::BottomLeft => HandleCorner::TopRight,
    }
}

fn scale_about(anchor: Point, factor: f64) -> Affine {
    Affine::translate((anchor.x, anchor.y))
        * Affine::scale(factor)
        * Affine::translate((-anchor.x, -anchor.y))
}

fn raster_resize_frame(
    corner: HandleCorner,
    pointer: Point,
    initial_center: Point,
    angle: f64,
    anchor_world: Point,
    anchor_local: Point,
) -> (f64, f64, f64, f64) {
    let local = inverse_rotate_about(pointer, initial_center, angle);
    let minimum = 0.1;
    let (x, y, width, height) = match corner {
        HandleCorner::TopLeft => {
            let px = local.x.min(anchor_local.x - minimum);
            let py = local.y.max(anchor_local.y + minimum);
            (px, anchor_local.y, anchor_local.x - px, py - anchor_local.y)
        }
        HandleCorner::TopRight => {
            let px = local.x.max(anchor_local.x + minimum);
            let py = local.y.max(anchor_local.y + minimum);
            (
                anchor_local.x,
                anchor_local.y,
                px - anchor_local.x,
                py - anchor_local.y,
            )
        }
        HandleCorner::BottomRight => {
            let px = local.x.max(anchor_local.x + minimum);
            let py = local.y.min(anchor_local.y - minimum);
            (anchor_local.x, py, px - anchor_local.x, anchor_local.y - py)
        }
        HandleCorner::BottomLeft => {
            let px = local.x.min(anchor_local.x - minimum);
            let py = local.y.min(anchor_local.y - minimum);
            (px, py, anchor_local.x - px, anchor_local.y - py)
        }
    };
    let new_center = Point::new(x + width * 0.5, y + height * 0.5);
    let (sin, cos) = angle.to_radians().sin_cos();
    let dx = anchor_local.x - new_center.x;
    let dy = anchor_local.y - new_center.y;
    let rotated = Point::new(dx * cos - dy * sin, dx * sin + dy * cos);
    let center_world = Point::new(anchor_world.x - rotated.x, anchor_world.y - rotated.y);
    (
        center_world.x - width * 0.5,
        center_world.y - height * 0.5,
        width,
        height,
    )
}

fn inverse_rotate_about(point: Point, center: Point, angle_deg: f64) -> Point {
    let (sin, cos) = angle_deg.to_radians().sin_cos();
    let dx = point.x - center.x;
    let dy = point.y - center.y;
    Point::new(
        center.x + dx * cos + dy * sin,
        center.y - dx * sin + dy * cos,
    )
}

fn rotate_about(center: Point, radians: f64) -> Affine {
    Affine::translate((center.x, center.y))
        * Affine::rotate(radians)
        * Affine::translate((-center.x, -center.y))
}

fn corner_point(bounds: kurbo::Rect, corner: HandleCorner) -> Point {
    match corner {
        HandleCorner::TopLeft => Point::new(bounds.x0, bounds.y1),
        HandleCorner::TopRight => Point::new(bounds.x1, bounds.y1),
        HandleCorner::BottomRight => Point::new(bounds.x1, bounds.y0),
        HandleCorner::BottomLeft => Point::new(bounds.x0, bounds.y0),
    }
}

fn normalize_angle(mut angle: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    while angle > PI {
        angle -= TAU;
    }
    while angle < -PI {
        angle += TAU;
    }
    angle
}

fn distance_to_contour(contour: &crate::preview::PreviewContour, point: Point) -> f64 {
    let points = &contour.points;
    let mut best = f64::INFINITY;
    for window in points.windows(2) {
        best = best.min(distance_to_segment(point, window[0], window[1]));
    }
    if contour.closed && points.len() > 1 {
        best = best.min(distance_to_segment(
            point,
            points[points.len() - 1],
            points[0],
        ));
    }
    best
}

fn distance_to_segment(point: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let ap = point - a;
    let length_squared = ab.x * ab.x + ab.y * ab.y;
    if length_squared <= 1e-12 {
        return ap.hypot();
    }
    let t = ((ap.x * ab.x + ap.y * ab.y) / length_squared).clamp(0.0, 1.0);
    let projected = Point::new(a.x + t * ab.x, a.y + t * ab.y);
    (point - projected).hypot()
}

fn is_image_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("png")
                || extension.eq_ignore_ascii_case("jpg")
                || extension.eq_ignore_ascii_case("jpeg")
                || extension.eq_ignore_ascii_case("bmp")
        })
}

fn is_project_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("orleans"))
        .unwrap_or(false)
}

const FRAME_FEED: f64 = 3000.0;

/// A short absolute program that traces the job bounds with the laser off.
fn frame_program(bounds: kurbo::Rect, feed: f64) -> String {
    let (x0, y0, x1, y1) = (bounds.x0, bounds.y0, bounds.x1, bounds.y1);
    format!(
        "; frame (laser off)\nG21\nG90\nG54\nM5\n\
         G0 X{x0:.3} Y{y0:.3}\n\
         G1 X{x1:.3} Y{y0:.3} F{feed:.0}\n\
         G1 X{x1:.3} Y{y1:.3}\n\
         G1 X{x0:.3} Y{y1:.3}\n\
         G1 X{x0:.3} Y{y0:.3}\nM5\n"
    )
}

fn state_color(state: &str) -> Color32 {
    if state.starts_with("Alarm") || state.starts_with("Door") {
        Color32::from_rgb(235, 100, 90)
    } else if state == "Run" || state == "Jog" {
        Color32::from_rgb(230, 190, 90)
    } else if state == "Home" {
        Color32::from_rgb(120, 170, 240)
    } else if state == "Idle" {
        Color32::from_rgb(120, 200, 130)
    } else {
        Color32::from_gray(180)
    }
}

/// Layer colours come from the SVG, and black is common. On a dark canvas the
/// original colour would be invisible, so lift dark colours toward white while
/// keeping their hue. Bright or saturated colours are left untouched.
fn display_color(rgb: [u8; 3]) -> Color32 {
    let r = f32::from(rgb[0]);
    let g = f32::from(rgb[1]);
    let b = f32::from(rgb[2]);
    let brightest = r.max(g).max(b);
    if brightest >= 140.0 {
        return Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    }
    let t = (150.0 - brightest) / (255.0 - brightest).max(1.0);
    let lift = |c: f32| (c * (1.0 - t) + 255.0 * t).round() as u8;
    Color32::from_rgb(lift(r), lift(g), lift(b))
}

fn snap_point(point: Point, step: f64) -> Point {
    if !step.is_finite() || step <= 0.0 {
        return point;
    }
    Point::new(
        (point.x / step).round() * step,
        (point.y / step).round() * step,
    )
}

fn nice_step(target: f64) -> f64 {
    if !target.is_finite() || target <= 0.0 {
        return 10.0;
    }
    let exponent = target.log10().floor();
    let base = 10f64.powf(exponent);
    let n = target / base;
    let m = if n <= 1.0 {
        1.0
    } else if n <= 2.0 {
        2.0
    } else if n <= 5.0 {
        5.0
    } else {
        10.0
    };
    m * base
}

fn format_duration(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    if total >= 3600 {
        format!("{}h {:02}m", total / 3600, (total % 3600) / 60)
    } else if total >= 60 {
        format!("{}m {:02}s", total / 60, total % 60)
    } else {
        format!("{total}s")
    }
}

impl eframe::App for OrleansApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump_machine(&ctx);
        self.ensure_raster_textures(&ctx);
        self.begin_settings_edit();

        if self.dirty {
            self.replan();
            self.dirty = false;
        }

        let node_editing = self.node_edit.is_some();
        let keyboard_input = ctx.egui_wants_keyboard_input();
        let mut action: Option<Action> = None;
        ctx.input(|i| {
            if i.modifiers.command && i.key_pressed(egui::Key::O) {
                action = Some(Action::Open);
            }
            if i.modifiers.command && i.key_pressed(egui::Key::E) {
                action = Some(Action::Export);
            }
            if i.modifiers.command && i.key_pressed(egui::Key::S) {
                action = Some(Action::SaveProject);
            }
            if i.modifiers.command && i.key_pressed(egui::Key::Z) {
                action = Some(if i.modifiers.shift {
                    Action::Redo
                } else {
                    Action::Undo
                });
            }
            if i.modifiers.command && i.key_pressed(egui::Key::Y) {
                action = Some(Action::Redo);
            }
            if !i.modifiers.command && i.key_pressed(egui::Key::F) {
                action = Some(Action::Fit);
            }
            if i.modifiers.command && i.key_pressed(egui::Key::Enter) {
                action = Some(Action::Send);
            }
            if node_editing && !keyboard_input && i.key_pressed(egui::Key::Escape) {
                action = Some(Action::ExitNodes);
            }
            if node_editing && !keyboard_input && i.key_pressed(egui::Key::Delete) {
                action = Some(Action::DeleteNode);
            }
        });

        self.toolbar(ui, &mut action);
        self.side_panel(ui);
        self.machine_panel(ui);
        self.bottom_bar(ui);
        self.canvas_panel(ui);

        // Commit a finished settings gesture before acting on undo/redo, so the
        // edit that just ended is the one they operate on.
        self.end_settings_edit(&ctx);

        match action {
            Some(Action::Open) => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("SVG", &["svg"])
                    .add_filter("Image", &["png", "jpg", "jpeg", "bmp"])
                    .add_filter("Orleans project", &["orleans"])
                    .set_title("Open design, image or project")
                    .pick_file()
                {
                    self.open_path(path);
                }
            }
            Some(Action::Export) => self.export(),
            Some(Action::SaveProject) => self.save_project(),
            Some(Action::Undo) => self.undo_edit(),
            Some(Action::Redo) => self.redo_edit(),
            Some(Action::ExitNodes) => self.exit_node_edit(),
            Some(Action::DeleteNode) => self.delete_selected_node(),
            Some(Action::Reload) => {
                if let Some(path) = self.project_path.clone() {
                    self.load_project(path);
                } else if let Some(path) = self.path.clone() {
                    self.open_path(path);
                } else {
                    self.status = Status::error("nothing to reload");
                }
            }
            Some(Action::Fit) => self.fit_pending = true,
            Some(Action::Send) => {
                if self.can_send() {
                    self.confirm_send = true;
                } else {
                    self.set_status(
                        "cannot send: connect to an idle controller with the laser off",
                        true,
                    );
                }
            }
            None => {}
        }

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        if let Some(path) = dropped.into_iter().next() {
            self.open_path(path);
        }

        self.home_dialog(&ctx);
        self.send_dialog(&ctx);
        self.settings_window(&ctx);
    }
}

impl Drop for OrleansApp {
    fn drop(&mut self) {
        // Closing the window during a cut must not leave the laser on.
        if self.job_running {
            self.machine.stop_job();
            std::thread::sleep(Duration::from_millis(400));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_steps_are_round_numbers() {
        assert_eq!(nice_step(0.7), 1.0);
        assert_eq!(nice_step(1.6), 2.0);
        assert_eq!(nice_step(3.0), 5.0);
        assert_eq!(nice_step(7.0), 10.0);
        assert_eq!(nice_step(12.0), 20.0);
        assert_eq!(nice_step(-1.0), 10.0);
    }

    #[test]
    fn dark_layer_colours_stay_visible() {
        assert_eq!(display_color([0, 0, 0]), Color32::from_rgb(150, 150, 150));
        // Bright or saturated colours are untouched.
        assert_eq!(display_color([255, 0, 0]), Color32::from_rgb(255, 0, 0));
        assert_eq!(display_color([0, 0, 255]), Color32::from_rgb(0, 0, 255));
    }

    #[test]
    fn frame_program_traces_bounds_without_firing() {
        let program = frame_program(kurbo::Rect::new(1.0, 2.0, 30.0, 40.0), 3000.0);
        assert!(program.starts_with("; frame (laser off)\nG21\nG90\nG54\nM5\n"));
        assert!(program.contains("G0 X1.000 Y2.000"));
        assert!(program.contains("G1 X30.000 Y2.000 F3000"));
        assert!(program.contains("G1 X30.000 Y40.000"));
        assert!(program.contains("G1 X1.000 Y40.000"));
        assert!(program.trim_end().ends_with("G1 X1.000 Y2.000\nM5"));
        assert_eq!(program.matches("M5").count(), 2);
    }

    #[test]
    fn handle_positions_match_the_selection_box() {
        let rect = Rect::from_min_size(Pos2::new(100.0, 100.0), egui::vec2(80.0, 40.0));
        assert_eq!(
            selection_handle_positions(rect),
            [
                (Handle::Rotate, Pos2::new(140.0, 78.0)),
                (
                    Handle::Scale(HandleCorner::TopLeft),
                    Pos2::new(100.0, 100.0)
                ),
                (
                    Handle::Scale(HandleCorner::TopRight),
                    Pos2::new(180.0, 100.0)
                ),
                (
                    Handle::Scale(HandleCorner::BottomRight),
                    Pos2::new(180.0, 140.0)
                ),
                (
                    Handle::Scale(HandleCorner::BottomLeft),
                    Pos2::new(100.0, 140.0)
                ),
            ]
        );
    }

    #[test]
    fn raster_resize_keeps_the_opposite_corner_fixed() {
        let (x, y, width, height) = raster_resize_frame(
            HandleCorner::TopRight,
            Point::new(12.0, 4.0),
            Point::new(5.0, 2.5),
            0.0,
            Point::new(0.0, 0.0),
            Point::new(0.0, 0.0),
        );
        assert!((x - 0.0).abs() < 1e-9);
        assert!((y - 0.0).abs() < 1e-9);
        assert!((width - 12.0).abs() < 1e-9);
        assert!((height - 4.0).abs() < 1e-9);

        let center = Point::new(5.0, 2.5);
        let angle = 45.0_f64;
        let anchor_local = Point::new(0.0, 0.0);
        let (sin, cos) = angle.to_radians().sin_cos();
        let anchor_world = Point::new(
            center.x - center.x * cos + center.y * sin,
            center.y - center.x * sin - center.y * cos,
        );
        let (x, y, width, height) = raster_resize_frame(
            HandleCorner::TopRight,
            Point::new(20.0, 0.0),
            center,
            angle,
            anchor_world,
            anchor_local,
        );
        let new_center = Point::new(x + width * 0.5, y + height * 0.5);
        let dx = x - new_center.x;
        let dy = y - new_center.y;
        let rotated = Point::new(dx * cos - dy * sin, dx * sin + dy * cos);
        let new_anchor = Point::new(new_center.x + rotated.x, new_center.y + rotated.y);
        assert!((new_anchor - anchor_world).hypot() < 1e-9);
        assert!(width > 0.0 && height > 0.0);
    }

    #[test]
    fn selection_anchor_points_follow_the_bounds() {
        let bounds = kurbo::Rect::new(10.0, 20.0, 30.0, 50.0);
        assert_eq!(
            SelectionAnchor::BottomLeft.point(bounds),
            Point::new(10.0, 20.0)
        );
        assert_eq!(
            SelectionAnchor::TopRight.point(bounds),
            Point::new(30.0, 50.0)
        );
        assert_eq!(
            SelectionAnchor::Center.point(bounds),
            Point::new(20.0, 35.0)
        );
        assert_eq!(
            SelectionAnchor::Bottom.point(bounds),
            Point::new(20.0, 20.0)
        );
        assert_eq!(SelectionAnchor::Right.point(bounds), Point::new(30.0, 35.0));
    }

    #[test]
    fn snapping_rounds_to_the_grid() {
        assert_eq!(
            snap_point(Point::new(12.4, -3.0), 5.0),
            Point::new(10.0, -5.0)
        );
        let unchanged = Point::new(12.4, 3.0);
        assert_eq!(snap_point(unchanged, 0.0), unchanged);
        assert_eq!(snap_point(unchanged, f64::NAN), unchanged);
    }

    #[test]
    fn handle_corners_use_world_orientation() {
        let bounds = kurbo::Rect::new(10.0, 20.0, 30.0, 50.0);
        assert_eq!(
            corner_point(bounds, HandleCorner::TopLeft),
            Point::new(10.0, 50.0)
        );
        assert_eq!(
            corner_point(bounds, HandleCorner::TopRight),
            Point::new(30.0, 50.0)
        );
        assert_eq!(
            corner_point(bounds, HandleCorner::BottomRight),
            Point::new(30.0, 20.0)
        );
        assert_eq!(
            corner_point(bounds, HandleCorner::BottomLeft),
            Point::new(10.0, 20.0)
        );
        assert_eq!(
            opposite_corner(HandleCorner::TopLeft),
            HandleCorner::BottomRight
        );
        assert_eq!(
            opposite_corner(HandleCorner::TopRight),
            HandleCorner::BottomLeft
        );
    }

    #[test]
    fn anchored_transforms_keep_their_origin_fixed() {
        let anchor = Point::new(10.0, 20.0);
        let scaled_anchor = scale_about(anchor, 2.5) * anchor;
        assert!((scaled_anchor - anchor).hypot() < 1e-9);

        let scaled = scale_about(anchor, 2.0) * Point::new(12.0, 20.0);
        assert!((scaled.x - 14.0).abs() < 1e-9);
        assert!((scaled.y - 20.0).abs() < 1e-9);

        let center = Point::new(5.0, 5.0);
        let rotated_center = rotate_about(center, std::f64::consts::FRAC_PI_2) * center;
        assert!((rotated_center - center).hypot() < 1e-9);

        let rotated = rotate_about(center, std::f64::consts::FRAC_PI_2) * Point::new(7.0, 5.0);
        assert!((rotated.x - 5.0).abs() < 1e-9);
        assert!((rotated.y - 7.0).abs() < 1e-9);
    }

    #[test]
    fn rotation_deltas_use_the_short_turn() {
        use std::f64::consts::{FRAC_PI_2, PI};
        assert!((normalize_angle(3.0 * FRAC_PI_2) + FRAC_PI_2).abs() < 1e-9);
        assert!((normalize_angle(-3.0 * FRAC_PI_2) - FRAC_PI_2).abs() < 1e-9);
        assert!((normalize_angle(PI) - PI).abs() < 1e-9);
    }

    #[test]
    fn durations_are_human_readable() {
        assert_eq!(format_duration(0.0), "0s");
        assert_eq!(format_duration(42.4), "42s");
        assert_eq!(format_duration(61.0), "1m 01s");
        assert_eq!(format_duration(3725.0), "1h 02m");
    }

    #[test]
    fn geometry_snapping_prefers_anchors_then_segments() {
        let line = kurbo::BezPath::from_svg("M0,0 L100,0").expect("line");
        let other = kurbo::BezPath::from_svg("M200,0 L200,100").expect("other");
        let paths = [&line, &other];

        let segment =
            nearest_geometry_point(&paths, Point::new(50.0, 2.0), 5.0).expect("segment snap");
        assert!((segment - Point::new(50.0, 0.0)).hypot() < 1e-9);

        let anchor =
            nearest_geometry_point(&paths, Point::new(1.0, 1.0), 5.0).expect("anchor snap");
        assert_eq!(anchor, Point::new(0.0, 0.0));

        assert!(nearest_geometry_point(&paths, Point::new(500.0, 500.0), 5.0).is_none());
    }

    #[test]
    fn align_deltas_match_edges_and_centres() {
        let target = kurbo::Rect::new(0.0, 0.0, 100.0, 50.0);
        let bounds = kurbo::Rect::new(10.0, 5.0, 30.0, 15.0);
        let cases = [
            (AlignAxis::Horizontal, AlignEdge::Start, -10.0),
            (AlignAxis::Horizontal, AlignEdge::Center, 30.0),
            (AlignAxis::Horizontal, AlignEdge::End, 70.0),
            (AlignAxis::Vertical, AlignEdge::Start, -5.0),
            (AlignAxis::Vertical, AlignEdge::Center, 15.0),
            (AlignAxis::Vertical, AlignEdge::End, 35.0),
        ];
        for (axis, edge, expected) in cases {
            assert_eq!(align_delta(bounds, target, axis, edge), expected);
        }
    }

    #[test]
    fn distribute_moves_only_the_intermediate_centres() {
        let rect = |x: f64| kurbo::Rect::new(x - 1.0, 0.0, x + 1.0, 1.0);
        // Input order is deliberately not left-to-right.
        let bounds = [rect(10.0), rect(40.0), rect(0.0)];
        let deltas = distribute_deltas(&bounds, AlignAxis::Horizontal);
        assert_eq!(deltas, vec![10.0, 0.0, 0.0]);

        let bounds = [rect(0.0), rect(10.0), rect(40.0)];
        let deltas = distribute_deltas(&bounds, AlignAxis::Horizontal);
        assert_eq!(deltas, vec![0.0, 10.0, 0.0]);
    }

    #[test]
    fn settings_snapshot_restores_layers_order_and_job() {
        use orleans_core::{CutParams, Layer, RasterImage};

        let mut doc = Document::new(100.0, 80.0);
        let mut outer = Layer::new(0, "outer", [200, 10, 10], CutParams::new(1200.0, 60.0, 2));
        outer.mode = LayerMode::Fill;
        outer.fill.spacing = 0.4;
        outer.tabs.count = 3;
        let mut image = Layer::new(1, "image", [10, 200, 10], CutParams::new(800.0, 40.0, 1));
        image.mode = LayerMode::Raster;
        image.visible = false;
        image.output = false;
        image.raster = Some(RasterImage {
            width: 2,
            height: 2,
            grayscale: vec![0, 64, 128, 255],
            x: 5.0,
            y: 6.0,
            width_mm: 20.0,
            height_mm: 10.0,
            angle: 15.0,
            mode: RasterMode::Dither,
            threshold: 90,
            invert: true,
            line_interval: 0.3,
            bidirectional: false,
        });
        doc.layers.push(image);
        doc.layers.push(outer);

        let mut plan = PlanOptions {
            kerf: 0.15,
            optimize: false,
            ..PlanOptions::default()
        };
        let mut gcode = GcodeOptions {
            laser_mode: LaserMode::M4,
            max_s: 750,
            ..GcodeOptions::default()
        };
        let saved = SettingsSnapshot::capture(&doc, &plan, &gcode);

        doc.layers[0].name = "changed".to_owned();
        doc.layers[0].visible = true;
        doc.layers[0].output = true;
        doc.layers[0].raster.as_mut().expect("raster layer").angle = -45.0;
        doc.layers[1].cut.speed = 100.0;
        doc.layers[1].mode = LayerMode::Line;
        doc.layers.swap(0, 1);
        plan.kerf = 0.5;
        plan.optimize = true;
        gcode.laser_mode = LaserMode::M3;
        gcode.max_s = 1000;

        saved.apply(&mut doc, &mut plan, &mut gcode);

        assert_eq!(
            doc.layers
                .iter()
                .map(|layer| layer.index)
                .collect::<Vec<_>>(),
            vec![1, 0]
        );
        let image = &doc.layers[0];
        assert_eq!(image.name, "image");
        assert!(!image.visible);
        assert!(!image.output);
        let raster = image.raster.as_ref().expect("raster layer");
        assert_eq!(raster.angle, 15.0);
        assert_eq!(raster.mode, RasterMode::Dither);
        assert_eq!(raster.threshold, 90);
        assert!(raster.invert);
        assert_eq!(raster.line_interval, 0.3);
        assert!(!raster.bidirectional);

        let outer = &doc.layers[1];
        assert_eq!(outer.name, "outer");
        assert_eq!(outer.mode, LayerMode::Fill);
        assert_eq!(outer.cut.speed, 1200.0);
        assert_eq!(outer.fill.spacing, 0.4);
        assert_eq!(outer.tabs.count, 3);

        assert_eq!(plan.kerf, 0.15);
        assert!(!plan.optimize);
        assert_eq!(gcode.laser_mode, LaserMode::M4);
        assert_eq!(gcode.max_s, 750);
    }
}
