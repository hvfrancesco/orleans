//! Background link to the laser controller.
//!
//! The GUI thread must never block on a socket, and GRBL needs continuous
//! status polling. A worker thread owns the connection and talks to the UI
//! through two channels: commands in, events out.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use orleans_driver::net::{parse_endpoint, TcpTransport};
use orleans_driver::{Controller, DriverError, GrblClient, MarlinClient, Protocol, Transport};

/// How the GUI should choose the controller protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriverChoice {
    /// Probe GRBL first, then Marlin.
    #[default]
    Auto,
    /// Force the GRBL / GrblHAL / FluidNC client.
    Grbl,
    /// Force the Marlin client.
    Marlin,
}

/// Physical link used to reach the controller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkKind {
    #[default]
    Tcp,
    Serial,
}

fn default_baud() -> u32 {
    115_200
}

/// A reusable machine connection, including the protocol preference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub kind: LinkKind,
    pub endpoint: String,
    #[serde(default = "default_baud")]
    pub baud: u32,
    pub driver: DriverChoice,
}

impl Default for Connection {
    fn default() -> Self {
        Self {
            kind: LinkKind::Tcp,
            endpoint: String::new(),
            baud: default_baud(),
            driver: DriverChoice::Auto,
        }
    }
}

impl Connection {
    pub fn tcp(endpoint: impl Into<String>, driver: DriverChoice) -> Self {
        Self {
            kind: LinkKind::Tcp,
            endpoint: endpoint.into(),
            baud: default_baud(),
            driver,
        }
    }

    pub fn serial(port: impl Into<String>, baud: u32, driver: DriverChoice) -> Self {
        Self {
            kind: LinkKind::Serial,
            endpoint: port.into(),
            baud,
            driver,
        }
    }

    pub fn label(&self) -> String {
        match self.kind {
            LinkKind::Tcp => self.endpoint.clone(),
            LinkKind::Serial => format!("{} @ {} baud", self.endpoint, self.baud),
        }
    }
}

/// Requests from the UI to the worker.
pub enum Command {
    Connect(Connection),
    Disconnect,
    Home,
    Unlock,
    Stop,
    Run(String),
    Jog { dx: f64, dy: f64, feed: f64 },
    JogTo { x: f64, y: f64, feed: f64 },
    JogCancel,
    SetOrigin,
    SaveOrigin,
    SetSetting { key: u32, value: String },
    RefreshSettings,
}

/// Notifications from the worker to the UI.
pub enum Event {
    Connecting(String),
    Connected(String),
    /// The link dropped unexpectedly and a reconnect attempt is in progress.
    Reconnecting(u32),
    Disconnected(Option<String>),
    /// A raw `<...>` status report.
    Status(String),
    Settings(Settings),
    JobStarted(usize),
    JobProgress {
        done: usize,
        total: usize,
    },
    JobDone(usize),
    JobStopped,
    JobFailed(String),
    Message {
        text: String,
        error: bool,
    },
}

/// Handle held by the app. Dropping it closes the command channel, which makes
/// the worker exit after its current operation.
pub struct Machine {
    tx: Sender<Command>,
    rx: Receiver<Event>,
    /// Lets Disconnect interrupt a blocking connect/reconnect attempt.
    cancel: Arc<AtomicBool>,
    /// Set to abort an in-progress job without waiting for the worker loop.
    stream_stop: Arc<AtomicBool>,
    /// Set to feed-hold an in-progress job and resume when cleared.
    stream_pause: Arc<AtomicBool>,
}

impl Machine {
    pub fn spawn() -> Self {
        let (tx, commands) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stream_stop = Arc::new(AtomicBool::new(false));
        let stream_pause = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_stop = Arc::clone(&stream_stop);
        let worker_pause = Arc::clone(&stream_pause);
        let _ = thread::Builder::new()
            .name("orleans-machine".to_owned())
            .spawn(move || worker(commands, events, worker_cancel, worker_stop, worker_pause));
        Self {
            tx,
            rx,
            cancel,
            stream_stop,
            stream_pause,
        }
    }

    /// Connect using a reusable TCP or serial connection.
    pub fn connect(&self, connection: Connection) -> bool {
        self.cancel.store(false, Ordering::Relaxed);
        self.tx.send(Command::Connect(connection)).is_ok()
    }

    pub fn disconnect(&self) -> bool {
        self.cancel.store(true, Ordering::Relaxed);
        self.tx.send(Command::Disconnect).is_ok()
    }

    pub fn home(&self) -> bool {
        self.tx.send(Command::Home).is_ok()
    }

    pub fn unlock(&self) -> bool {
        self.tx.send(Command::Unlock).is_ok()
    }

    pub fn stop(&self) -> bool {
        self.tx.send(Command::Stop).is_ok()
    }

    pub fn run(&self, program: &str) -> bool {
        self.stream_stop.store(false, Ordering::Relaxed);
        self.stream_pause.store(false, Ordering::Relaxed);
        self.tx.send(Command::Run(program.to_owned())).is_ok()
    }

    /// Abort a running job. The driver sends feed hold and reset itself.
    pub fn stop_job(&self) {
        self.stream_stop.store(true, Ordering::Relaxed);
        self.stream_pause.store(false, Ordering::Relaxed);
    }

    pub fn pause_job(&self) {
        self.stream_pause.store(true, Ordering::Relaxed);
    }

    pub fn resume_job(&self) {
        self.stream_pause.store(false, Ordering::Relaxed);
    }

    pub fn jog(&self, dx: f64, dy: f64, feed: f64) -> bool {
        self.tx.send(Command::Jog { dx, dy, feed }).is_ok()
    }

