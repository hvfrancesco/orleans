//! `orleans` — a small CAM tool for laser cutters.
//!
//! Three commands are enough to be useful end to end:
//!
//! ```text
//! orleans gui   design.svg
//! orleans info  design.svg
//! orleans gcode design.svg -o design.gcode
//! orleans info  drawing.dxf
//! orleans gcode drawing.dxf -o drawing.gcode
//! orleans run   design.svg --url tcp://192.168.1.50   # WiFi / Telnet
//! orleans run   design.svg --port /dev/ttyUSB0        # USB serial
//! orleans run   design.svg --mock                     # simulator
//! orleans stop  --url tcp://192.168.1.50              # stop + laser off
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use kurbo::Point;
use orleans_core::Document;
use orleans_driver::gcode::{self, GcodeDialect, GcodeOptions, LaserMode};
use orleans_driver::net::{parse_endpoint, TcpTransport};
use orleans_driver::{
    Controller, GrblClient, GrblConfig, MarlinClient, MarlinConfig, Protocol, SimulatedGrbl,
    SimulatedMarlin, Transport,
};
use orleans_import::{import_dxf, import_pdf, import_svg, ImportOptions};
use orleans_toolpath::{plan, PlanOptions};

#[derive(Parser)]
#[command(
    name = "orleans",
    version,
    about = "Open CAM for laser cutters: SVG in, G-code out, GRBL or Marlin over USB or WiFi"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
enum DriverChoice {
    /// Detect GRBL or Marlin from the controller's status/identity response.
    #[default]
    Auto,
    /// Force the GRBL / GrblHAL / FluidNC driver.
    Grbl,
    /// Force the Marlin laser driver.
    Marlin,
}

