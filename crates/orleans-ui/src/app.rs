//! The main application window: toolbar, layer panel, canvas, status bar.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, StrokeKind};
use kurbo::{Affine, Point};
use orleans_core::{Document, Job, Op};
use orleans_driver::gcode::{self, GcodeOptions, LaserMode};
use orleans_geom::point_in_polygon;
use orleans_import::{import_svg, ImportOptions};
use orleans_toolpath::{plan, PlanOptions};

use crate::machine::{Event as MachineEvent, Machine, Settings, Status as MachineStatus};
use crate::nodes::{self, NodeKind, NodeRef};
use crate::preview::Preview;
use crate::project::{EditedPath, ProjectFile, ShapeTransform, FORMAT};
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

struct UndoSnapshot {
    shapes: Vec<(usize, String, kurbo::BezPath)>,
    transforms: ShapeTransforms,
    edited_paths: BTreeSet<(usize, String)>,
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
    ExitNodes,
    DeleteNode,
}

pub struct OrleansApp {
    doc: Document,
    preview: Preview,
    selected_layer: Option<usize>,
    selected_shapes: BTreeSet<ShapeRef>,
    shape_transforms: ShapeTransforms,
    undo_stack: Vec<UndoSnapshot>,
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
    node_edit: Option<ShapeRef>,
    selected_node: Option<NodeRef>,
    node_drag: Option<NodeDrag>,
    node_snap: bool,
    edited_paths: BTreeSet<(usize, String)>,
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
            shape_transforms: ShapeTransforms::new(),
            undo_stack: Vec::new(),
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
            node_edit: None,
            selected_node: None,
            node_drag: None,
            node_snap: false,
            edited_paths: BTreeSet::new(),
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
                self.selected_shapes.clear();
                self.shape_transforms.clear();
                self.undo_stack.clear();
                self.dragging_shape = false;
                self.active_handle = None;
                self.node_edit = None;
                self.selected_node = None;
                self.node_drag = None;
                self.edited_paths.clear();
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

