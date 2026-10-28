//! Desktop editor for orleans.
//!
//! Milestone 1 is a viewer with layer settings: import an SVG, inspect the
//! geometry, tune speed/power/passes per layer, see live job statistics and
//! export G-code. Streaming from the GUI comes next.

mod app;
mod machine;
mod preview;
mod viewport;

pub use app::OrleansApp;
pub use machine::Status as MachineStatus;
pub use preview::{LayerPreview, Preview, PreviewContour};
pub use viewport::Viewport;

use std::path::PathBuf;

/// Open the editor, optionally loading `initial` at startup.
pub fn run(initial: Option<PathBuf>, endpoint: Option<String>) -> anyhow::Result<()> {
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
        Box::new(move |cc| Ok(Box::new(OrleansApp::new(cc, initial, endpoint)))),
    )
    .map_err(|err| anyhow::anyhow!("could not start the GUI: {err}"))
}
