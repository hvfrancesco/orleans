//! Desktop editor for orleans.
//!
//! Imports SVG designs, keeps per-layer cutting parameters, edits geometry on
//! the canvas (selection transforms and path nodes), previews the planned job
//! and streams it to a GRBL controller.

mod app;
mod cjk;
mod discovery;
mod history;
mod i18n;
mod icons;
mod machine;
mod nodes;
mod preflight;
mod preview;
mod profiles;
mod project;
mod viewport;

pub use app::OrleansApp;
pub use machine::{Connection, DriverChoice, LinkKind, Status as MachineStatus};
pub use preview::{LayerPreview, Preview, PreviewContour, ShapePreview};
pub use viewport::Viewport;

use std::path::PathBuf;

/// Open the editor, optionally loading `initial` at startup.
pub fn run(initial: Option<PathBuf>, connection: Option<Connection>) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([820.0, 520.0])
            .with_title("orleans"),
        ..Default::default()
    };

    eframe::run_native(
        "orleans",
        options,
        Box::new(move |cc| Ok(Box::new(OrleansApp::new(cc, initial, connection)))),
    )
    .map_err(|err| anyhow::anyhow!("could not start the GUI: {err}"))
}
