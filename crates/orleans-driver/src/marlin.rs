//! Marlin laser streaming client.
//!
//! Marlin is a line-oriented G-code firmware. Unlike GRBL it has no realtime
//! `?` status report and no character-counting status response: every command
//! is acknowledged with `ok`, and `M400` is used to wait until the planner has
//! finished the moves queued before it. Status is built from `M114` (position)
//! and `M105` (temperatures), which is the information the machine panel needs.
//!
//! Laser control uses the same `M3`/`M4`/`M5` and `S` words as the GRBL
//! dialect. Marlin's configured PWM range is commonly 0..=255, so callers
//! should use [`crate::Protocol::default_max_s`] when generating a program.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::grbl::DriverError;
use crate::transport::Transport;

#[derive(Clone, Debug)]
pub struct MarlinConfig {
    /// Maximum number of unacknowledged bytes in flight. Marlin's serial RX
    /// buffer is 128 bytes on the common AVR builds.
    pub window: usize,
    /// Give up if the controller is silent for this long while streaming.
    pub timeout: Duration,
    /// `M400` can legitimately take a long time on a large job. This is a
    /// finish timeout, not a response timeout.
    pub drain_timeout: Duration,
}

impl Default for MarlinConfig {
    fn default() -> Self {
        Self {
            window: 120,
            timeout: Duration::from_secs(30),
            drain_timeout: Duration::from_secs(3600),
        }
    }
}

/// A status snapshot assembled from `M114` and `M105`.
#[derive(Clone, Debug, PartialEq)]
pub struct MarlinStatus {
    /// Machine/work position as reported by `M114`.
    pub position: Option<[f64; 3]>,
    /// Feed rate, when the firmware reports one in the status output.
    pub feed: Option<u32>,
    /// Raw lines received while asking for status.
    pub raw: Vec<String>,
    /// `Idle`, `Home`, or `Alarm` for the UI.
    pub state: String,
}

impl Default for MarlinStatus {
    fn default() -> Self {
        Self {
            position: None,
            feed: None,
            raw: Vec::new(),
            state: "Idle".to_owned(),
        }
    }
}

impl MarlinStatus {
    /// Render the snapshot in the GRBL status shape used by the editor.
    ///
    /// The editor deliberately keeps one status representation for all
    /// drivers. `WPos` is used because Marlin's `M114` already reports the
    /// current work position after `G92` offsets, and no separate WCO is
    /// available.
    pub fn as_grbl_status(&self) -> Option<String> {
        let [x, y, z] = self.position?;
        let feed = self.feed.unwrap_or(0);
        Some(format!(
            "<{}|WPos:{x:.3},{y:.3},{z:.3}|FS:{feed},0>",
            self.state
        ))
    }
}

/// What an `M115` identity probe found.
#[derive(Clone, Debug, Default)]
pub struct MarlinProbe {
    pub firmware: Option<String>,
    pub is_marlin: bool,
    pub lines: Vec<String>,
}

/// Blocking Marlin connection.
pub struct MarlinClient<T: Transport> {
    transport: T,
    config: MarlinConfig,
    pending: VecDeque<usize>,
    in_flight: usize,
    acked: usize,
    state: String,
    homing: bool,
}

impl<T: Transport> MarlinClient<T> {
    pub fn new(transport: T) -> Self {
        Self::with_config(transport, MarlinConfig::default())
    }

    pub fn with_config(transport: T, config: MarlinConfig) -> Self {
        Self {
            transport,
            config,
            pending: VecDeque::new(),
            in_flight: 0,
            acked: 0,
            state: "Idle".to_owned(),
            homing: false,
        }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    pub fn into_transport(self) -> T {
        self.transport
    }

    pub fn config(&self) -> &MarlinConfig {
        &self.config
    }

    /// Adjust the no-response timeout for an existing connection.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.config.timeout = timeout;
    }

    /// Wake a serial link by terminating any partial line and asking for the
    /// firmware identity.
    pub fn wake(&mut self) -> Result<(), DriverError> {
        self.transport.write_raw(b"\n")?;
        Ok(())
    }

