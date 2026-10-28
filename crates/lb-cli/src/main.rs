//! `lb` — a small CAM tool for laser cutters.
//!
//! Three commands are enough to be useful end to end:
//!
//! ```text
//! lb info  design.svg
//! lb gcode design.svg -o design.gcode
//! lb run   design.svg --port /dev/ttyUSB0      # or --mock
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use kurbo::Point;
use lb_core::Document;
use lb_driver::gcode::{self, GcodeOptions, LaserMode};
use lb_driver::{GrblClient, GrblConfig, SimulatedGrbl};
use lb_import::{import_svg, ImportOptions};
use lb_toolpath::{plan, PlanOptions};

#[derive(Parser)]
#[command(
    name = "lb",
    version,
    about = "Open CAM for laser cutters: SVG in, G-code out, GRBL over serial"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect a design and the job it would produce.
    Info(Common),
    /// Convert a design to G-code.
    Gcode {
        #[command(flatten)]
        common: Common,
        /// Output file (defaults to the input file with a .gcode extension).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Print the program to stdout instead of writing a file.
        #[arg(long)]
        stdout: bool,
    },
    /// Stream a design to a controller.
    Run {
        #[command(flatten)]
        common: Common,
        /// Serial port, e.g. /dev/ttyUSB0 or COM3.
        #[arg(long)]
        port: Option<String>,
        /// Baud rate.
        #[arg(long, default_value_t = 115200)]
        baud: u32,
        /// Run against the built-in simulator instead of hardware.
        #[arg(long)]
        mock: bool,
    },
    /// Ask a connected controller for its status.
    Status {
        #[arg(long)]
        port: String,
        #[arg(long, default_value_t = 115200)]
        baud: u32,
    },
}

#[derive(Args, Clone)]
struct Common {
    /// Input SVG.
    input: PathBuf,
    /// Feed rate in mm/min.
    #[arg(long, default_value_t = 1000.0)]
    speed: f64,
    /// Laser power, percent of maximum.
    #[arg(long, default_value_t = 20.0)]
    power: f64,
    /// Number of passes per contour.
    #[arg(long, default_value_t = 1)]
    passes: u32,
    /// Kerf width in mm (compensation is half of it per side).
    #[arg(long, default_value_t = 0.0)]
    kerf: f64,
    /// `S` value for 100 % power.
    #[arg(long, default_value_t = 1000)]
    max_s: u32,
    /// Use M4 (dynamic power) instead of M3.
    #[arg(long)]
    dynamic_power: bool,
    /// Flattening tolerance in mm.
    #[arg(long, default_value_t = lb_geom::DEFAULT_TOLERANCE)]
    tolerance: f64,
    /// Keep the SVG's Y-down orientation instead of flipping it.
    #[arg(long)]
    no_flip: bool,
    /// Uniform scale applied to the imported design.
    #[arg(long, default_value_t = 1.0)]
    scale: f64,
    /// Cut travel order optimisation off.
    #[arg(long)]
    no_optimize: bool,
}

impl Common {
    fn import(&self) -> Result<Document> {
        let bytes = std::fs::read(&self.input)
            .with_context(|| format!("reading {}", self.input.display()))?;
        let opt = ImportOptions {
            dpi: lb_import::DEFAULT_DPI,
            flip_y: !self.no_flip,
            scale: self.scale,
            default_cut: lb_core::CutParams::new(self.speed, self.power, self.passes),
            group_by_color: true,
        };
        let doc = import_svg(&bytes, &opt)
            .with_context(|| format!("importing {}", self.input.display()))?;
        if doc.is_empty() {
            bail!("{} contains no paths", self.input.display());
        }
        Ok(doc)
    }

    fn plan(&self, doc: &Document) -> lb_core::Job {
        let opt = PlanOptions {
            tolerance: self.tolerance,
            kerf: self.kerf,
            rapid_speed: 3000.0,
            optimize: !self.no_optimize,
            start: Point::ORIGIN,
        };
        plan(doc, &opt)
    }