impl DriverChoice {
    fn export_protocol(self) -> Protocol {
        match self {
            DriverChoice::Marlin => Protocol::Marlin,
            DriverChoice::Auto | DriverChoice::Grbl => Protocol::Grbl,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Open the visual editor.
    #[cfg(feature = "gui")]
    Gui {
        /// SVG, raster image or .orleans project to open at startup.
        input: Option<PathBuf>,
        /// Machine endpoint to prefill, e.g. tcp://192.168.1.10 or ws://host:81/ws.
        #[arg(long)]
        url: Option<String>,
        /// Serial port to prefill, e.g. /dev/ttyUSB0 or COM3.
        #[arg(long)]
        port: Option<String>,
        /// Baud rate for a serial startup connection.
        #[arg(long, default_value_t = 115200)]
        baud: u32,
        /// Protocol preference for the startup connection.
        #[arg(long, value_enum, default_value_t)]
        driver: DriverChoice,
    },
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
        /// G-code dialect. Auto means GRBL for an offline export.
        #[arg(long, value_enum, default_value_t)]
        driver: DriverChoice,
    },
    /// Stream a design to a controller.
    Run {
        #[command(flatten)]
        common: Common,
        /// Serial port, e.g. /dev/ttyUSB0 or COM3.
        #[arg(long)]
        port: Option<String>,
        /// Network endpoint, e.g. tcp://192.168.1.50 (Telnet) or ws://host:81/ws.
        #[arg(long)]
        url: Option<String>,
        /// Baud rate for serial connections.
        #[arg(long, default_value_t = 115200)]
        baud: u32,
        /// Controller firmware. Auto probes first for GRBL, then Marlin.
        #[arg(long, value_enum, default_value_t)]
        driver: DriverChoice,
        /// Run against the built-in simulator instead of hardware.
        #[arg(long)]
        mock: bool,
        /// Stream even if the controller does not answer a status request.
        #[arg(long)]
        force: bool,
        /// Abort if the controller is silent for this many seconds.
        ///
        /// This is a no-response timeout, not a limit on the job duration.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },
    /// Stop the machine now and turn the laser off.
    Stop {
        /// Serial port, e.g. /dev/ttyUSB0 or COM3.
        #[arg(long)]
        port: Option<String>,
        /// Network endpoint, e.g. tcp://192.168.1.50 (Telnet) or ws://host:81/ws.
        #[arg(long)]
        url: Option<String>,
        #[arg(long, default_value_t = 115200)]
        baud: u32,
        /// Controller firmware. Auto probes first for GRBL, then Marlin.
        #[arg(long, value_enum, default_value_t)]
        driver: DriverChoice,
    },
    /// Clear a GRBL alarm (`$X`) or restart a halted Marlin (`M999`).
    Unlock {
        /// Serial port, e.g. /dev/ttyUSB0 or COM3.
        #[arg(long)]
        port: Option<String>,
        /// Network endpoint, e.g. tcp://192.168.1.50 (Telnet) or ws://host:81/ws.
        #[arg(long)]
        url: Option<String>,
        #[arg(long, default_value_t = 115200)]
        baud: u32,
        /// Controller firmware. Auto probes first for GRBL, then Marlin.
        #[arg(long, value_enum, default_value_t)]
        driver: DriverChoice,
    },
    /// Ask a connected controller for its status.
    Status {
        /// Serial port, e.g. /dev/ttyUSB0 or COM3.
        #[arg(long)]
        port: Option<String>,
        /// Network endpoint, e.g. tcp://192.168.1.50 (Telnet) or ws://host:81/ws.
        #[arg(long)]
        url: Option<String>,
        #[arg(long, default_value_t = 115200)]
        baud: u32,
        /// Controller firmware. Auto probes first for GRBL, then Marlin.
        #[arg(long, value_enum, default_value_t)]
        driver: DriverChoice,
    },
}

#[derive(Args, Clone)]
struct Common {
    /// Input SVG, DXF or PDF/AI file.
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
    /// `S` value for 100 % power (default: 1000 GRBL, 255 Marlin).
    #[arg(long)]
    max_s: Option<u32>,
    /// Use M4 (dynamic power) instead of M3.
    #[arg(long)]
    dynamic_power: bool,
    /// Flattening tolerance in mm.
    #[arg(long, default_value_t = orleans_geom::DEFAULT_TOLERANCE)]
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
    /// PDF page number, starting at 1.
    #[arg(long, default_value_t = 1)]
    page: usize,
}

impl Common {
    fn import(&self) -> Result<Document> {
        let bytes = std::fs::read(&self.input)
            .with_context(|| format!("reading {}", self.input.display()))?;
        let opt = ImportOptions {
            dpi: orleans_import::DEFAULT_DPI,
            flip_y: !self.no_flip,
            scale: self.scale,
            default_cut: orleans_core::CutParams::new(self.speed, self.power, self.passes),
            group_by_color: true,
            pdf_page: self.page.saturating_sub(1),
        };
        let is_dxf = self
            .input
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dxf"));
        let is_pdf = self
            .input
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("pdf") || extension.eq_ignore_ascii_case("ai")
            });
        let doc = if is_dxf {
            import_dxf(&bytes, &opt)
        } else if is_pdf {
            import_pdf(&bytes, &opt)
        } else {
            import_svg(&bytes, &opt)
        }
        .with_context(|| format!("importing {}", self.input.display()))?;
        if doc.is_empty() {
            bail!("{} contains no paths", self.input.display());
        }
        Ok(doc)
    }

    fn plan(&self, doc: &Document) -> orleans_core::Job {
        let opt = PlanOptions {
            tolerance: self.tolerance,
            kerf: self.kerf,
            rapid_speed: 3000.0,
            optimize: !self.no_optimize,
            start: Point::ORIGIN,
        };
        plan(doc, &opt)
    }

