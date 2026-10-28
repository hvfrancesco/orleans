//! GRBL streaming client.
//!
//! GRBL has a 128-byte receive buffer and replies `ok` (or `error:N`) per line.
//! Streaming too fast overflows it, so this client uses the standard
//! "character counting" flow control: keep at most `window` bytes unacknowledged.
//!
//! The loop is synchronous. A GUI would run [`GrblClient::run`] on a worker
//! thread and drive `feed_hold` / `resume` from the UI; the realtime bytes are
//! handled by the controller even while it is busy, so they do not need to
//! interleave with the streaming thread.

use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

use crate::transport::Transport;

#[derive(Clone, Debug)]
pub struct GrblConfig {
    /// Maximum number of unacknowledged bytes in flight.
    pub window: usize,
    /// Give up if the controller does not answer for this long.
    pub timeout: Duration,
}

impl Default for GrblConfig {
    fn default() -> Self {
        Self {
            // GRBL's RX buffer is 128 bytes; leave a little headroom.
            window: 120,
            timeout: Duration::from_secs(10),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("controller error: {0}")]
    Controller(String),
    #[error("controller did not respond within {0:?}")]
    Timeout(Duration),
}

/// What a status probe found.
#[derive(Debug, Default)]
pub struct StatusProbe {
    pub status: Option<String>,
    /// Everything else received while probing: banners, `ok`, errors, echoes.
    pub seen: Vec<String>,
}

/// Blocking GRBL connection.
pub struct GrblClient<T: Transport> {
    transport: T,
    config: GrblConfig,
    pending: VecDeque<usize>,
    in_flight: usize,
    acked: usize,
}

impl<T: Transport> GrblClient<T> {
    pub fn new(transport: T) -> Self {
        Self::with_config(transport, GrblConfig::default())
    }

