//! `orleans` — a small CAM tool for laser cutters.
//!
//! Three commands are enough to be useful end to end:
//!
//! ```text
//! orleans gui   design.svg
//! orleans info  design.svg
//! orleans gcode design.svg -o design.gcode
//! orleans run   design.svg --url tcp://192.168.1.50   # WiFi / Telnet
//! orleans run   design.svg --port /dev/ttyUSB0        # USB serial
//! orleans run   design.svg --mock                     # simulator
//! orleans stop  --url tcp://192.168.1.50              # stop + laser off
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use kurbo::Point;
use orleans_core::Document;
use orleans_driver::gcode::{self, GcodeOptions, LaserMode};
use orleans_driver::net::{TcpTransport, TELNET_PORT};
use orleans_driver::{GrblClient, GrblConfig, SimulatedGrbl, Transport};
use orleans_import::{import_svg, ImportOptions};
use orleans_toolpath::{plan, PlanOptions};

#[derive(Parser)]
#[command(
    name = "orleans",
    version,
    about = "Open CAM for laser cutters: SVG in, G-code out, GRBL over USB or WiFi"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Open the visual editor.
    #[cfg(feature = "gui")]
    Gui {
        /// Design to open at startup.
        input: Option<PathBuf>,
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
    },
    /// Stream a design to a controller.
    Run {
        #[command(flatten)]
        common: Common,
        /// Serial port, e.g. /dev/ttyUSB0 or COM3.
        #[arg(long)]
        port: Option<String>,
        /// Network endpoint, e.g. tcp://192.168.1.50 (Telnet, port 23).
        #[arg(long)]
        url: Option<String>,
        /// Baud rate for serial connections.
        #[arg(long, default_value_t = 115200)]
        baud: u32,
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
    /// Stop the machine now and turn the laser off (feed hold + reset).
    Stop {
        /// Serial port, e.g. /dev/ttyUSB0 or COM3.
        #[arg(long)]
        port: Option<String>,
        /// Network endpoint, e.g. tcp://192.168.1.50 (Telnet, port 23).
        #[arg(long)]
        url: Option<String>,
        #[arg(long, default_value_t = 115200)]
        baud: u32,
    },
    /// Clear a GRBL alarm with `$X`.
    Unlock {
        /// Serial port, e.g. /dev/ttyUSB0 or COM3.
        #[arg(long)]
        port: Option<String>,
        /// Network endpoint, e.g. tcp://192.168.1.50 (Telnet, port 23).
        #[arg(long)]
        url: Option<String>,
        #[arg(long, default_value_t = 115200)]
        baud: u32,
    },
    /// Ask a connected controller for its status.
    Status {
        /// Serial port, e.g. /dev/ttyUSB0 or COM3.
        #[arg(long)]
        port: Option<String>,
        /// Network endpoint, e.g. tcp://192.168.1.50 (Telnet, port 23).
        #[arg(long)]
        url: Option<String>,
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
        };
        let doc = import_svg(&bytes, &opt)
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
        #[cfg(feature = "gui")]
        Command::Gui { input } => orleans_ui::run(input),
        Command::Info(cmd) => info(&cmd),
        Command::Gcode {
            common,
            output,
            stdout,
        } => gcode_cmd(&common, output, stdout),
        Command::Run {
            common,
            port,
            url,
            baud,
            mock,
            force,
            timeout,
        } => run(&common, port, url, baud, mock, force, timeout),
        Command::Stop { port, url, baud } => stop_machine(port, url, baud),
        Command::Unlock { port, url, baud } => unlock(port, url, baud),
        Command::Status { port, url, baud } => status(port, url, baud),
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

fn run(
    cmd: &Common,
    port: Option<String>,
    url: Option<String>,
    baud: u32,
    mock: bool,
    force: bool,
    timeout: u64,
) -> Result<()> {
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

    let (transport, kind) = open_transport(url.as_deref(), port.as_deref(), baud)?;
    let config = GrblConfig {
        window: 120,
        timeout: Duration::from_secs(timeout.max(1)),
    };
    let mut client = GrblClient::with_config(transport, config);
    if kind == LinkKind::Serial {
        // Only USB serial needs the newline wake-up.
        client.wake()?;
    }

    if !force {
        match client.probe_status(Duration::from_secs(2))?.status {
            Some(status) => eprintln!("controller: {status}"),
            None => bail!(
                "no GRBL status from the controller, so the job was not sent.\n\
                 Run `orleans status` against the same endpoint for a diagnosis, \
                 or pass --force to stream anyway."
            ),
        }
    }

    let mut last_report = Instant::now();
    let acks = client
        .run_with_progress(&program, |done, total| {
            if done == total || last_report.elapsed() >= Duration::from_secs(2) {
                eprintln!("  {done}/{total} lines acknowledged");
                last_report = Instant::now();
            }
        })
        .map_err(|err| {
            anyhow::anyhow!(
                "streaming G-code failed: {err}\n\
                 orleans attempted feed hold and a soft reset (laser off). If the \n\
                 link was already gone, stop the machine at the panel. Check the \n\
                 position and re-home or re-zero before retrying. A slow controller \n\
                 can be given a longer --timeout."
            )
        })?;
    eprintln!("done: {acks} lines acknowledged");
    Ok(())
}

fn stop_machine(port: Option<String>, url: Option<String>, baud: u32) -> Result<()> {
    let (transport, _kind) = open_transport(url.as_deref(), port.as_deref(), baud)?;
    let mut client = GrblClient::new(transport);
    client.stop().context("stopping the controller")?;
    println!("controller stopped and reset; laser off");
    Ok(())
}

fn unlock(port: Option<String>, url: Option<String>, baud: u32) -> Result<()> {
    let (transport, kind) = open_transport(url.as_deref(), port.as_deref(), baud)?;
    let mut client = GrblClient::new(transport);
    if kind == LinkKind::Serial {
        client.wake()?;
    }
    client.send("$X").context("clearing the alarm")?;
    println!("alarm cleared ($X)");
    Ok(())
}

fn status(port: Option<String>, url: Option<String>, baud: u32) -> Result<()> {
    let (transport, kind) = open_transport(url.as_deref(), port.as_deref(), baud)?;
    let mut client = GrblClient::new(transport);
    if kind == LinkKind::Serial {
        client.wake()?;
    }
    std::thread::sleep(Duration::from_millis(150));

    let probe = client.probe_status(Duration::from_secs(3))?;
    if let Some(status) = probe.status {
        println!("{status}");
        return Ok(());
    }

    eprintln!("connected, but no GRBL status report within 3 s.");
    if probe.seen.is_empty() {
        eprintln!("  received: nothing");
    } else {
        for line in probe.seen.iter().take(12) {
            eprintln!("  received: {line}");
        }
    }

    eprintln!("probing for firmware identity ($I, M115)…");
    let mut identity = Vec::new();
    for cmd in ["$I", "M115"] {
        client.transport_mut().write_line(cmd)?;
        for line in client.collect_for(Duration::from_millis(900))? {
            eprintln!("  {cmd} -> {line}");
            identity.push(line);
        }
    }

    let joined = identity.join("\n").to_ascii_uppercase();
    if joined.contains("GRBL") || joined.contains("[VER:") {
        bail!(
            "the controller looks like GRBL but does not answer `?`. \
             Check that Telnet/status reporting is enabled in the bridge \
             (some ESP3D builds expose the laser on a different port, e.g. 81/WebSocket)."
        );
    }
    if joined.contains("MARLIN") || joined.contains("FIRMWARE_NAME") {
        bail!(
            "this is Marlin firmware, not GRBL. Orleans only speaks GRBL today; \
             a Marlin driver (M105/M114 status, M3/M5 laser) is still on the roadmap."
        );
    }
    bail!(
        "the TCP endpoint accepted the connection but did not speak GRBL.\n\
         Check that the laser controller itself is powered on (the WiFi bridge \
         can stay up while the laser is off), that port 23 is the GRBL Telnet \
         service and not another device on the network, and that the bridge has \
         Telnet enabled. `nc -vz <ip> 23` confirms reachability."
    );
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

/// Open either a TCP (Telnet) or serial link, boxed so the caller does not
/// need to be generic over the transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkKind {
    Serial,
    Tcp,
}

fn open_transport(
    url: Option<&str>,
    port: Option<&str>,
    baud: u32,
) -> Result<(Box<dyn Transport>, LinkKind)> {
    match (url, port) {
        (Some(_), Some(_)) => bail!("use either --url or --port, not both"),
        (Some(url), None) => {
            let (host, port) = parse_tcp_url(url)?;
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

/// Parse `tcp://host[:port]`, `telnet://host[:port]`, or a bare `host[:port]`.
/// Telnet's port 23 is the default.
fn parse_tcp_url(input: &str) -> Result<(String, u16)> {
    let raw = input.trim();
    let raw = raw
        .strip_prefix("tcp://")
        .or_else(|| raw.strip_prefix("telnet://"))
        .unwrap_or(raw)
        .trim_end_matches('/');
    if raw.is_empty() {
        bail!("empty network address");
    }
    if let Some((host, port)) = raw.rsplit_once(':') {
        let port: u16 = port
            .parse()
            .with_context(|| format!("invalid port in {input:?}"))?;
        if host.is_empty() {
            bail!("missing host in {input:?}");
        }
        return Ok((host.to_owned(), port));
    }
    Ok((raw.to_owned(), TELNET_PORT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tcp_urls() {
        assert_eq!(
            parse_tcp_url("tcp://192.168.1.50:23").unwrap(),
            ("192.168.1.50".to_owned(), 23)
        );
        assert_eq!(
            parse_tcp_url("192.168.1.50").unwrap(),
            ("192.168.1.50".to_owned(), TELNET_PORT)
        );
        assert_eq!(
            parse_tcp_url("telnet://laser.local:2323/").unwrap(),
            ("laser.local".to_owned(), 2323)
        );
        assert!(parse_tcp_url("tcp://host:not-a-port").is_err());
        assert!(parse_tcp_url("tcp://:23").is_err());
    }
}