    fn gcode_options(&self) -> GcodeOptions {
        GcodeOptions {
            laser_mode: if self.dynamic_power {
                LaserMode::M4
            } else {
                LaserMode::M3
            },
            max_s: self.max_s,
            ..Default::default()
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Info(cmd) => info(&cmd),
        Command::Gcode {
            common,
            output,
            stdout,
        } => gcode_cmd(&common, output, stdout),
        Command::Run {
            common,
            port,
            baud,
            mock,
        } => run(&common, port, baud, mock),
        Command::Status { port, baud } => status(&port, baud),
    }
}

fn info(cmd: &Common) -> Result<()> {
    let doc = cmd.import()?;
    let job = cmd.plan(&doc);

    println!("{}", cmd.input.display());
    println!("  size:   {:.1} x {:.1} mm", doc.width, doc.height);
    if let Some(b) = doc.bounds() {
        println!(
            "  bounds: {:.1},{:.1} -> {:.1},{:.1} mm",
            b.x0, b.y0, b.x1, b.y1
        );
    }
    println!(
        "  layers: {}  shapes: {}",
        doc.layers.len(),
        doc.shape_count()
    );
    for layer in &doc.layers {
        println!(
            "    {} #{:02x}{:02x}{:02x}  {:?}  {:>6.0} mm/min  {:>3.0}%  x{}  ({} shapes)",
            layer.name,
            layer.color[0],
            layer.color[1],
            layer.color[2],
            layer.mode,
            layer.cut.speed,
            layer.cut.power,
            layer.cut.passes,
            layer.shapes.len()
        );
    }
    println!(
        "  job:    {} cuts  cut {:.1} mm  travel {:.1} mm  est {}",
        job.cuts().count(),
        job.stats.cut_mm,
        job.stats.travel_mm,
        format_duration(job.stats.seconds)
    );
    Ok(())
}

fn gcode_cmd(cmd: &Common, output: Option<PathBuf>, stdout: bool) -> Result<()> {
    let doc = cmd.import()?;
    let job = cmd.plan(&doc);
    let program = gcode::write(&job, &cmd.gcode_options());

    if stdout {
        print!("{program}");
        return Ok(());
    }

    let path = output.unwrap_or_else(|| cmd.input.with_extension("gcode"));
    std::fs::write(&path, &program).with_context(|| format!("writing {}", path.display()))?;
    eprintln!(
        "{}: {} cuts, {:.1} mm of cutting, est {}",
        path.display(),
        job.cuts().count(),
        job.stats.cut_mm,
        format_duration(job.stats.seconds)
    );
    Ok(())
}

fn run(cmd: &Common, port: Option<String>, baud: u32, mock: bool) -> Result<()> {
    let doc = cmd.import()?;
    let job = cmd.plan(&doc);
    let program = gcode::write(&job, &cmd.gcode_options());

    eprintln!(
        "job: {} cuts, {:.1} mm, est {}",
        job.cuts().count(),
        job.stats.cut_mm,
        format_duration(job.stats.seconds)
    );

    if mock {
        let mut sim = SimulatedGrbl::new();
        {
            let config = GrblConfig {
                window: 120,
                timeout: Duration::from_secs(5),
            };
            let mut client = GrblClient::with_config(&mut sim, config);
            client.wake()?;
            client.request_status()?;
            if let Some(status) = client.read_status()? {
                eprintln!("status before: {status}");
            }
            let acks = client.run(&program)?;
            client.request_status()?;
            if let Some(status) = client.read_status()? {
                eprintln!("status after:  {status}");
            }
            eprintln!(
                "simulated: {acks} lines acknowledged, {} received",
                sim.received.len()
            );
        }
        eprintln!("final position: X{:.1} Y{:.1}", sim.pos[0], sim.pos[1]);
        return Ok(());
    }

    let port = port.ok_or_else(|| {
        anyhow::anyhow!("no port given: pass --port /dev/ttyUSB0, or --mock to simulate")
    })?;
    let link = lb_driver::open_serial(&port, baud)
        .with_context(|| format!("opening {port} at {baud} baud"))?;
    let mut client = GrblClient::new(link);

    client.wake()?;
    client.request_status()?;
    if let Some(status) = client.read_status()? {
        eprintln!("controller: {status}");
    }

    let acks = client.run(&program).context("streaming G-code")?;
    eprintln!("done: {acks} lines acknowledged");
    Ok(())
}

fn status(port: &str, baud: u32) -> Result<()> {
    let link = lb_driver::open_serial(port, baud)
        .with_context(|| format!("opening {port} at {baud} baud"))?;
    let mut client = GrblClient::new(link);
    client.wake()?;
    std::thread::sleep(Duration::from_millis(100));
    // Drain the banner before asking for a status report.
    let _ = client.transport_mut();
    client.request_status()?;
    let status = client
        .read_status()?
        .context("no status report from controller")?;
    println!("{status}");
    Ok(())
}

fn format_duration(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}h {m:02}m {s:02}s")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

#[allow(dead_code)]
fn _unused(_: &Path) {}