    pub fn jog_to(&self, x: f64, y: f64, feed: f64) -> bool {
        self.tx.send(Command::JogTo { x, y, feed }).is_ok()
    }

    pub fn jog_cancel(&self) -> bool {
        self.tx.send(Command::JogCancel).is_ok()
    }

    pub fn set_origin(&self) -> bool {
        self.tx.send(Command::SetOrigin).is_ok()
    }

    pub fn save_origin(&self) -> bool {
        self.tx.send(Command::SaveOrigin).is_ok()
    }

    pub fn set_setting(&self, key: u32, value: &str) -> bool {
        self.tx
            .send(Command::SetSetting {
                key,
                value: value.to_owned(),
            })
            .is_ok()
    }

    pub fn refresh_settings(&self) -> bool {
        self.tx.send(Command::RefreshSettings).is_ok()
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.rx.try_recv().ok()
    }
}

fn worker(
    commands: Receiver<Command>,
    events: Sender<Event>,
    cancel: Arc<AtomicBool>,
    stream_stop: Arc<AtomicBool>,
    stream_pause: Arc<AtomicBool>,
) {
    const MAX_RECONNECT_ATTEMPTS: u32 = 5;

    let mut client: Option<Controller<Box<dyn Transport>>> = None;
    let mut connection: Option<Connection> = None;
    let mut last_poll = Instant::now();
    let mut misses = 0u32;
    let mut warned = false;
    let mut last_controller_error: Option<String> = None;
    let mut reconnect_at: Option<Instant> = None;
    let mut reconnect_attempt = 0u32;

    loop {
        // Retry an unexpected drop. Attempts are spread out so a WiFi bridge
        // that reboots (for example during homing) has time to come back.
        if client.is_none() {
            if let Some(at) = reconnect_at {
                if Instant::now() >= at {
                    reconnect_at = None;
                    if let Some(target) = connection.clone() {
                        let result =
                            connect(&target, Duration::from_secs(2), Duration::from_secs(1));
                        if cancel.load(Ordering::Relaxed) {
                            connection = None;
                            reconnect_attempt = 0;
                            let _ = events.send(Event::Disconnected(None));
                            continue;
                        }
                        match result {
                            Ok((connected, status, settings)) => {
                                client = Some(connected);
                                last_poll = Instant::now();
                                misses = 0;
                                warned = false;
                                last_controller_error = None;
                                reconnect_attempt = 0;
                                let _ = events.send(Event::Connected(target.label()));
                                if let Some(status) = status {
                                    let _ = events.send(Event::Status(status));
                                }
                                let _ = events.send(Event::Settings(settings));
                                message(&events, "connection restored", false);
                            }
                            Err(err) => {
                                reconnect_attempt += 1;
                                if reconnect_attempt <= MAX_RECONNECT_ATTEMPTS {
                                    reconnect_at = Some(Instant::now() + Duration::from_secs(2));
                                    let _ = events.send(Event::Reconnecting(reconnect_attempt));
                                    message(&events, format!("reconnecting failed: {err}"), true);
                                } else {
                                    connection = None;
                                    reconnect_attempt = 0;
                                    let _ = events.send(Event::Disconnected(Some(format!(
                                        "could not reconnect: {err}"
                                    ))));
                                }
                            }
                        }
                    }
                }
            }
        }

        match commands.recv_timeout(Duration::from_millis(150)) {
            Ok(Command::Connect(target)) => {
                client = None;
                misses = 0;
                warned = false;
                last_controller_error = None;
                reconnect_at = None;
                reconnect_attempt = 0;
                let label = target.label();
                connection = Some(target.clone());
                let _ = events.send(Event::Connecting(label.clone()));
                let result = connect(&target, Duration::from_secs(5), Duration::from_secs(2));
                if cancel.load(Ordering::Relaxed) {
                    connection = None;
                    let _ = events.send(Event::Disconnected(None));
                    continue;
                }
                match result {
                    Ok((connected, status, settings)) => {
                        client = Some(connected);
                        last_poll = Instant::now();
                        let _ = events.send(Event::Connected(label));
                        if let Some(status) = status {
                            let _ = events.send(Event::Status(status));
                        }
                        let _ = events.send(Event::Settings(settings));
                    }
                    Err(err) => {
                        connection = None;
                        let _ = events.send(Event::Disconnected(Some(err)));
                    }
                }
            }
            Ok(Command::Disconnect) => {
                client = None;
                connection = None;
                reconnect_at = None;
                reconnect_attempt = 0;
                misses = 0;
                warned = false;
                last_controller_error = None;
                let _ = events.send(Event::Disconnected(None));
            }
            Ok(Command::Home) => match client.as_mut() {
                Some(connected) => {
                    let protocol = connected.protocol();
                    match connected.home() {
                        Ok(()) => message(
                            &events,
                            match protocol {
                                Protocol::Grbl => "homing started ($H)",
                                Protocol::Marlin => "homing started (G28)",
                            },
                            false,
                        ),
                        Err(err) => message(&events, format!("homing failed: {err}"), true),
                    }
                }
                None => message(&events, "not connected", true),
            },
            Ok(Command::Unlock) => match client.as_mut() {
                Some(connected) => {
                    let protocol = connected.protocol();
                    match connected.unlock() {
                        Ok(()) => message(
                            &events,
                            match protocol {
                                Protocol::Grbl => "alarm unlock requested ($X)",
                                Protocol::Marlin => "controller restart requested (M999)",
                            },
                            false,
                        ),
                        Err(err) => message(&events, format!("unlock failed: {err}"), true),
                    }
                }
                None => message(&events, "not connected", true),
            },
            Ok(Command::Stop) => match client.as_mut() {
                Some(connected) => {
                    let protocol = connected.protocol();
                    match connected.stop() {
                        Ok(()) => message(
                            &events,
                            match protocol {
                                Protocol::Grbl => "feed hold + soft reset sent; laser off",
                                Protocol::Marlin => {
                                    "emergency stop sent (M112); laser off; send Unlock (M999) to restart"
                                }
                            },
                            false,
                        ),
                        Err(err) => message(&events, format!("stop failed: {err}"), true),
                    }
                }
                None => message(&events, "not connected", true),
            },
            Ok(Command::Jog { dx, dy, feed }) => match client.as_mut() {
                Some(connected) => match connected.jog(dx, dy, feed) {
                    Ok(()) => message(&events, format!("jog X{dx:+.2} Y{dy:+.2} mm"), false),
                    Err(err) => message(&events, format!("jog failed: {err}"), true),
                },
                None => message(&events, "not connected", true),
            },
            Ok(Command::JogTo { x, y, feed }) => match client.as_mut() {
                Some(connected) => match connected.jog_to(x, y, feed) {
                    Ok(()) => message(&events, format!("jog to X{x:.2} Y{y:.2} mm"), false),
                    Err(err) => message(&events, format!("jog failed: {err}"), true),
                },
                None => message(&events, "not connected", true),
            },
            Ok(Command::JogCancel) => match client.as_mut() {
                Some(connected) => match connected.jog_cancel() {
                    Ok(()) => message(&events, "jog cancelled", false),
                    Err(err) => message(&events, format!("jog cancel failed: {err}"), true),
                },
                None => message(&events, "not connected", true),
            },
            Ok(Command::SetOrigin) => match client.as_mut() {
                Some(connected) => match connected.set_origin() {
                    Ok(()) => message(&events, "work origin set (G92 X0 Y0)", false),
                    Err(err) => message(&events, format!("setting origin failed: {err}"), true),
                },
                None => message(&events, "not connected", true),
            },
            Ok(Command::SaveOrigin) => match client.as_mut() {
                Some(connected) => match connected.save_origin() {
                    Ok(()) => message(
                        &events,
                        "stored work origin saved (G10 L20 P1 X0 Y0)",
                        false,
                    ),
                    Err(err) => message(&events, format!("saving origin failed: {err}"), true),
                },
                None => message(&events, "not connected", true),
            },
            Ok(Command::SetSetting { key, value }) => match client.as_mut() {
                Some(Controller::Grbl(connected)) => {
                    let line = format!("${key}={value}");
                    match connected.send(&line) {
                        Ok(()) => {
                            let settings = read_settings(connected);
                            let _ = events.send(Event::Settings(settings));
                            message(&events, format!("applied {line}"), false);
                        }
                        Err(err) => {
                            message(&events, format!("setting failed: {err}"), true);
                        }
                    }
                }
                Some(Controller::Marlin(_)) => {
                    message(&events, "Marlin settings are defined in the firmware", true)
                }
                None => message(&events, "not connected", true),
            },
            Ok(Command::RefreshSettings) => match client.as_mut() {
                Some(Controller::Grbl(connected)) => {
                    let settings = read_settings(connected);
                    let _ = events.send(Event::Settings(settings));
                    message(&events, "settings refreshed", false);
                }
                Some(Controller::Marlin(_)) => {
                    let settings = marlin_settings(None);
                    let _ = events.send(Event::Settings(settings));
                    message(
                        &events,
                        "Marlin settings are defined in the firmware",
                        false,
                    );
                }
                None => message(&events, "not connected", true),
            },
            Ok(Command::Run(program)) => match client.as_mut() {
                Some(connected) => {
                    let total = program
                        .lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty() && !line.starts_with(';'))
                        .count();
                    let _ = events.send(Event::JobStarted(total));
                    let result = connected.run_pausable(
                        &program,
                        |done, total| {
                            let _ = events.send(Event::JobProgress { done, total });
                        },
                        &stream_stop,
                        &stream_pause,
                    );
                    match result {
                        Ok(acks) => {
                            let idle =
                                wait_until_idle(connected, &events, &stream_stop, &stream_pause);
                            if idle {
                                let _ = events.send(Event::JobDone(acks));
                            } else {
                                let _ = connected.stop();
                                let _ = events.send(Event::JobStopped);
                            }
                        }
                        Err(DriverError::Stopped) => {
                            let _ = events.send(Event::JobStopped);
                        }
                        Err(err) => {
                            let _ = events.send(Event::JobFailed(err.to_string()));
                        }
                    }
                }
                None => {
                    let _ = events.send(Event::JobFailed("not connected".to_owned()));
                }
            },
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if let Some(connected) = client.as_mut() {
            if last_poll.elapsed() >= Duration::from_millis(500) {
                last_poll = Instant::now();
                let timeout = if connected.protocol() == Protocol::Marlin {
                    Duration::from_millis(250)
                } else {
                    Duration::from_millis(750)
                };
                match connected.poll_status(timeout) {
                    Ok(poll) => {
                        for line in &poll.lines {
                            if controller_error(line)
                                && last_controller_error.as_deref() != Some(line.as_str())
                            {
                                last_controller_error = Some(line.clone());
                                let _ = events.send(Event::Message {
                                    text: format!("controller: {line}"),
                                    error: true,
                                });
                            }
                        }
                        if let Some(status) = poll.status {
                            misses = 0;
                            if warned {
                                warned = false;
                                message(&events, "status resumed", false);
                            }
                            let _ = events.send(Event::Status(status));
                        } else {
                            // Silence is not a disconnect: homing and very slow
                            // moves can delay status replies over WiFi.
                            misses += 1;
                            if misses >= 10 && !warned {
                                warned = true;
                                message(
                                    &events,
                                    "no status from the controller; still connected",
                                    true,
                                );
                            }
                        }
                    }
                    Err(err) => {
                        client = None;
                        misses = 0;
                        warned = false;
                        if connection.is_some() {
                            reconnect_attempt = 1;
                            reconnect_at = Some(Instant::now());
                            let _ = events.send(Event::Reconnecting(1));
                            message(&events, format!("link lost ({err}); reconnecting…"), true);
                        } else {
                            let _ = events
                                .send(Event::Disconnected(Some(format!("link error: {err}"))));
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
fn jog_line(dx: f64, dy: f64, feed: f64) -> String {
    format!("$J=G91 G21 X{dx:.3} Y{dy:.3} F{feed:.0}")
}

#[cfg(test)]
fn jog_absolute_line(x: f64, y: f64, feed: f64) -> String {
    format!("$J=G90 G21 X{x:.3} Y{y:.3} F{feed:.0}")
}

type Connected = (Controller<Box<dyn Transport>>, Option<String>, Settings);

fn connect(
    connection: &Connection,
    tcp_timeout: Duration,
    probe_timeout: Duration,
) -> Result<Connected, String> {
    let mut transport: Box<dyn Transport> = match connection.kind {
        LinkKind::Tcp => {
            let (host, port) = parse_endpoint(&connection.endpoint)?;
            let link = TcpTransport::connect(&host, port, tcp_timeout)
                .map_err(|err| format!("connecting to {host}:{port}: {err}"))?;
            Box::new(link)
        }
        LinkKind::Serial => {
            let link = orleans_driver::open_serial(&connection.endpoint, connection.baud).map_err(
                |err| {
                    format!(
                        "opening {} at {} baud: {err}",
                        connection.endpoint, connection.baud
                    )
                },
            )?;
            Box::new(link)
        }
    };

    // Auto-detection has no client yet, so wake a serial link before probing.
    // Forced drivers wake themselves in their own client.
    if connection.kind == LinkKind::Serial && connection.driver == DriverChoice::Auto {
        let _ = transport.write_raw(b"\r\n\r\n");
    }

    let (mut controller, status, firmware) = match connection.driver {
        DriverChoice::Auto => {
            let detection = Controller::detect(transport, probe_timeout, probe_timeout)
                .map_err(|err| format!("probing {}: {err}", connection.label()))?;
            (detection.controller, detection.status, detection.firmware)
        }
        DriverChoice::Grbl => {
            let mut client = GrblClient::new(transport);
            let _ = client.wake();
            // A forced driver is a recovery path: keep the connection even if
            // the controller does not answer the status probe.
            let status = client
                .probe_status(probe_timeout)
                .ok()
                .and_then(|probe| probe.status);
            (Controller::Grbl(client), status, None::<String>)
        }
        DriverChoice::Marlin => {
            let mut client = MarlinClient::new(transport);
            let _ = client.wake();
            let firmware = client
                .probe_identity(probe_timeout)
                .ok()
                .and_then(|probe| probe.firmware);
            let status = client.status_line().ok().flatten();
            (Controller::Marlin(client), status, firmware)
        }
    };

    let settings = match &mut controller {
        Controller::Grbl(client) => read_settings(client),
        Controller::Marlin(_) => marlin_settings(firmware),
    };
    Ok((controller, status, settings))
}

/// GRBL acknowledges lines when they enter the planner, which can be ahead of
/// the actual motion. Wait for an `Idle` status before declaring a job done.
/// Returns false if the user asked to stop while the last moves finish.
fn wait_until_idle(
    connected: &mut Controller<Box<dyn Transport>>,
    events: &Sender<Event>,
    stop: &AtomicBool,
    pause: &AtomicBool,
) -> bool {
    let Controller::Grbl(client) = connected else {
        // Marlin's client waits for M400 before returning from the stream.
        return true;
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        if pause.load(Ordering::Relaxed) {
            let _ = client.feed_hold();
            while pause.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
            }
            if stop.load(Ordering::Relaxed) {
                return false;
            }
            let _ = client.resume();
        }
        let _ = client.request_status();
        if let Ok(lines) = client.collect_for(Duration::from_millis(200)) {
            for line in lines.iter().filter(|line| line.starts_with('<')) {
                let _ = events.send(Event::Status(line.clone()));
                if let Some(status) = Status::parse(line) {
                    if status.state == "Idle" {
                        return true;
                    }
                }
            }
        }
    }
    true
}

/// Controller settings that affect GUI safety.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Firmware family detected during connection.
    pub protocol: Protocol,
    /// `M115` firmware banner for Marlin; `None` for GRBL.
    pub firmware: Option<String>,
    /// `$32=1`: laser mode. Required for a safe M4 pause.
    pub laser_mode: Option<bool>,
    /// `$30`: the S value for 100 % power.
    pub max_s: Option<u32>,
    /// `$22=1`: homing enabled.
    pub homing: Option<bool>,
    /// `$20=1`: soft limits enabled.
    pub soft_limits: Option<bool>,
    /// `$130`/`$131`: X/Y maximum travel, used as the machine bed size.
    pub bed: Option<[f64; 2]>,
    /// Every `$n=value` returned by `$$`, for the raw list.
    pub all: BTreeMap<u32, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            protocol: Protocol::Grbl,
            firmware: None,
            laser_mode: None,
            max_s: None,
            homing: None,
            soft_limits: None,
            bed: None,
            all: BTreeMap::new(),
        }
    }
}

fn read_settings(client: &mut GrblClient<Box<dyn Transport>>) -> Settings {
    let mut settings = Settings::default();
    if client.write_command("$$").is_err() {
        return settings;
    }
    let deadline = Instant::now() + Duration::from_millis(1200);
    while Instant::now() < deadline {
        match client.transport_mut().read_line() {
            Ok(Some(line)) => {
                parse_setting(&line, &mut settings);
                if line.trim() == "ok" {
                    break;
                }
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(_) => break,
        }
    }
    settings
}

fn marlin_settings(firmware: Option<String>) -> Settings {
    Settings {
        protocol: Protocol::Marlin,
        firmware,
        laser_mode: Some(true),
        max_s: Some(Protocol::Marlin.default_max_s()),
        homing: Some(true),
        soft_limits: None,
        bed: None,
        all: BTreeMap::new(),
    }
}

fn parse_setting(line: &str, settings: &mut Settings) {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix('$') {
        if let Some((key, value)) = rest.split_once('=') {
            if let Ok(key) = key.parse::<u32>() {
                settings.all.insert(key, value.trim().to_owned());
            }
        }
    }
    if let Some(value) = line.strip_prefix("$32=") {
        settings.laser_mode = value.parse::<u8>().ok().map(|value| value != 0);
    } else if let Some(value) = line.strip_prefix("$30=") {
        settings.max_s = value.parse().ok();
    } else if let Some(value) = line.strip_prefix("$22=") {
        settings.homing = value.parse::<u8>().ok().map(|value| value != 0);
    } else if let Some(value) = line.strip_prefix("$20=") {
        settings.soft_limits = value.parse::<u8>().ok().map(|value| value != 0);
    }
    settings.bed = match (
        settings
            .all
            .get(&130)
            .and_then(|value| value.parse::<f64>().ok()),
        settings
            .all
            .get(&131)
            .and_then(|value| value.parse::<f64>().ok()),
    ) {
        (Some(x), Some(y)) if x > 0.0 && y > 0.0 => Some([x, y]),
        _ => None,
    };
}

fn controller_error(line: &str) -> bool {
    let upper = line.trim().to_ascii_uppercase();
    upper.starts_with("ERROR") || upper.starts_with("!!") || upper.starts_with("ALARM")
}

fn message(events: &Sender<Event>, text: impl Into<String>, error: bool) {
    let _ = events.send(Event::Message {
        text: text.into(),
        error,
    });
}

/// A parsed GRBL status report.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub raw: String,
    pub state: String,
    pub mpos: Option<[f64; 3]>,
    pub wpos: Option<[f64; 3]>,
    pub wco: Option<[f64; 3]>,
    pub feed: Option<u32>,
    pub spindle: Option<u32>,
}

impl Status {
    /// Parse `<Idle|WPos:1.000,2.000,0.000|FS:1000,0>`.
    pub fn parse(line: &str) -> Option<Self> {
        let raw = line.trim();
        let body = raw.strip_prefix('<')?.strip_suffix('>')?;
        let mut fields = body.split('|');
        let state = fields.next()?.trim().to_owned();
        if state.is_empty() {
            return None;
        }

        let parse_xyz = |values: &str| {
            let nums: Vec<f64> = values
                .split(',')
                .filter_map(|value| value.trim().parse().ok())
                .collect();
            (nums.len() >= 2).then(|| [nums[0], nums[1], *nums.get(2).unwrap_or(&0.0)])
        };

        let mut mpos = None;
        let mut wpos = None;
        let mut wco = None;
        let mut feed = None;
        let mut spindle = None;
        for field in fields {
            if let Some(values) = field.strip_prefix("MPos:") {
                mpos = parse_xyz(values);
            } else if let Some(values) = field.strip_prefix("WPos:") {
                wpos = parse_xyz(values);
            } else if let Some(values) = field.strip_prefix("WCO:") {
                wco = parse_xyz(values);
            } else if let Some(values) = field.strip_prefix("FS:") {
                let mut values = values.split(',');
                feed = values.next().and_then(|value| value.trim().parse().ok());
                spindle = values.next().and_then(|value| value.trim().parse().ok());
            }
        }

        Some(Self {
            raw: raw.to_owned(),
            state,
            mpos,
            wpos,
            wco,
            feed,
            spindle,
        })
    }

    /// Work offset: reported directly as `WCO`, or derived from `MPos-WPos`.
    pub fn work_offset(&self) -> Option<[f64; 3]> {
        if let Some(wco) = self.wco {
            return Some(wco);
        }
        match (self.mpos, self.wpos) {
            (Some(mpos), Some(wpos)) => {
                Some([mpos[0] - wpos[0], mpos[1] - wpos[1], mpos[2] - wpos[2]])
            }
            _ => None,
        }
    }

    /// Work position: reported directly as `WPos`, or derived from `MPos` and
    /// `WCO` (`WPos = MPos - WCO`).
    pub fn work_position(&self) -> Option<[f64; 3]> {
        if let Some(wpos) = self.wpos {
            return Some(wpos);
        }
        match (self.mpos, self.wco) {
            (Some(mpos), Some(wco)) => Some([mpos[0] - wco[0], mpos[1] - wco[1], mpos[2] - wco[2]]),
            _ => None,
        }
    }

    pub fn is_alarm(&self) -> bool {
        self.state.starts_with("Alarm")
    }

    /// Homing is only accepted when the controller is idle or in an alarm,
    /// and never while the laser is still enabled.
    pub fn can_home(&self) -> bool {
        if self.is_alarm() {
            return true;
        }
        self.state == "Idle" && self.spindle.unwrap_or(0) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A tiny GRBL-over-TCP controller for the worker test.
    ///
    /// When `silence_after_home` is set, the controller stops answering `?`
    /// for that long after `$H`, which is what some WiFi bridges do while the
    /// machine homes.
    fn spawn_controller(
        silence_after_home: Option<Duration>,
    ) -> (u16, Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("timeout");
            let mut buffer = [0u8; 256];
            let mut line = String::new();
            let mut silent_until: Option<Instant> = None;
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        for byte in &buffer[..n] {
                            match *byte {
                                b'?' => {
                                    let silent = silent_until
                                        .map(|until| Instant::now() < until)
                                        .unwrap_or(false);
                                    if !silent {
                                        silent_until = None;
                                        let _ = stream
                                            .write_all(b"<Idle|MPos:0.000,0.000,0.000|FS:0,0>\n");
                                        let _ = stream.flush();
                                    }
                                }
                                b'\n' => {
                                    let command = std::mem::take(&mut line);
                                    let command = command.trim().to_owned();
                                    if command != "$$" {
                                        if command == "$$" {
                                            let _ =
                                                stream.write_all(b"$30=1000\n$32=1\n$22=1\nok\n");
                                            let _ = stream.flush();
                                            continue;
                                        }
                                        let _ = tx.send(command.clone());
                                    }
                                    let _ = stream.write_all(b"ok\n");
                                    let _ = stream.flush();
                                    if command == "$H" {
                                        if let Some(silence) = silence_after_home {
                                            silent_until = Some(Instant::now() + silence);
                                        }
                                    }
                                }
                                b'\r' => {}
                                other => line.push(other as char),
                            }
                        }
                    }
                    Err(err)
                        if err.kind() == std::io::ErrorKind::WouldBlock
                            || err.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        continue
                    }
                    Err(_) => break,
                }
            }
        });
        (port, rx, handle)
    }

    fn tcp_connection(port: u16) -> Connection {
        Connection::tcp(format!("tcp://127.0.0.1:{port}"), DriverChoice::Auto)
    }

    /// A GRBL controller on a pseudo-terminal, for exercising the serial path
    /// without hardware. The returned slave fd must stay open until the client
    /// has opened the port; close it afterwards to let the emulator exit.
    #[cfg(unix)]
    fn spawn_serial_grbl() -> (String, i32, thread::JoinHandle<()>) {
        use std::os::unix::io::FromRawFd;

        let pty = nix::pty::openpty(None, None).expect("openpty");
        let path = nix::unistd::ttyname(pty.slave)
            .expect("ttyname")
            .to_string_lossy()
            .into_owned();
        let mut master = unsafe { std::fs::File::from_raw_fd(pty.master) };
        let handle = thread::spawn(move || {
            let mut buffer = [0u8; 256];
            let mut line = Vec::new();
            loop {
                match master.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        for byte in &buffer[..n] {
                            match *byte {
                                b'?' => {
                                    let _ =
                                        master.write_all(b"<Idle|MPos:1.000,2.000,0.000|FS:0,0>\n");
                                    let _ = master.flush();
                                }
                                b'\n' => {
                                    let command = String::from_utf8_lossy(&line).trim().to_owned();
                                    line.clear();
                                    if command.is_empty() {
                                        continue;
                                    }
                                    let response = match command.as_str() {
                                        "$$" => "$32=1\n$30=1000\n$22=1\n$130=400\n$131=300\nok\n",
                                        _ => "ok\n",
                                    };
                                    let _ = master.write_all(response.as_bytes());
                                    let _ = master.flush();
                                }
                                b'\r' => {}
                                other => line.push(other),
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        (path, pty.slave, handle)
    }

    /// A tiny Marlin-over-TCP controller. It ignores GRBL `?` queries and
    /// answers the commands used by the Marlin driver.
    fn spawn_marlin_controller(identity: bool) -> (u16, Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("timeout");
            let mut buffer = [0u8; 256];
            let mut line = String::new();
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        for byte in &buffer[..n] {
                            match *byte {
                                b'?' => {}
                                b'\n' => {
                                    let command = std::mem::take(&mut line);
                                    let command = command.trim().to_owned();
                                    if command.is_empty() {
                                        continue;
                                    }
                                    let _ = tx.send(command.clone());
                                    let response = match command.as_str() {
                                        "M115" if identity => "FIRMWARE_NAME:Marlin 2.1.2 (test) PROTOCOL_VERSION:1.0 EXTRUDER_COUNT:1\nCap:SERIAL_XON_XOFF:0\nok\n",
                                        "M115" => "ok\n",
                                        "M114" => "X:1.000 Y:2.000 Z:0.000 E:0.000 Count X:0 Y:0 Z:0\nok\n",
                                        "M105" => "T:0.00 /0.00 B:0.00 /0.00\nok\n",
                                        "M112" => "Error:Printer halted. kill() called!\n",
                                        _ => "ok\n",
                                    };
                                    let _ = stream.write_all(response.as_bytes());
                                    let _ = stream.flush();
                                }
                                b'\r' => {}
                                other => line.push(other as char),
                            }
                        }
                    }
                    Err(err)
                        if err.kind() == std::io::ErrorKind::WouldBlock
                            || err.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        continue
                    }
                    Err(_) => break,
                }
            }
        });
        (port, rx, handle)
    }

    #[test]
    fn absolute_jog_uses_current_work_coordinates() {
        assert_eq!(
            jog_absolute_line(0.0, 0.0, 1000.0),
            "$J=G90 G21 X0.000 Y0.000 F1000"
        );
        assert_eq!(
            jog_absolute_line(12.5, -3.25, 250.0),
            "$J=G90 G21 X12.500 Y-3.250 F250"
        );
    }

    #[test]
    fn worker_connects_reports_status_and_homes() {
        let (port, commands, server) = spawn_controller(None);
        let machine = Machine::spawn();
        assert!(machine.connect(tcp_connection(port)));

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut connected = false;
        let mut status = None;
        while Instant::now() < deadline && (status.is_none() || !connected) {
            while let Some(event) = machine.try_recv() {
                match event {
                    Event::Connected(_) => connected = true,
                    Event::Status(line) => status = Some(line),
                    Event::Disconnected(reason) => panic!("disconnected: {reason:?}"),
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(connected, "did not connect");
        assert!(status.is_some(), "no status received");

        assert!(machine.home());
        let command = commands
            .recv_timeout(Duration::from_secs(2))
            .expect("home command");
        assert_eq!(command, "$H");

        assert!(machine.jog(1.0, 0.0, 500.0));
        let command = commands
            .recv_timeout(Duration::from_secs(2))
            .expect("jog command");
        assert_eq!(command, "$J=G91 G21 X1.000 Y0.000 F500");

        assert!(machine.set_origin());
        let command = commands
            .recv_timeout(Duration::from_secs(2))
            .expect("origin command");
        assert_eq!(command, "G92 X0 Y0");

        assert!(machine.save_origin());
        let command = commands
            .recv_timeout(Duration::from_secs(2))
            .expect("stored origin command");
        assert_eq!(command, "G10 L20 P1 X0 Y0");

        drop(machine);
        let _ = server.join();
    }

    #[test]
    fn worker_detects_marlin_and_uses_marlin_commands() {
        let (port, commands, server) = spawn_marlin_controller(true);
        let machine = Machine::spawn();
        assert!(machine.connect(tcp_connection(port)));

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut connected = false;
        let mut settings = None;
        let mut status = None;
        while Instant::now() < deadline && (settings.is_none() || status.is_none()) {
            while let Some(event) = machine.try_recv() {
                match event {
                    Event::Connected(_) => connected = true,
                    Event::Status(line) => status = Some(line),
                    Event::Settings(value) => settings = Some(value),
                    Event::Disconnected(reason) => panic!("disconnected: {reason:?}"),
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(connected, "did not connect");
        let settings = settings.expect("settings");
        assert_eq!(settings.protocol, Protocol::Marlin);
        assert_eq!(settings.max_s, Some(255));
        assert!(status
            .as_deref()
            .is_some_and(|line| line.contains("WPos:1.000,2.000,0.000")));

        assert!(machine.home());
        let mut saw_g28 = false;
        let mut saw_m400 = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !(saw_g28 && saw_m400) {
            match commands.recv_timeout(Duration::from_millis(200)) {
                Ok(command) if command == "G28" => saw_g28 = true,
                Ok(command) if command == "M400" => saw_m400 = true,
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(saw_g28 && saw_m400, "home commands missing");

        assert!(machine.jog(1.0, 0.0, 500.0));
        let mut jog = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && jog.len() < 3 {
            if let Ok(command) = commands.recv_timeout(Duration::from_millis(200)) {
                if command.starts_with("G9") || command.starts_with("G0 ") {
                    jog.push(command);
                }
            }
        }
        assert_eq!(jog, vec!["G91", "G0 X1.000 Y0.000 F500", "G90"]);

        drop(machine);
        let _ = server.join();
    }

    #[cfg(unix)]
    #[test]
    fn worker_connects_over_serial() {
        let (path, slave, server) = spawn_serial_grbl();
        let machine = Machine::spawn();
        assert!(machine.connect(Connection::serial(&path, 115_200, DriverChoice::Auto)));

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut connected = false;
        let mut settings = None;
        let mut status = None;
        while Instant::now() < deadline && (settings.is_none() || status.is_none()) {
            while let Some(event) = machine.try_recv() {
                match event {
                    Event::Connected(_) => connected = true,
                    Event::Settings(value) => settings = Some(value),
                    Event::Status(line) => status = Some(line),
                    Event::Disconnected(reason) => panic!("disconnected: {reason:?}"),
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(connected);
        let settings = settings.expect("settings");
        assert_eq!(settings.protocol, Protocol::Grbl);
        assert_eq!(settings.max_s, Some(1000));
        assert!(status
            .as_deref()
            .is_some_and(|line| line.contains("MPos:1.000,2.000,0.000")));

        nix::unistd::close(slave).ok();
        drop(machine);
        let _ = server.join();
    }

    #[test]
    fn worker_forced_marlin_connects_without_m115_identity() {
        let (port, _commands, server) = spawn_marlin_controller(false);
        let machine = Machine::spawn();
        assert!(machine.connect(Connection::tcp(
            format!("tcp://127.0.0.1:{port}"),
            DriverChoice::Marlin
        )));

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut settings = None;
        let mut status = None;
        while Instant::now() < deadline && (settings.is_none() || status.is_none()) {
            while let Some(event) = machine.try_recv() {
                match event {
                    Event::Settings(value) => settings = Some(value),
                    Event::Status(line) => status = Some(line),
                    Event::Disconnected(reason) => panic!("disconnected: {reason:?}"),
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        let settings = settings.expect("settings");
        assert_eq!(settings.protocol, Protocol::Marlin);
        assert!(settings.firmware.is_none());
        assert_eq!(settings.max_s, Some(255));
        assert!(status
            .as_deref()
            .is_some_and(|line| line.contains("WPos:1.000,2.000,0.000")));

        drop(machine);
        let _ = server.join();
    }

    #[test]
    fn worker_survives_status_silence_while_homing() {
        let (port, commands, server) = spawn_controller(Some(Duration::from_secs(4)));
        let machine = Machine::spawn();
        assert!(machine.connect(tcp_connection(port)));

        // Wait for the first status so we know the link is up.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut connected = false;
        while Instant::now() < deadline && !connected {
            while let Some(event) = machine.try_recv() {
                match event {
                    Event::Connected(_) => connected = true,
                    Event::Disconnected(reason) => panic!("disconnected: {reason:?}"),
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(connected);

        assert!(machine.home());
        commands
            .recv_timeout(Duration::from_secs(2))
            .expect("home command");

        // The controller is now silent for 4 s. The old four-miss rule would
        // drop the link after about 3 s; the worker must stay connected.
        let silence_end = Instant::now() + Duration::from_millis(4300);
        while Instant::now() < silence_end {
            while let Some(event) = machine.try_recv() {
                if let Event::Disconnected(reason) = event {
                    panic!("disconnected during homing: {reason:?}");
                }
            }
            thread::sleep(Duration::from_millis(20));
        }

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut resumed = false;
        while Instant::now() < deadline && !resumed {
            while let Some(event) = machine.try_recv() {
                match event {
                    Event::Status(_) => resumed = true,
                    Event::Disconnected(reason) => {
                        panic!("disconnected after homing: {reason:?}")
                    }
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(resumed, "status did not resume after homing");

        drop(machine);
        let _ = server.join();
    }

    #[test]
    fn worker_streams_a_job_and_reports_progress() {
        let (port, commands, server) = spawn_controller(None);
        let machine = Machine::spawn();
        assert!(machine.connect(tcp_connection(port)));

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut connected = false;
        while Instant::now() < deadline && !connected {
            while let Some(event) = machine.try_recv() {
                match event {
                    Event::Connected(_) => connected = true,
                    Event::Disconnected(reason) => panic!("disconnected: {reason:?}"),
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(connected);

        assert!(machine.run("G0 X1\nG0 X2\n"));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut done = None;
        let mut saw_progress = false;
        while Instant::now() < deadline && done.is_none() {
            while let Some(event) = machine.try_recv() {
                match event {
                    Event::JobProgress { .. } => saw_progress = true,
                    Event::JobDone(lines) => done = Some(lines),
                    Event::JobFailed(reason) => panic!("job failed: {reason}"),
                    Event::Disconnected(reason) => panic!("disconnected: {reason:?}"),
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(done, Some(2));
        assert!(saw_progress);

        let mut received = Vec::new();
        while received.len() < 2 {
            let command = commands
                .recv_timeout(Duration::from_secs(2))
                .expect("streamed command");
            received.push(command);
        }
        assert_eq!(received, ["G0 X1", "G0 X2"]);

        drop(machine);
        let _ = server.join();
    }

    #[test]
    fn parses_controller_settings() {
        let mut settings = Settings::default();
        parse_setting("$130=400", &mut settings);
        parse_setting("$131=300", &mut settings);
        parse_setting("$30=1000", &mut settings);
        parse_setting("$32=1", &mut settings);
        parse_setting("$22=0", &mut settings);
        assert_eq!(settings.bed, Some([400.0, 300.0]));
        assert_eq!(settings.max_s, Some(1000));
        assert_eq!(settings.laser_mode, Some(true));
        assert_eq!(settings.homing, Some(false));

        parse_setting("$32=0", &mut settings);
        assert_eq!(settings.laser_mode, Some(false));
    }

    #[test]
    fn jog_lines_are_relative() {
        assert_eq!(jog_line(1.5, -2.0, 800.0), "$J=G91 G21 X1.500 Y-2.000 F800");
    }

    #[test]
    fn parses_an_idle_status() {
        let status = Status::parse("<Idle|MPos:1.000,2.500,0.000|FS:1000,0>").expect("status");
        assert_eq!(status.state, "Idle");
        assert_eq!(status.mpos, Some([1.0, 2.5, 0.0]));
        assert_eq!(status.feed, Some(1000));
        assert_eq!(status.spindle, Some(0));
        assert!(status.can_home());
    }

    #[test]
    fn parses_alarm_and_motion_states() {
        let alarm = Status::parse("<Alarm:1|MPos:0.000,0.000,0.000|FS:0,0>").expect("alarm");
        assert!(alarm.is_alarm());
        assert!(alarm.can_home());

        let run = Status::parse("<Run|MPos:0.000,0.000,0.000|FS:800,1000>").expect("run");
        assert!(!run.can_home());

        let laser_on = Status::parse("<Idle|MPos:0.000,0.000,0.000|FS:0,500>").expect("idle");
        assert!(!laser_on.can_home());
    }

    #[test]
    fn derives_the_work_position() {
        let status = Status::parse("<Idle|MPos:10.000,20.000,0.000|FS:0,0|WCO:5.000,5.000,0.000>")
            .expect("status");
        assert_eq!(status.work_position(), Some([5.0, 15.0, 0.0]));

        let status = Status::parse("<Idle|WPos:1.000,2.000,0.000|FS:0,0>").expect("status");
        assert_eq!(status.work_position(), Some([1.0, 2.0, 0.0]));
    }

    #[test]
    fn rejects_garbage() {
        assert!(Status::parse("Idle").is_none());
        assert!(Status::parse("<>").is_none());
    }
}