    fn open_path(&mut self, path: PathBuf) {
        if is_project_path(&path) {
            self.load_project(path);
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
        match import_svg(project.svg.as_bytes(), &ImportOptions::default()) {
            Ok(mut doc) if !doc.is_empty() => {
                project.apply(&mut doc);
                self.preview = Preview::build(&doc, DISPLAY_TOLERANCE);
                self.doc = doc;
                self.selected_layer = self.doc.layers.first().map(|layer| layer.index);
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
                self.undo_stack.clear();
                self.dragging_shape = false;
                self.active_handle = None;
                self.node_edit = None;
                self.selected_node = None;
                self.node_drag = None;
                self.edited_paths = project
                    .edited_paths
                    .iter()
                    .map(|edited| (edited.layer, edited.shape.clone()))
                    .collect();
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
            Ok(_) => {
                self.set_status(format!("{} contains no paths", path.display()), true);
            }
            Err(err) => {
                self.set_status(format!("importing project SVG: {err}"), true);
            }
        }
    }

    fn save_project(&mut self) {
        let Some(svg) = self.source_svg.clone() else {
            self.set_status("open an SVG before saving a project", true);
            return;
        };
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
        self.undo_stack.push(UndoSnapshot {
            shapes,
            transforms: self.shape_transforms.clone(),
            edited_paths: self.edited_paths.clone(),
        });
        if self.undo_stack.len() > 30 {
            self.undo_stack.remove(0);
        }
    }

    fn undo_shape_move(&mut self) {
        let Some(snapshot) = self.undo_stack.pop() else {
            self.set_status("nothing to undo", false);
            return;
        };
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
        self.shape_transforms = snapshot.transforms;
        self.edited_paths = snapshot.edited_paths;
        self.node_drag = None;
        self.selected_node = None;
        self.preview = Preview::build(&self.doc, DISPLAY_TOLERANCE);
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
        self.dragging_shape = false;
        self.active_handle = None;
        self.set_status("undid edit", false);
    }

    fn apply_affine_to_selection(&mut self, affine: Affine) {
        let selected: Vec<ShapeRef> = self.selected_shapes.iter().cloned().collect();
        for selection in selected {
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
        self.selected_node = Some(node);
        self.push_selection_undo();
        self.node_drag = Some(NodeDrag {
            node,
            start_world: world,
            start_node,
        });
        self.dragging_shape = false;
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
            let mut desired = drag.start_node + (world - drag.start_world);
            if snap {
                desired = snap_point(desired, step);
            }
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
        self.selected_node = Some(node);
        self.dirty = true;
        self.set_status("added node", false);
        true
    }

    fn delete_selected_node(&mut self) {
        let Some(node) = self.selected_node else {
            self.set_status("select an anchor to delete", true);
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
        self.selected_node = None;
        self.node_drag = None;
        self.dirty = true;
        self.set_status("deleted node", false);
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
        self.selected_node = None;
        self.node_drag = None;
        self.active_handle = None;
        self.dragging_shape = false;
        self.set_status("node editing: drag anchors and control points", false);
    }

    fn exit_node_edit(&mut self) {
        if self.node_edit.take().is_some() {
            self.selected_node = None;
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
                    ui.label(RichText::new("No layers yet. Open an SVG or drop one here.").weak());
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
                                let shapes = layer.shapes.len();
                                let summary = format!(
                                    "{:.0} mm/min  ·  {:.0}%  ·  ×{}",
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
                                                        RichText::new(format!(
                                                            "{shapes} shape{}",
                                                            if shapes == 1 { "" } else { "s" }
                                                        ))
                                                        .small()
                                                        .weak(),
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
                        if !self
                            .selected_shapes
                            .iter()
                            .any(|shape| shape.layer == index)
                        {
                            self.selected_shapes.clear();
                        }
                        self.selected_layer = Some(index);
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
                            ui.color_edit_button_srgb(&mut layer.color);
                            ui.add(
                                egui::TextEdit::singleline(&mut layer.name)
                                    .hint_text("Layer name")
                                    .desired_width(f32::INFINITY),
                            );
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
                            ui.label(RichText::new("· Line").small().weak());
                        });
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
                    "Drag nodes · dbl-click segment adds · Del removes · Esc finishes"
                } else if selected_count == 0 {
                    "Click a shape · Ctrl/Shift-click for multiple · drag to move · right-drag to pan"
                } else {
                    "Corner handles scale · top handle rotates · Ctrl+Z to undo"
                };
                ui.label(RichText::new(hint).small().weak());

                if selected_count > 0 {
                    ui.horizontal(|ui| {
                        ui.strong(format!("Selection ({selected_count})"));
                    });
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
                                let can_delete = matches!(
                                    self.selected_node,
                                    Some(node) if node.kind == NodeKind::Anchor
                                );
                                if ui
                                    .add_enabled(can_delete, egui::Button::new("Delete node"))
                                    .on_hover_text("Delete the selected anchor (Del)")
                                    .clicked()
                                {
                                    self.delete_selected_node();
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
                        ui.add(
                            egui::DragValue::new(&mut self.gcode_opts.max_s)
                                .speed(10.0)
                                .range(1..=100_000),
                        );
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
                }

                if changed {
                    self.dirty = true;
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
                    ui.label(format!(
                        "{} layers · {} shapes · {} cuts",
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
        self.draw_origin(&painter, area);

        let additive = ui.input(|input| input.modifiers.command || input.modifiers.shift);
        let node_editing = self.node_edit.is_some();

        if response.clicked_by(egui::PointerButton::Primary) {
            if let Some(position) = response.interact_pointer_pos() {
                if node_editing {
                    self.selected_node = self.node_at(position, area);
                } else if self.selection_handle_at(position, area).is_none() {
                    let world = self.view.screen_to_world(area, position);
                    let hit = self.hit_test(world);
                    if let Some(hit) = hit {
                        if additive {
                            if !self.selected_shapes.remove(&hit) {
                                self.selected_shapes.insert(hit.clone());
                            }
                        } else {
                            self.selected_shapes.clear();
                            self.selected_shapes.insert(hit.clone());
                        }
                        self.selected_layer = Some(hit.layer);
                    } else if !additive {
                        self.selected_shapes.clear();
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
                let mut handle_started = false;
                if let Some(position) = origin {
                    if let Some(handle) = self.selection_handle_at(position, area) {
                        let world = self.view.screen_to_world(area, position);
                        self.begin_handle_drag(handle, world);
                        handle_started = true;
                    }
                }
                if !handle_started {
                    let world = origin.map(|position| self.view.screen_to_world(area, position));
                    let mut drag = false;
                    if let Some(world) = world {
                        if self.press_on_selection(world) {
                            drag = true;
                        } else if let Some(hit) = self.hit_test(world) {
                            if additive {
                                self.selected_shapes.insert(hit.clone());
                            } else {
                                self.selected_shapes.clear();
                                self.selected_shapes.insert(hit.clone());
                            }
                            self.selected_layer = Some(hit.layer);
                            drag = true;
                        }
                    }
                    if drag {
                        self.push_selection_undo();
                        self.dragging_shape = true;
                    } else {
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
        }

        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.view.pan(response.drag_delta());
        } else if response.dragged_by(egui::PointerButton::Primary) {
            if self.node_drag.is_some() {
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

        if let Some(selected) = self.selected_node {
            if let Some(node) = path_nodes.iter().find(|node| node.node == selected) {
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
                action = Some(Action::Undo);
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

        match action {
            Some(Action::Open) => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("SVG", &["svg"])
                    .add_filter("Orleans project", &["orleans"])
                    .set_title("Open design or project")
                    .pick_file()
                {
                    self.open_path(path);
                }
            }
            Some(Action::Export) => self.export(),
            Some(Action::SaveProject) => self.save_project(),
            Some(Action::Undo) => self.undo_shape_move(),
            Some(Action::ExitNodes) => self.exit_node_edit(),
            Some(Action::DeleteNode) => self.delete_selected_node(),
            Some(Action::Reload) => {
                if let Some(path) = self.project_path.clone() {
                    self.load_project(path);
                } else if let Some(path) = self.path.clone() {
                    self.load_svg(path);
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
}