    pub fn with_config(transport: T, config: GrblConfig) -> Self {
        Self {
            transport,
            config,
            pending: VecDeque::new(),
            in_flight: 0,
            acked: 0,
        }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    pub fn config(&self) -> &GrblConfig {
        &self.config
    }

    /// GRBL prints its banner when it sees a newline; two are the usual wake-up.
    pub fn wake(&mut self) -> Result<(), DriverError> {
        self.transport.write_raw(b"\r\n\r\n")?;
        Ok(())
    }

    /// Ask for a status report (`<Idle|MPos:...>`).
    pub fn request_status(&mut self) -> Result<(), DriverError> {
        self.transport.write_raw(b"?")?;
        Ok(())
    }

    /// Read one status report, if the controller sends one within a short
    /// window. A single 20 ms socket timeout is not enough over WiFi.
    pub fn read_status(&mut self) -> Result<Option<String>, DriverError> {
        Ok(self.probe_status(Duration::from_millis(750))?.status)
    }

    /// Ask for status repeatedly until `timeout`, collecting any other lines.
    ///
    /// Retrying matters for three reasons: WiFi latency exceeds one read
    /// timeout, the first `?` can be sent before a bridge finishes connecting,
    /// and some line-buffered Telnet bridges only forward a realtime byte when
    /// a newline follows. After half the timeout the probe also sends `?\n`.
    pub fn probe_status(&mut self, timeout: Duration) -> Result<StatusProbe, DriverError> {
        let start = Instant::now();
        let deadline = start + timeout;
        let mut seen = Vec::new();
        let mut next_request = start;
        let mut line_mode = false;

        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(StatusProbe { status: None, seen });
            }

            if now >= next_request {
                self.transport.write_raw(b"?")?;
                // Fallback for bridges that buffer a whole line.
                if !line_mode && now.duration_since(start) >= timeout / 2 {
                    self.transport.write_line("?")?;
                    line_mode = true;
                }
                next_request = now + Duration::from_millis(400);
            }

            match self.transport.read_line()? {
                Some(line) => {
                    if line.starts_with('<') {
                        return Ok(StatusProbe {
                            status: Some(line),
                            seen,
                        });
                    }
                    seen.push(line);
                }
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
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

    /// Feed hold (realtime `!`). The machine decelerates and stops.
    pub fn feed_hold(&mut self) -> Result<(), DriverError> {
        self.transport.write_raw(b"!")?;
        Ok(())
    }

    /// Cycle start / resume (realtime `~`).
    pub fn resume(&mut self) -> Result<(), DriverError> {
        self.transport.write_raw(b"~")?;
        Ok(())
    }

    /// Soft reset (Ctrl-X). Aborts the job and clears the buffer.
    pub fn soft_reset(&mut self) -> Result<(), DriverError> {
        self.transport.write_raw(b"\x18")?;
        self.pending.clear();
        self.in_flight = 0;
        Ok(())
    }

    /// Send a single command and wait for its `ok`.
    pub fn send(&mut self, line: &str) -> Result<(), DriverError> {
        let started = Instant::now();
        self.transport.write_line(line)?;
        self.pending.push_back(line.len() + 1);
        self.in_flight += line.len() + 1;
        self.wait_ack(started)
    }

    /// Stream a whole G-code program, respecting the receive window.
    ///
    /// Returns the number of acknowledged lines.
    pub fn run(&mut self, gcode: &str) -> Result<usize, DriverError> {
        let started = Instant::now();
        self.acked = 0;

        for line in gcode.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            let bytes = line.len() + 1;
            if bytes > self.config.window {
                return Err(DriverError::Controller(format!(
                    "line of {bytes} bytes does not fit in a {} byte window",
                    self.config.window
                )));
            }
            while self.in_flight + bytes > self.config.window {
                self.wait_ack(started)?;
            }
            self.transport.write_line(line)?;
            self.pending.push_back(bytes);
            self.in_flight += bytes;
        }

        while self.in_flight > 0 {
            self.wait_ack(started)?;
        }
        Ok(self.acked)
    }

    fn wait_ack(&mut self, started: Instant) -> Result<(), DriverError> {
        loop {
            if started.elapsed() > self.config.timeout {
                return Err(DriverError::Timeout(self.config.timeout));
            }
            match self.transport.read_line()? {
                Some(line) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if line.starts_with("ok") {
                        if let Some(bytes) = self.pending.pop_front() {
                            self.in_flight = self.in_flight.saturating_sub(bytes);
                        }
                        self.acked += 1;
                        return Ok(());
                    }
                    if line.starts_with("error") || line.starts_with("ALARM") {
                        return Err(DriverError::Controller(line.to_owned()));
                    }
                    // Banner and status reports are informational.
                }
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{RunState, SimulatedGrbl};

    #[test]
    fn streams_a_program_to_the_simulator() {
        let mut sim = SimulatedGrbl::new();
        {
            let mut client = GrblClient::new(&mut sim);
            let acks = client
                .run("G21\nG90\nM5\nG0 X10 Y20\nM3 S500\nG1 X30 Y20 F1000\nM5\n")
                .expect("run");
            assert_eq!(acks, 7);
        }
        assert_eq!(sim.pos[0], 30.0);
        assert_eq!(sim.pos[1], 20.0);
        assert_eq!(sim.state, RunState::Idle);
    }

    #[test]
    fn respects_the_receive_window() {
        let mut sim = SimulatedGrbl::new();
        {
            let config = GrblConfig {
                window: 24,
                timeout: Duration::from_secs(1),
            };
            let mut client = GrblClient::with_config(&mut sim, config);
            let program = "G0 X1 Y1\nG0 X2 Y2\nG0 X3 Y3\nG0 X4 Y4\n";
            let acks = client.run(program).expect("run");
            assert_eq!(acks, 4);
        }
        assert_eq!(sim.pos, [4.0, 4.0, 0.0]);
    }

    #[test]
    fn surfaces_controller_errors() {
        let mut sim = SimulatedGrbl::new();
        let mut client = GrblClient::new(&mut sim);
        let err = client.run("INVALID\n").unwrap_err();
        assert!(matches!(err, DriverError::Controller(ref m) if m.starts_with("error")));
    }

    #[test]
    fn reports_status() {
        let mut sim = SimulatedGrbl::new();
        sim.pos = [7.0, 8.0, 0.0];
        let mut client = GrblClient::new(&mut sim);
        client.request_status().unwrap();
        let status = client.read_status().unwrap().expect("status");
        assert!(status.contains("MPos:7.000,8.000"));
    }

    /// A bridge that drops realtime bytes and only answers newline-terminated
    /// commands, like some line-buffered Telnet servers.
    #[derive(Default)]
    struct LineBuffered {
        queue: std::collections::VecDeque<String>,
    }

    impl Transport for LineBuffered {
        fn write_line(&mut self, line: &str) -> io::Result<()> {
            if line == "?" {
                self.queue
                    .push_back("<Idle|MPos:1.000,2.000,0.000>".to_owned());
            }
            Ok(())
        }

        fn write_raw(&mut self, _bytes: &[u8]) -> io::Result<()> {
            Ok(())
        }

        fn read_line(&mut self) -> io::Result<Option<String>> {
            Ok(self.queue.pop_front())
        }
    }

    #[test]
    fn probe_falls_back_to_line_mode_on_buffered_bridges() {
        let mut client = GrblClient::new(LineBuffered::default());
        let probe = client
            .probe_status(Duration::from_millis(600))
            .expect("probe");
        assert!(probe.status.is_some(), "seen: {:?}", probe.seen);
    }

    #[test]
    fn probe_times_out_on_a_silent_link() {
        struct Mute;
        impl Transport for Mute {
            fn write_line(&mut self, _line: &str) -> io::Result<()> {
                Ok(())
            }
            fn write_raw(&mut self, _bytes: &[u8]) -> io::Result<()> {
                Ok(())
            }
            fn read_line(&mut self) -> io::Result<Option<String>> {
                Ok(None)
            }
        }

        let mut client = GrblClient::new(Mute);
        let probe = client
            .probe_status(Duration::from_millis(80))
            .expect("probe");
        assert!(probe.status.is_none());
    }

    #[test]
    fn probe_keeps_unexpected_lines_for_diagnostics() {
        #[derive(Default)]
        struct Noisy {
            sent: bool,
        }
        impl Transport for Noisy {
            fn write_line(&mut self, _line: &str) -> io::Result<()> {
                Ok(())
            }
            fn write_raw(&mut self, _bytes: &[u8]) -> io::Result<()> {
                self.sent = true;
                Ok(())
            }
            fn read_line(&mut self) -> io::Result<Option<String>> {
                if !self.sent {
                    return Ok(None);
                }
                self.sent = false;
                Ok(Some("error:20".to_owned()))
            }
        }

        let mut client = GrblClient::new(Noisy::default());
        let probe = client
            .probe_status(Duration::from_millis(40))
            .expect("probe");
        assert!(probe.status.is_none());
        assert!(probe.seen.iter().any(|line| line.contains("error:20")));
    }

    #[test]
    fn feed_hold_stops_the_simulator() {
        let mut sim = SimulatedGrbl::new();
        {
            let mut client = GrblClient::new(&mut sim);
            client.feed_hold().unwrap();
            assert_eq!(client.transport().state, RunState::Hold);
            client.resume().unwrap();
            assert_eq!(client.transport().state, RunState::Run);
        }
    }
}
