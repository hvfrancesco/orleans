//! Background link to the laser controller.
//!
//! The GUI thread must never block on a socket, and GRBL needs continuous
//! status polling. A worker thread owns the connection and talks to the UI
//! through two channels: commands in, events out.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use orleans_driver::net::{parse_endpoint, TcpTransport};
use orleans_driver::{GrblClient, GrblConfig, Transport};

/// Requests from the UI to the worker.
pub enum Command {
    Connect(String),
    Disconnect,
    Home,
    Unlock,
    Stop,
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
}

impl Machine {
    pub fn spawn() -> Self {
        let (tx, commands) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let _ = thread::Builder::new()
            .name("orleans-machine".to_owned())
            .spawn(move || worker(commands, events, worker_cancel));
        Self { tx, rx, cancel }
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

    pub fn try_recv(&self) -> Option<Event> {
        self.rx.try_recv().ok()
    }
}

fn worker(commands: Receiver<Command>, events: Sender<Event>, cancel: Arc<AtomicBool>) {
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
                            Ok((connected, status)) => {
                                client = Some(connected);
                                last_poll = Instant::now();
                                misses = 0;
                                warned = false;
                                reconnect_attempt = 0;
                                let _ = events.send(Event::Connected(target));
                                let _ = events.send(Event::Status(status));
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
                    Ok((connected, status)) => {
                        client = Some(connected);
                        last_poll = Instant::now();
                        let _ = events.send(Event::Connected(target));
                        let _ = events.send(Event::Status(status));
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

fn connect(
    endpoint: &str,
    tcp_timeout: Duration,
    probe_timeout: Duration,
) -> Result<(GrblClient<Box<dyn Transport>>, String), String> {
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
        Some(status) => Ok((client, status)),
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

        let mut mpos = None;
        let mut feed = None;
        let mut spindle = None;
        for field in fields {
            if let Some(values) = field.strip_prefix("MPos:") {
                let nums: Vec<f64> = values
                    .split(',')
                    .filter_map(|value| value.trim().parse().ok())
                    .collect();
                if nums.len() >= 2 {
                    mpos = Some([nums[0], nums[1], *nums.get(2).unwrap_or(&0.0)]);
                }
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
            feed,
            spindle,
        })
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
                                    let _ = tx.send(command.clone());
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
    fn rejects_garbage() {
        assert!(Status::parse("Idle").is_none());
        assert!(Status::parse("<>").is_none());
    }
}
