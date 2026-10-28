//! The main application window: toolbar, layer panel, canvas, status bar.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, StrokeKind};
use kurbo::Point;
use orleans_core::{Document, Job, Op};
use orleans_driver::gcode::{self, GcodeOptions, LaserMode};
use orleans_import::{import_svg, ImportOptions};
use orleans_toolpath::{plan, PlanOptions};

use crate::machine::{Event as MachineEvent, Machine, Status as MachineStatus};
use crate::preview::Preview;
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
}

pub struct OrleansApp {
    doc: Document,
    preview: Preview,
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

    endpoint: String,
    machine: Machine,
    link: LinkState,
    machine_status: Option<MachineStatus>,
    /// Set right after asking for homing, to debounce the button.
    home_pending_until: Option<Instant>,
    confirm_home: bool,
    reconnect_attempt: u32,
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
            job: Job::default(),
            path: None,
            view: Viewport::default(),
            plan_opts: PlanOptions::default(),
            gcode_opts: GcodeOptions::default(),
            status: Status::ok("Open an SVG, or drop one onto the window."),
            dirty: false,
            fit_pending: true,
            show_grid: true,
            show_travel: false,
            endpoint: endpoint.unwrap_or_default(),
            machine: Machine::spawn(),
            link: LinkState::Disconnected,
            machine_status: None,
            home_pending_until: None,
            confirm_home: false,
            reconnect_attempt: 0,
        };
        if let Some(path) = initial {
            app.load(path);
        }
        app
    }

    fn load(&mut self, path: PathBuf) {
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
                self.fit_pending = true;
                self.dirty = false;
            }
            Err(err) => {
                self.status = Status::error(format!("importing {}: {err}", path.display()));
            }
        }
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
        while let Some(event) = self.machine.try_recv() {
            match event {
                MachineEvent::Connecting(endpoint) => {
                    self.link = LinkState::Connecting;
                    self.reconnect_attempt = 0;
                    self.machine_status = None;
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
                    self.home_pending_until = None;
                    match reason {
                        Some(reason) => self.set_status(reason, true),
                        None => self.set_status("disconnected", false),
                    }
                }
                MachineEvent::Status(line) => {
                    if self.link == LinkState::Connected {
                        if let Some(status) = MachineStatus::parse(&line) {
                            self.machine_status = Some(status);
                        }
                    }
                }
                MachineEvent::Message { text, error } => self.set_status(text, error),
            }
        }
        if self.link != LinkState::Disconnected {
            ctx.request_repaint_after(Duration::from_millis(250));
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

    fn disconnect_machine(&mut self) {
        let _ = self.machine.disconnect();
        self.link = LinkState::Disconnected;
        self.reconnect_attempt = 0;
        self.machine_status = None;
        self.set_status("disconnected", false);
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
                        if ui.button("Disconnect").clicked() {
                            self.disconnect_machine();
                        }
                    }
                });

                ui.separator();
                ui.heading("Status");
                match &self.machine_status {
                    Some(status) => {
                        ui.horizontal(|ui| {
                            ui.label("State");
                            ui.colored_label(
                                state_color(&status.state),
                                RichText::new(&status.state).strong(),
                            );
                        });
                        if let Some([x, y, z]) = status.mpos {
                            ui.monospace(format!("X {x:.2}   Y {y:.2}   Z {z:.2}"));
                        }
                        if status.feed.is_some() || status.spindle.is_some() {
                            ui.label(format!(
                                "F {} mm/min   S {}",
                                status.feed.unwrap_or(0),
                                status.spindle.unwrap_or(0)
                            ));
                        }
                        ui.add_space(2.0);
                        ui.label(RichText::new(&status.raw).small().weak().monospace());
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

                ui.separator();
                ui.heading("Control");
                let connected = self.link == LinkState::Connected;
                let pending = self
                    .home_pending_until
                    .map(|until| until > Instant::now())
                    .unwrap_or(false);
                let can_home = connected
                    && !pending
                    && self
                        .machine_status
                        .as_ref()
                        .map(|status| status.can_home())
                        .unwrap_or(false);

                if ui
                    .add_enabled(can_home, egui::Button::new("Home ($H)"))
                    .on_hover_text(
                        "Move the head to the limit switches. \
                         Enabled when the controller is idle or in alarm.",
                    )
                    .clicked()
                {
                    self.confirm_home = true;
                }
                if ui
                    .add_enabled(connected, egui::Button::new("Unlock ($X)"))
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
                    if !self.machine.stop() {
                        self.set_status("machine thread stopped", true);
                    }
                    self.home_pending_until = None;
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Homing moves the head. Clear the bed first.")
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
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
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
                if ui.button("Open SVG…").clicked() {
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
                    .add_enabled(has_doc, egui::Button::new("Reload"))
                    .clicked()
                {
                    *action = Some(Action::Reload);
                }
                if ui.button("Fit").clicked() {
                    *action = Some(Action::Fit);
                }
                ui.separator();
                ui.checkbox(&mut self.show_grid, "Grid");
                ui.checkbox(&mut self.show_travel, "Travel");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let name = self
                        .path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "no file".to_owned());
                    ui.label(RichText::new(name).weak());
                });
            });
            ui.add_space(4.0);
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("layers")
            .resizable(true)
            .default_size(340.0)
            .min_size(280.0)
            .max_size(560.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.heading("Layers");
                ui.add_space(2.0);

                let mut changed = false;
                let list_height = (ui.available_height() - 210.0).max(120.0);
                egui::ScrollArea::vertical()
                    .max_height(list_height)
                    .show(ui, |ui| {
                        if self.doc.layers.is_empty() {
                            ui.label(
                                RichText::new("No layers yet. Open an SVG or drop one here.")
                                    .weak(),
                            );
                        }
                        for layer in &mut self.doc.layers {
                            let color = display_color(layer.color);
                            ui.horizontal(|ui| {
                                ui.colored_label(color, "●");
                                ui.strong(&layer.name);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let shapes = layer.shapes.len();
                                        ui.label(
                                            RichText::new(format!(
                                                "{shapes} shape{}",
                                                if shapes == 1 { "" } else { "s" }
                                            ))
                                            .weak(),
                                        );
                                    },
                                );
                            });
                            egui::Grid::new(format!("params-{}", layer.index))
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
                            ui.add_space(4.0);
                            ui.separator();
                        }
                    });

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
                if ui.checkbox(&mut m4, "Dynamic power (M4)").changed() {
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
            let bounds = self.doc.bounds().unwrap_or_else(|| {
                kurbo::Rect::new(0.0, 0.0, self.doc.width.max(1.0), self.doc.height.max(1.0))
            });
            self.view.fit(bounds, area, 48.0);
            self.fit_pending = false;
        }

        if self.show_grid {
            self.draw_grid(&painter, area);
        }
        self.draw_page(&painter, area);
        if self.show_travel {
            self.draw_travel(&painter, area);
        }
        self.draw_layers(&painter, area);
        self.draw_origin(&painter, area);

        // Pan with any mouse button; zoom with the wheel around the cursor.
        if response.dragged() {
            self.view.pan(response.drag_delta());
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
            self.fit_pending = true;
        }
    }

    fn draw_grid(&self, painter: &egui::Painter, area: Rect) {
        let visible = self.view.visible_world(area);
        let step = nice_step(80.0 / f64::from(self.view.zoom));
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
        for (layer, preview) in self.doc.layers.iter().zip(&self.preview.layers) {
            let color = display_color(layer.color);
            let stroke = Stroke::new(1.4, color);
            for contour in &preview.contours {
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

    fn draw_origin(&self, painter: &egui::Painter, area: Rect) {
        if self.doc.is_empty() {
            return;
        }
        let origin = self.view.world_to_screen(area, Point::ORIGIN);
        painter.circle_filled(origin, 3.0, Color32::from_rgb(230, 190, 90));
    }
}

/// Layer colours come from the SVG, and black is common. On a dark canvas the
/// original colour would be invisible, so lift dark colours toward white while
/// keeping their hue. Bright or saturated colours are left untouched.
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

        let mut action: Option<Action> = None;
        ctx.input(|i| {
            if i.modifiers.command && i.key_pressed(egui::Key::O) {
                action = Some(Action::Open);
            }
            if i.modifiers.command && i.key_pressed(egui::Key::E) {
                action = Some(Action::Export);
            }
            if !i.modifiers.command && i.key_pressed(egui::Key::F) {
                action = Some(Action::Fit);
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
                    .set_title("Open SVG")
                    .pick_file()
                {
                    self.load(path);
                }
            }
            Some(Action::Export) => self.export(),
            Some(Action::Reload) => {
                if let Some(path) = self.path.clone() {
                    self.load(path);
                } else {
                    self.status = Status::error("nothing to reload");
                }
            }
            Some(Action::Fit) => self.fit_pending = true,
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
            self.load(path);
        }

        self.home_dialog(&ctx);
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
    fn durations_are_human_readable() {
        assert_eq!(format_duration(0.0), "0s");
        assert_eq!(format_duration(42.4), "42s");
        assert_eq!(format_duration(61.0), "1m 01s");
        assert_eq!(format_duration(3725.0), "1h 02m");
    }
}