    fn gcode_options(&self, protocol: Protocol) -> GcodeOptions {
        GcodeOptions {
            laser_mode: if self.dynamic_power {
                LaserMode::M4
            } else {
                LaserMode::M3
            },
            max_s: self.max_s.unwrap_or_else(|| protocol.default_max_s()),
            dialect: match protocol {
                Protocol::Grbl => GcodeDialect::Grbl,
                Protocol::Marlin => GcodeDialect::Marlin,
            },
            ..Default::default()
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        #[cfg(feature = "gui")]
        Command::Gui {
            input,
            url,
            port,
            baud,
            driver,
        } => orleans_ui::run(input, gui_connection(url, port, baud, driver)?),
        Command::Info(cmd) => info(&cmd),
        Command::Gcode {
            common,
            output,
            stdout,
            driver,
        } => gcode_cmd(&common, output, stdout, driver),
        Command::Run {
            common,
            port,
            url,
            baud,
            driver,
            mock,
            force,
            timeout,
        } => run(&common, port, url, baud, driver, mock, force, timeout),
        Command::Stop {
            port,
            url,
            baud,
            driver,
        } => stop_machine(port, url, baud, driver),
        Command::Unlock {
            port,
            url,
            baud,
            driver,
        } => unlock(port, url, baud, driver),
        Command::Status {
            port,
            url,
            baud,
            driver,
        } => status(port, url, baud, driver),
    }
}

#[cfg(feature = "gui")]
fn gui_connection(
    url: Option<String>,
    port: Option<String>,
    baud: u32,
    driver: DriverChoice,
) -> Result<Option<orleans_ui::Connection>> {
    use orleans_ui::{Connection, DriverChoice as UiDriverChoice};
    let driver = match driver {
        DriverChoice::Auto => UiDriverChoice::Auto,
        DriverChoice::Grbl => UiDriverChoice::Grbl,
        DriverChoice::Marlin => UiDriverChoice::Marlin,
    };
    match (url, port) {
        (Some(_), Some(_)) => bail!("use either --url or --port, not both"),
        (Some(url), None) if url.starts_with("ws://") || url.starts_with("wss://") => {
            Ok(Some(Connection::websocket(url, driver)))
        }
        (Some(url), None) => Ok(Some(Connection::tcp(url, driver))),
        (None, Some(port)) => Ok(Some(Connection::serial(port, baud, driver))),
        (None, None) => Ok(None),
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

fn gcode_cmd(
    cmd: &Common,
    output: Option<PathBuf>,
    stdout: bool,
    driver: DriverChoice,
) -> Result<()> {
    let doc = cmd.import()?;
    let job = cmd.plan(&doc);
    let program = gcode::write(&job, &cmd.gcode_options(driver.export_protocol()));

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

#[allow(clippy::too_many_arguments)]
fn run(
    cmd: &Common,
    port: Option<String>,
    url: Option<String>,
    baud: u32,
    driver: DriverChoice,
    mock: bool,
    force: bool,
    timeout: u64,
) -> Result<()> {
    let doc = cmd.import()?;
    let job = cmd.plan(&doc);

    eprintln!(
        "job: {} cuts, {:.1} mm, est {}",
        job.cuts().count(),
        job.stats.cut_mm,
        format_duration(job.stats.seconds)
    );

    let protocol = if mock {
        driver.export_protocol()
    } else {
        Protocol::Grbl
    };

    if mock {
        let program = gcode::write(&job, &cmd.gcode_options(protocol));
        return run_mock(protocol, &program);
    }

    let (transport, kind) = open_transport(url.as_deref(), port.as_deref(), baud)?;
    let (mut controller, protocol, status) = connect_controller(transport, kind, driver, 2)?;
    controller.set_timeout(Duration::from_secs(timeout.max(1)));
    let program = gcode::write(&job, &cmd.gcode_options(protocol));

    if !force {
        match status {
            Some(status) => eprintln!("controller: {} ({status})", protocol.label()),
            None => bail!(
                "no {} status from the controller, so the job was not sent.\n\
                 Run `orleans status --driver {driver:?}` against the same endpoint \
                 for a diagnosis, or pass --force to stream anyway.",
                protocol.label()
            ),
        }
    }

    let mut last_report = Instant::now();
    let stop = std::sync::atomic::AtomicBool::new(false);
    let pause = std::sync::atomic::AtomicBool::new(false);
    let acks = controller
        .run_pausable(
            &program,
            |done, total| {
                if done == total || last_report.elapsed() >= Duration::from_secs(2) {
                    eprintln!("  {done}/{total} lines acknowledged");
                    last_report = Instant::now();
                }
            },
            &stop,
            &pause,
        )
        .map_err(|err| {
            let recovery = match protocol {
                Protocol::Grbl => "orleans attempted feed hold and a soft reset (laser off).",
                Protocol::Marlin => {
                    "orleans attempted an M112 emergency stop (laser off). \
                     Press Unlock (M999) before continuing."
                }
            };
            anyhow::anyhow!(
                "streaming G-code failed: {err}\n\
                 {recovery} If the link was already gone, stop the machine at the \
                 panel. Check the position and re-home or re-zero before retrying. \
                 A slow controller can be given a longer --timeout."
            )
        })?;
    eprintln!("done: {acks} lines acknowledged");
    Ok(())
}

fn run_mock(protocol: Protocol, program: &str) -> Result<()> {
    match protocol {
        Protocol::Grbl => {
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
                let acks = client.run(program)?;
                client.request_status()?;
                if let Some(status) = client.read_status()? {
                    eprintln!("status after:  {status}");
                }
                eprintln!(
                    "simulated GRBL: {acks} lines acknowledged, {} received",
                    sim.received.len()
                );
            }
            eprintln!("final position: X{:.1} Y{:.1}", sim.pos[0], sim.pos[1]);
        }
        Protocol::Marlin => {
            let mut sim = SimulatedMarlin::new();
            {
                let config = MarlinConfig {
                    window: 120,
                    timeout: Duration::from_secs(5),
                    drain_timeout: Duration::from_secs(30),
                };
                let mut client = MarlinClient::with_config(&mut sim, config);
                if let Some(status) = client.status_line()? {
                    eprintln!("status before: {status}");
                }
                let acks = client.run(program)?;
                if let Some(status) = client.status_line()? {
                    eprintln!("status after:  {status}");
                }
                eprintln!(
                    "simulated Marlin: {acks} lines acknowledged, {} received",
                    sim.received().len()
                );
            }
            let [x, y, _] = sim.position();
            eprintln!("final position: X{x:.1} Y{y:.1}");
        }
    }
    Ok(())
}

fn stop_machine(
    port: Option<String>,
    url: Option<String>,
    baud: u32,
    driver: DriverChoice,
) -> Result<()> {
    let (transport, kind) = open_transport(url.as_deref(), port.as_deref(), baud)?;
    let (mut controller, protocol, _) = connect_controller(transport, kind, driver, 2)?;
    controller.stop().context("stopping the controller")?;
    match protocol {
        Protocol::Grbl => println!("controller stopped and reset; laser off"),
        Protocol::Marlin => {
            println!("emergency stop sent (M112); laser off. Send `orleans unlock` to restart")
        }
    }
    Ok(())
}

fn unlock(
    port: Option<String>,
    url: Option<String>,
    baud: u32,
    driver: DriverChoice,
) -> Result<()> {
    let (transport, kind) = open_transport(url.as_deref(), port.as_deref(), baud)?;
    let (mut controller, protocol, _) = connect_controller(transport, kind, driver, 2)?;
    controller
        .unlock()
        .context("clearing the controller alarm")?;
    match protocol {
        Protocol::Grbl => println!("alarm cleared ($X)"),
        Protocol::Marlin => println!("controller restarted (M999)"),
    }
    Ok(())
}

fn status(
    port: Option<String>,
    url: Option<String>,
    baud: u32,
    driver: DriverChoice,
) -> Result<()> {
    let (transport, kind) = open_transport(url.as_deref(), port.as_deref(), baud)?;
    let (mut controller, protocol, status) = connect_controller(transport, kind, driver, 2)?;
    println!("protocol: {}", protocol.label());

    match protocol {
        Protocol::Grbl => match status {
            Some(status) => println!("{status}"),
            None => bail!("connected, but no GRBL status report arrived"),
        },
        Protocol::Marlin => {
            let poll = controller.poll_status(Duration::from_secs(2))?;
            for line in &poll.lines {
                if !line.trim().eq_ignore_ascii_case("ok") {
                    println!("  {line}");
                }
            }
            match poll.status {
                Some(status) => println!("{status}"),
                None => bail!("connected to Marlin, but M114 did not report a position"),
            }
        }
    }
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

type ConnectedController = (Controller<Box<dyn Transport>>, Protocol, Option<String>);

fn connect_controller(
    transport: Box<dyn Transport>,
    kind: LinkKind,
    driver: DriverChoice,
    probe_secs: u64,
) -> Result<ConnectedController> {
    let probe = Duration::from_secs(probe_secs.max(1));
    match driver {
        DriverChoice::Auto => {
            let detection =
                Controller::detect(transport, probe, probe).map_err(anyhow::Error::msg)?;
            let protocol = detection.controller.protocol();
            if let Some(firmware) = &detection.firmware {
                eprintln!("identified: {firmware}");
            }
            Ok((detection.controller, protocol, detection.status))
        }
        DriverChoice::Grbl => {
            let mut client = GrblClient::new(transport);
            if kind == LinkKind::Serial {
                client.wake()?;
            }
            let probe = client.probe_status(probe)?;
            Ok((Controller::Grbl(client), Protocol::Grbl, probe.status))
        }
        DriverChoice::Marlin => {
            let mut client = MarlinClient::new(transport);
            if kind == LinkKind::Serial {
                client.wake()?;
            }
            let identity = client.probe_identity(probe)?;
            if !identity.is_marlin {
                bail!(
                    "no Marlin identity (M115); received: {}",
                    if identity.lines.is_empty() {
                        "nothing".to_owned()
                    } else {
                        identity.lines.join(" | ")
                    }
                );
            }
            if let Some(firmware) = &identity.firmware {
                eprintln!("identified: {firmware}");
            }
            let status = client.status_line()?;
            Ok((Controller::Marlin(client), Protocol::Marlin, status))
        }
    }
}

/// Open either a TCP (Telnet) or serial link, boxed so the caller does not
/// need to be generic over the transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkKind {
    Serial,
    Tcp,
    WebSocket,
}

fn open_transport(
    url: Option<&str>,
    port: Option<&str>,
    baud: u32,
) -> Result<(Box<dyn Transport>, LinkKind)> {
    match (url, port) {
        (Some(_), Some(_)) => bail!("use either --url or --port, not both"),
        (Some(url), None) if url.starts_with("http://") || url.starts_with("https://") => {
            bail!("HTTP command forwarding is not supported; use ws://host:81/ws or tcp://host:23")
        }
        (Some(url), None) if url.starts_with("ws://") || url.starts_with("wss://") => {
            eprintln!("link: {url}");
            let link = orleans_driver::WebSocketTransport::open(url)
                .with_context(|| format!("opening WebSocket {url}"))?;
            Ok((Box::new(link), LinkKind::WebSocket))
        }
        (Some(url), None) => {
            let (host, port) = parse_endpoint(url).map_err(anyhow::Error::msg)?;
            eprintln!("link: tcp://{host}:{port}");
            let link = TcpTransport::connect(&host, port, Duration::from_secs(5))
                .with_context(|| format!("connecting to {host}:{port}"))?;
            Ok((Box::new(link), LinkKind::Tcp))
        }
        (None, Some(port)) => {
            eprintln!("link: serial {port} at {baud} baud");
            let link = orleans_driver::open_serial(port, baud)
                .with_context(|| format!("opening {port} at {baud} baud"))?;
            Ok((Box::new(link), LinkKind::Serial))
        }
        (None, None) => {
            bail!("no connection given: pass --url tcp://HOST, --port /dev/ttyUSB0, or --mock")
        }
    }
}