    /// Ask for the firmware banner. Marlin answers `M115` with a set of
    /// `KEY:value` lines followed by `ok`.
    pub fn probe_identity(&mut self, timeout: Duration) -> Result<MarlinProbe, DriverError> {
        self.transport.write_line("M115")?;
        // Stop once a real identity line has been followed by `ok`. A GRBL
        // detection attempt may leave an unrelated `ok` in the buffer first;
        // ignoring that early `ok` avoids mistaking it for the M115 reply.
        let deadline = Instant::now() + timeout;
        let mut lines = Vec::new();
        let mut saw_identity = false;
        loop {
            if let Some(line) = self.transport.read_line()? {
                let upper = line.to_ascii_uppercase();
                if upper.contains("FIRMWARE_NAME:")
                    || upper.contains("PROTOCOL_VERSION:")
                    || upper.contains("PRINTER HALTED")
                {
                    saw_identity = true;
                }
                let finished = line.trim().eq_ignore_ascii_case("ok") && saw_identity;
                let halted = upper.contains("PRINTER HALTED");
                lines.push(line);
                if finished || halted {
                    break;
                }
                continue;
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(parse_identity(lines))
    }

    /// Ask for position and temperature. Both replies are read for `timeout`;
    /// the caller usually only needs the position.
    pub fn read_status(&mut self, timeout: Duration) -> Result<MarlinStatus, DriverError> {
        if !self.homing {
            self.transport.write_line("M114")?;
            self.transport.write_line("M105")?;
        }
        let raw = self.collect_for(timeout)?;

        let position = raw.iter().find_map(|line| parse_position(line));
        let feed = raw.iter().find_map(|line| parse_feed(line));
        let state = if self.homing {
            // M114 is queued after the M400 sent by home(), so a position
            // reply means homing has actually finished. Report one Home
            // sample first; the next poll reports Idle.
            if position.is_some() {
                self.homing = false;
                self.state = "Idle".to_owned();
                "Home".to_owned()
            } else {
                "Home".to_owned()
            }
        } else {
            self.state.clone()
        };

        Ok(MarlinStatus {
            position,
            feed,
            raw,
            state,
        })
    }

    /// Convenience wrapper used by the connection probe.
    pub fn status_line(&mut self) -> Result<Option<String>, DriverError> {
        Ok(self
            .read_status(Duration::from_millis(750))?
            .as_grbl_status())
    }

    /// Read whatever arrives for `timeout` without sending anything.
    pub fn collect_for(&mut self, timeout: Duration) -> Result<Vec<String>, DriverError> {
        let deadline = Instant::now() + timeout;
        let mut lines = Vec::new();
        loop {
            if let Some(line) = self.transport.read_line()? {
                lines.push(line);
                continue;
            }
            if Instant::now() >= deadline {
                return Ok(lines);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Send a single command and wait for its `ok`.
    pub fn send(&mut self, line: &str) -> Result<(), DriverError> {
        self.transport.write_line(line)?;
        self.pending.push_back(line.len() + 1);
        self.in_flight += line.len() + 1;
        let never = AtomicBool::new(false);
        let mut last_response = Instant::now();
        let mut held = false;
        self.wait_ack(&mut last_response, &never, &never, &mut held)
    }

    /// Write a command without waiting for its `ok`.
    pub fn write_command(&mut self, line: &str) -> Result<(), DriverError> {
        self.transport.write_line(line)?;
        Ok(())
    }

    /// Home the axes. `M400` is queued after `G28` so a later status query
    /// waits for homing to finish before reporting the new position.
    pub fn home(&mut self) -> Result<(), DriverError> {
        self.state = "Home".to_owned();
        self.homing = true;
        self.write_command("M5")?;
        self.write_command("G28")?;
        self.write_command("M400")?;
        // Queue one status read behind the homing move. While `homing` is set,
        // read_status only drains; this avoids filling Marlin's command queue
        // with status queries during a long G28 sequence.
        self.write_command("M114")?;
        self.write_command("M105")?;
        Ok(())
    }

    /// Clear Marlin's kill/error state.
    pub fn unlock(&mut self) -> Result<(), DriverError> {
        self.send("M999")?;
        self.state = "Idle".to_owned();
        self.homing = false;
        Ok(())
    }

    /// Stop immediately and switch the laser off.
    ///
    /// `M112` is Marlin's emergency stop. It halts the planner and disables
    /// the heaters/steppers. The controller then needs `M999` (the Unlock
    /// button) before it accepts another job.
    pub fn stop(&mut self) -> Result<(), DriverError> {
        // M5 first so the laser is off even on builds where M112 is queued
        // rather than handled by the emergency parser. M112 then halts the
        // planner immediately when EMERGENCY_PARSER is enabled.
        let _ = self.transport.write_line("M5");
        let _ = self.transport.write_line("M112");
        self.pending.clear();
        self.in_flight = 0;
        self.state = "Alarm".to_owned();
        self.homing = false;
        Ok(())
    }

    /// Jog in relative machine coordinates.
    pub fn jog(&mut self, dx: f64, dy: f64, feed: f64) -> Result<(), DriverError> {
        self.send("G91")?;
        self.send(&format!("G0 X{dx:.3} Y{dy:.3} F{feed:.0}"))?;
        self.send("G90")
    }

    /// Move to an absolute work position.
    pub fn jog_to(&mut self, x: f64, y: f64, feed: f64) -> Result<(), DriverError> {
        self.send("G90")?;
        self.send(&format!("G0 X{x:.3} Y{y:.3} F{feed:.0}"))
    }

    /// Marlin has no jog-cancel primitive; a quick stop is the safe equivalent.
    pub fn jog_cancel(&mut self) -> Result<(), DriverError> {
        self.send("M410")
    }

    /// Set temporary work zero. Marlin supports `G92`, but does not persist it.
    pub fn set_origin(&mut self) -> Result<(), DriverError> {
        self.send("G92 X0 Y0")
    }

    /// Stream a program and wait for the planner to drain.
    pub fn run(&mut self, gcode: &str) -> Result<usize, DriverError> {
        self.run_with_progress(gcode, |_, _| {})
    }

    pub fn run_with_progress<F>(&mut self, gcode: &str, progress: F) -> Result<usize, DriverError>
    where
        F: FnMut(usize, usize),
    {
        let never = AtomicBool::new(false);
        self.run_pausable(gcode, progress, &never, &never)
    }

    pub fn run_controlled<F>(
        &mut self,
        gcode: &str,
        progress: F,
        stop: &AtomicBool,
    ) -> Result<usize, DriverError>
    where
        F: FnMut(usize, usize),
    {
        let never = AtomicBool::new(false);
        self.run_pausable(gcode, progress, stop, &never)
    }

    /// Stream G-code. `pause` is not implemented for Marlin; the UI disables
    /// pause for this driver, but a caller that sets it gets `Unsupported`.
    pub fn run_pausable<F>(
        &mut self,
        gcode: &str,
        mut progress: F,
        stop: &AtomicBool,
        pause: &AtomicBool,
    ) -> Result<usize, DriverError>
    where
        F: FnMut(usize, usize),
    {
        match self.run_inner(gcode, &mut progress, stop, pause) {
            Ok(acks) => Ok(acks),
            Err(err) => {
                if self.acked > 0 || self.in_flight > 0 {
                    let _ = self.stop();
                }
                Err(err)
            }
        }
    }

    fn run_inner<F>(
        &mut self,
        gcode: &str,
        progress: &mut F,
        stop: &AtomicBool,
        pause: &AtomicBool,
    ) -> Result<usize, DriverError>
    where
        F: FnMut(usize, usize),
    {
        if pause.load(Ordering::Relaxed) {
            return Err(DriverError::Unsupported("pause"));
        }
        self.acked = 0;
        self.state = "Run".to_owned();
        let lines: Vec<&str> = gcode
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with(';'))
            .collect();
        let total = lines.len();
        let mut last_response = Instant::now();
        let mut held = false;
        progress(0, total);

        for line in lines {
            if stop.load(Ordering::Relaxed) {
                return Err(DriverError::Stopped);
            }
            if pause.load(Ordering::Relaxed) {
                return Err(DriverError::Unsupported("pause"));
            }
            progress(self.acked, total);

            let bytes = line.len() + 1;
            if bytes > self.config.window {
                return Err(DriverError::Controller(format!(
                    "line of {bytes} bytes does not fit in a {} byte window",
                    self.config.window
                )));
            }
            while self.in_flight + bytes > self.config.window {
                self.wait_ack(&mut last_response, stop, pause, &mut held)?;
                progress(self.acked, total);
            }
            self.transport.write_line(line)?;
            self.pending.push_back(bytes);
            self.in_flight += bytes;
        }

        while self.in_flight > 0 {
            self.wait_ack(&mut last_response, stop, pause, &mut held)?;
            progress(self.acked, total);
        }

        if stop.load(Ordering::Relaxed) {
            return Err(DriverError::Stopped);
        }
        self.drain(stop)?;
        self.state = "Idle".to_owned();
        progress(self.acked, total);
        Ok(self.acked)
    }

    /// Queue `M400` and wait for the planner to finish. This is what makes a
    /// Marlin job "done" rather than merely accepted into the command buffer.
    fn drain(&mut self, stop: &AtomicBool) -> Result<(), DriverError> {
        self.transport.write_line("M400")?;
        let deadline = Instant::now() + self.config.drain_timeout;
        loop {
            if stop.load(Ordering::Relaxed) {
                return Err(DriverError::Stopped);
            }
            if Instant::now() >= deadline {
                return Err(DriverError::Timeout(self.config.drain_timeout));
            }
            match self.transport.read_line()? {
                Some(line) => {
                    let line = line.trim();
                    if line.starts_with("ok") {
                        return Ok(());
                    }
                    if is_error(line) {
                        return Err(DriverError::Controller(line.to_owned()));
                    }
                }
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
    }

    fn wait_ack(
        &mut self,
        last_response: &mut Instant,
        stop: &AtomicBool,
        pause: &AtomicBool,
        _held: &mut bool,
    ) -> Result<(), DriverError> {
        loop {
            if stop.load(Ordering::Relaxed) {
                return Err(DriverError::Stopped);
            }
            if pause.load(Ordering::Relaxed) {
                return Err(DriverError::Unsupported("pause"));
            }
            if last_response.elapsed() > self.config.timeout {
                return Err(DriverError::Timeout(self.config.timeout));
            }
            match self.transport.read_line()? {
                Some(line) => {
                    *last_response = Instant::now();
                    let line = line.trim();
                    if line.starts_with("ok") {
                        if let Some(bytes) = self.pending.pop_front() {
                            self.in_flight = self.in_flight.saturating_sub(bytes);
                        }
                        self.acked += 1;
                        return Ok(());
                    }
                    if is_error(line) {
                        return Err(DriverError::Controller(line.to_owned()));
                    }
                }
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
    }
}

fn is_error(line: &str) -> bool {
    let upper = line.to_ascii_uppercase();
    upper.starts_with("ERROR:") || upper.starts_with("!!") || upper.starts_with("ALARM")
}

fn parse_identity(lines: Vec<String>) -> MarlinProbe {
    let firmware = lines
        .iter()
        .find(|line| line.to_ascii_uppercase().contains("FIRMWARE_NAME:"))
        .cloned();
    let joined = lines.join("\n").to_ascii_uppercase();
    let is_marlin = (joined.contains("FIRMWARE_NAME:") && joined.contains("MARLIN"))
        || (joined.contains("PROTOCOL_VERSION:") && joined.contains("EXTRUDER_COUNT:"))
        // A halted Marlin may answer M115 with its kill message instead of the
        // usual identity block. Recognise it so `unlock` can still find the
        // controller and send M999.
        || (joined.contains("PRINTER HALTED") && joined.contains("KILL() CALLED"));
    MarlinProbe {
        firmware,
        is_marlin,
        lines,
    }
}

/// Parse `X:1.000 Y:2.000 Z:0.000 E:0.00` or `M114 D` output.
fn parse_position(line: &str) -> Option<[f64; 3]> {
    let upper = line.trim().to_ascii_uppercase();
    if !upper.contains("X:") {
        return None;
    }
    let mut xyz = [0.0; 3];
    let mut found = 0;
    for (index, axis) in ['X', 'Y', 'Z'].iter().enumerate() {
        let needle = format!("{axis}:");
        let start = upper.find(&needle)?;
        let rest = upper[start + needle.len()..].trim_start();
        let value = rest
            .split(|c: char| c.is_ascii_whitespace())
            .next()
            .and_then(|value| value.parse::<f64>().ok())?;
        xyz[index] = value;
        found += 1;
    }
    (found >= 2).then_some(xyz)
}

fn parse_feed(line: &str) -> Option<u32> {
    let upper = line.trim().to_ascii_uppercase();
    let start = upper.find("F:")?;
    let rest = upper[start + 2..].trim_start();
    rest.split(|c: char| c.is_ascii_whitespace())
        .next()
        .and_then(|value| value.parse::<f64>().ok())
        .map(|value| value.max(0.0).round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::SimulatedMarlin;

    #[test]
    fn detects_marlin_identity() {
        let mut sim = SimulatedMarlin::new();
        let mut client = MarlinClient::new(&mut sim);
        let probe = client
            .probe_identity(Duration::from_millis(200))
            .expect("probe");
        assert!(probe.is_marlin);
        assert!(probe
            .firmware
            .as_deref()
            .is_some_and(|line| line.contains("Marlin")));
    }

    #[test]
    fn recognises_a_halted_controller() {
        let probe = parse_identity(vec!["Error:Printer halted. kill() called!".to_owned()]);
        assert!(probe.is_marlin);
    }

    #[test]
    fn reads_position_from_m114() {
        let mut sim = SimulatedMarlin::new();
        sim.set_position([7.0, 8.0, 0.0]);
        let mut client = MarlinClient::new(&mut sim);
        let status = client
            .read_status(Duration::from_millis(200))
            .expect("status");
        assert_eq!(status.position, Some([7.0, 8.0, 0.0]));
        assert!(status.raw.iter().any(|line| line.starts_with("T:")));
        let line = status.as_grbl_status().expect("status line");
        assert!(line.contains("WPos:7.000,8.000,0.000"));
    }

    #[test]
    fn streams_a_program_and_waits_for_m400() {
        let mut sim = SimulatedMarlin::new();
        {
            let mut client = MarlinClient::new(&mut sim);
            let acks = client
                .run("G21\nG90\nM5\nG0 X10 Y20\nM3 S200\nG1 X30 Y20 F1000\nM5\n")
                .expect("run");
            assert_eq!(acks, 7);
        }
        assert_eq!(sim.position(), [30.0, 20.0, 0.0]);
        assert!(!sim.laser_on());
        assert!(sim.received().iter().any(|line| line == "M400"));
    }

    #[test]
    fn respects_the_receive_window() {
        let mut sim = SimulatedMarlin::new();
        {
            let config = MarlinConfig {
                window: 24,
                timeout: Duration::from_secs(1),
                drain_timeout: Duration::from_secs(1),
            };
            let mut client = MarlinClient::with_config(&mut sim, config);
            let program = "G0 X1 Y1\nG0 X2 Y2\nG0 X3 Y3\nG0 X4 Y4\n";
            let acks = client.run(program).expect("run");
            assert_eq!(acks, 4);
        }
        assert_eq!(sim.position(), [4.0, 4.0, 0.0]);
    }

    #[test]
    fn surfaces_controller_errors() {
        let mut sim = SimulatedMarlin::new();
        let mut client = MarlinClient::new(&mut sim);
        let err = client.run("INVALID\n").unwrap_err();
        assert!(
            matches!(err, DriverError::Controller(ref m) if m.to_ascii_lowercase().contains("error"))
        );
    }

    #[test]
    fn stop_sends_m112() {
        let mut sim = SimulatedMarlin::new();
        {
            let mut client = MarlinClient::new(&mut sim);
            client.stop().expect("stop");
        }
        let received: Vec<&str> = sim.received().iter().map(String::as_str).collect();
        let laser_off = received.iter().position(|line| *line == "M5");
        let emergency = received.iter().position(|line| *line == "M112");
        assert!(laser_off.is_some() && emergency.is_some());
        assert!(laser_off < emergency, "M5 must precede M112");
        assert!(sim.killed());
    }

    #[test]
    fn home_queues_one_status_read_behind_m400() {
        let mut sim = SimulatedMarlin::new();
        {
            let mut client = MarlinClient::new(&mut sim);
            client.home().expect("home");
        }
        let received: Vec<&str> = sim.received().iter().map(String::as_str).collect();
        assert_eq!(received, vec!["M5", "G28", "M400", "M114", "M105"]);
    }

    #[test]
    fn homing_reports_home_then_idle() {
        let mut sim = SimulatedMarlin::new();
        let mut client = MarlinClient::new(&mut sim);
        client.home().expect("home");
        let first = client
            .read_status(Duration::from_millis(100))
            .expect("first status");
        assert_eq!(first.state, "Home");
        let second = client
            .read_status(Duration::from_millis(100))
            .expect("second status");
        assert_eq!(second.state, "Idle");
    }

    #[test]
    fn pause_is_not_supported() {
        let mut sim = SimulatedMarlin::new();
        let pause = AtomicBool::new(true);
        let stop = AtomicBool::new(false);
        let mut client = MarlinClient::new(&mut sim);
        let err = client
            .run_pausable("G0 X1\n", |_, _| {}, &stop, &pause)
            .unwrap_err();
        assert!(matches!(err, DriverError::Unsupported("pause")));
    }

    #[test]
    fn parses_position_and_feed() {
        let line = "X:1.5 Y:-2.25 Z:0.00 E:0.00 Count X:1 Y:2 Z:0";
        assert_eq!(parse_position(line), Some([1.5, -2.25, 0.0]));
        assert_eq!(parse_feed("X:1 Y:2 F:1200"), Some(1200));
    }
}
