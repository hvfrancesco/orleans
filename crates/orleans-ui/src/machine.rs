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

use orleans_driver::net::{parse_endpoint, TcpTransport};
use orleans_driver::{DriverError, GrblClient, GrblConfig, Transport};

/// Requests from the UI to the worker.
pub enum Command {
    Connect(String),
    Disconnect,
    Home,
    Unlock,
    Stop,
    Run(String),
    Jog { dx: f64, dy: f64, feed: f64 },
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

    pub fn connect(&self, endpoint: &str) -> bool {
        self.cancel.store(false, Ordering::Relaxed);
        self.tx.send(Command::Connect(endpoint.to_owned())).is_ok()
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

    let mut client: Option<GrblClient<Box<dyn Transport>>> = None;
    let mut endpoint: Option<String> = None;
    let mut last_poll = Instant::now();
    let mut misses = 0u32;
    let mut warned = false;
    let mut reconnect_at: Option<Instant> = None;
    let mut reconnect_attempt = 0u32;

    loop {
        // Retry an unexpected drop. Attempts are spread out so a WiFi bridge
        // that reboots (for example during homing) has time to come back.
        if client.is_none() {
            if let Some(at) = reconnect_at {
                if Instant::now() >= at {
                    reconnect_at = None;
                    if let Some(target) = endpoint.clone() {
                        let result =
                            connect(&target, Duration::from_secs(2), Duration::from_secs(1));
                        if cancel.load(Ordering::Relaxed) {
                            endpoint = None;
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
                                reconnect_attempt = 0;
                                let _ = events.send(Event::Connected(target));
                                let _ = events.send(Event::Status(status));
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
                                    endpoint = None;
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
                reconnect_at = None;
                reconnect_attempt = 0;
                endpoint = Some(target.clone());
                let _ = events.send(Event::Connecting(target.clone()));
                let result = connect(&target, Duration::from_secs(5), Duration::from_secs(2));
                if cancel.load(Ordering::Relaxed) {
                    endpoint = None;
                    let _ = events.send(Event::Disconnected(None));
                    continue;
                }
                match result {
                    Ok((connected, status, settings)) => {
                        client = Some(connected);
                        last_poll = Instant::now();
                        let _ = events.send(Event::Connected(target));
                        let _ = events.send(Event::Status(status));
                        let _ = events.send(Event::Settings(settings));
                    }
                    Err(err) => {
                        endpoint = None;
                        let _ = events.send(Event::Disconnected(Some(err)));
                    }
                }
            }
            Ok(Command::Disconnect) => {
                client = None;
                endpoint = None;
                reconnect_at = None;
                reconnect_attempt = 0;
                misses = 0;
                warned = false;
                let _ = events.send(Event::Disconnected(None));
            }
            Ok(Command::Home) => match client.as_mut() {
                Some(connected) => match connected.write_command("$H") {
                    Ok(()) => message(&events, "homing started ($H)", false),
                    Err(err) => message(&events, format!("homing failed: {err}"), true),
                },
                None => message(&events, "not connected", true),
            },
            Ok(Command::Unlock) => match client.as_mut() {
                Some(connected) => match connected.write_command("$X") {
                    Ok(()) => message(&events, "alarm unlock requested ($X)", false),
                    Err(err) => message(&events, format!("unlock failed: {err}"), true),
                },
                None => message(&events, "not connected", true),
            },
            Ok(Command::Stop) => match client.as_mut() {
                Some(connected) => match connected.stop() {
                    Ok(()) => message(&events, "feed hold + soft reset sent; laser off", false),
                    Err(err) => message(&events, format!("stop failed: {err}"), true),
                },
                None => message(&events, "not connected", true),
            },
            Ok(Command::Jog { dx, dy, feed }) => match client.as_mut() {
                Some(connected) => {
                    let line = jog_line(dx, dy, feed);
                    match connected.write_command(&line) {
                        Ok(()) => message(&events, format!("jog X{dx:+.2} Y{dy:+.2} mm"), false),
                        Err(err) => message(&events, format!("jog failed: {err}"), true),
                    }
                }
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
                Some(connected) => match connected.write_command("G92 X0 Y0") {
                    Ok(()) => message(&events, "work origin set (G92 X0 Y0)", false),
                    Err(err) => message(&events, format!("setting origin failed: {err}"), true),
                },
                None => message(&events, "not connected", true),
            },
            Ok(Command::SaveOrigin) => match client.as_mut() {
                Some(connected) => match connected.write_command("G10 L20 P1 X0 Y0") {
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
                Some(connected) => {
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
                None => message(&events, "not connected", true),
            },
            Ok(Command::RefreshSettings) => match client.as_mut() {
                Some(connected) => {
                    let settings = read_settings(connected);
                    let _ = events.send(Event::Settings(settings));
                    message(&events, "settings refreshed", false);
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
                let _ = connected.request_status();
                match connected.collect_for(Duration::from_millis(250)) {
                    Ok(lines) => {
                        for line in &lines {
                            if line.starts_with("error") || line.starts_with("ALARM") {
                                let _ = events.send(Event::Message {
                                    text: format!("controller: {line}"),
                                    error: true,
                                });
                            }
                        }
                        if let Some(status) = lines.iter().rev().find(|line| line.starts_with('<'))
                        {
                            misses = 0;
                            if warned {
                                warned = false;
                                message(&events, "status resumed", false);
                            }
                            let _ = events.send(Event::Status(status.clone()));
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
                        if endpoint.is_some() {
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

fn jog_line(dx: f64, dy: f64, feed: f64) -> String {
    format!("$J=G91 G21 X{dx:.3} Y{dy:.3} F{feed:.0}")
}

type Connected = (GrblClient<Box<dyn Transport>>, String, Settings);

fn connect(
    endpoint: &str,
    tcp_timeout: Duration,
    probe_timeout: Duration,
) -> Result<Connected, String> {
    let (host, port) = parse_endpoint(endpoint)?;
    let transport = TcpTransport::connect(&host, port, tcp_timeout)
        .map_err(|err| format!("connecting to {host}:{port}: {err}"))?;
    let mut client = GrblClient::with_config(
        Box::new(transport) as Box<dyn Transport>,
        GrblConfig {
            window: 120,
            timeout: Duration::from_secs(30),
        },
    );
    let probe = client
        .probe_status(probe_timeout)
        .map_err(|err| format!("probing {host}:{port}: {err}"))?;
    match probe.status {
        Some(status) => {
            let settings = read_settings(&mut client);
            Ok((client, status, settings))
        }
        None => {
            let detail = if probe.seen.is_empty() {
                "nothing received".to_owned()
            } else {
                format!("received: {}", probe.seen.join(" | "))
            };
            Err(format!(
                "connected to {host}:{port}, but no GRBL status report ({detail}). \
                 Check that the controller is powered and that this is its Telnet port."
            ))
        }
    }
}

/// GRBL acknowledges lines when they enter the planner, which can be ahead of
/// the actual motion. Wait for an `Idle` status before declaring a job done.
/// Returns false if the user asked to stop while the last moves finish.
fn wait_until_idle(
    connected: &mut GrblClient<Box<dyn Transport>>,
    events: &Sender<Event>,
    stop: &AtomicBool,
    pause: &AtomicBool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        if pause.load(Ordering::Relaxed) {
            let _ = connected.feed_hold();
            while pause.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
            }
            if stop.load(Ordering::Relaxed) {
                return false;
            }
            let _ = connected.resume();
        }
        let _ = connected.request_status();
        if let Ok(lines) = connected.collect_for(Duration::from_millis(200)) {
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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    /// `$32=1`: laser mode. Required for a safe M4 pause.
    pub laser_mode: Option<bool>,
    /// `$30`: the S value for 100 % power.
    pub max_s: Option<u32>,
    /// `$22=1`: homing enabled.
    pub homing: Option<bool>,
    /// Every `$n=value` returned by `$$`, for the raw list.
    pub all: BTreeMap<u32, String>,
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
    }
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
    /// Parse `<Idle|MPos:1.000,2.000,0.000|FS:1000,0>`.
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
                .set_read_timeout(Some(Duration::from_secs(3)))
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

    #[test]
    fn worker_connects_reports_status_and_homes() {
        let (port, commands, server) = spawn_controller(None);
        let machine = Machine::spawn();
        assert!(machine.connect(&format!("tcp://127.0.0.1:{port}")));

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
    fn worker_survives_status_silence_while_homing() {
        let (port, commands, server) = spawn_controller(Some(Duration::from_secs(4)));
        let machine = Machine::spawn();
        assert!(machine.connect(&format!("tcp://127.0.0.1:{port}")));

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
        assert!(machine.connect(&format!("tcp://127.0.0.1:{port}")));

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
        parse_setting("$30=1000", &mut settings);
        parse_setting("$32=1", &mut settings);
        parse_setting("$22=0", &mut settings);
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
