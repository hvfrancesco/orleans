//! Simulated GRBL and Marlin controllers.
//!
//! They speak just enough of each protocol to exercise the streaming clients,
//! the G-code writer and the planner without hardware: `ok` for every line,
//! `error` for garbage, GRBL `?` status reports and realtime controls, and
//! Marlin `M114`/`M105`/`M400`/`M112` behaviour. The `orleans run --mock`
//! command uses them, and so do the driver tests.

use std::collections::VecDeque;
use std::io;

use crate::transport::Transport;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Idle,
    Run,
    Hold,
    Alarm,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            RunState::Idle => "Idle",
            RunState::Run => "Run",
            RunState::Hold => "Hold",
            RunState::Alarm => "Alarm",
        }
    }
}

/// In-memory GRBL lookalike.
#[derive(Debug)]
pub struct SimulatedGrbl {
    pending: VecDeque<String>,
    pub state: RunState,
    /// Machine position, millimetres.
    pub pos: [f64; 3],
    /// Current `S` value.
    pub power: u32,
    /// Current feed rate.
    pub feed: f64,
    /// Every line the controller received, for debugging.
    pub received: Vec<String>,
    laser_on: bool,
}

impl Default for SimulatedGrbl {
    fn default() -> Self {
        Self {
            pending: VecDeque::new(),
            state: RunState::Idle,
            pos: [0.0; 3],
            power: 0,
            feed: 0.0,
            received: Vec::new(),
            laser_on: false,
        }
    }
}

impl SimulatedGrbl {
    pub fn new() -> Self {
        Self::default()
    }

    /// GRBL's status report, as requested with `?`.
    pub fn status_line(&self) -> String {
        format!(
            "<{}|MPos:{:.3},{:.3},{:.3}|FS:{:.0},{}>",
            self.state.as_str(),
            self.pos[0],
            self.pos[1],
            self.pos[2],
            self.feed,
            self.power
        )
    }

    fn reply(&mut self, line: impl Into<String>) {
        self.pending.push_back(line.into());
    }

    fn execute(&mut self, line: &str) {
        let upper = line.trim().to_ascii_uppercase();
        if upper == "INVALID" {
            self.reply("error:20");
            return;
        }

        let words = words(&upper);
        let command = words.first().copied();
        match command {
            Some(('G', 0.0)) | Some(('G', 1.0)) => {
                let x = word_value(&words, 'X');
                let y = word_value(&words, 'Y');
                let z = word_value(&words, 'Z');
                let f = word_value(&words, 'F');
                if let Some(x) = x {
                    self.pos[0] = x;
                }
                if let Some(y) = y {
                    self.pos[1] = y;
                }
                if let Some(z) = z {
                    self.pos[2] = z;
                }
                if let Some(f) = f {
                    self.feed = f;
                }
                self.state = if self.laser_on && command == Some(('G', 1.0)) {
                    RunState::Run
                } else {
                    RunState::Idle
                };
                self.reply("ok");
            }
            Some(('M', 3.0)) | Some(('M', 4.0)) => {
                if let Some(s) = word_value(&words, 'S') {
                    self.power = s.max(0.0) as u32;
                }
                self.laser_on = true;
                self.reply("ok");
            }
            Some(('M', 5.0)) => {
                self.laser_on = false;
                self.power = 0;
                self.state = RunState::Idle;
                self.reply("ok");
            }
            Some(('G', 4.0)) | Some(('G', 21.0)) | Some(('G', 90.0)) => self.reply("ok"),
            // `$` settings, `$H`, `$X`, `G28`, `G10` … are accepted as no-ops.
            Some((c, _)) if c == 'G' || c == 'M' || c == '$' => {
                if c == '$' && upper.starts_with("$H") {
                    self.pos = [0.0; 3];
                    self.state = RunState::Idle;
                }
                self.reply("ok");
            }
            _ => self.reply("error:20"),
        }
    }
}

impl Transport for SimulatedGrbl {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            return Ok(());
        }
        self.received.push(line.to_owned());
        self.execute(line);
        Ok(())
    }

    fn write_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        match bytes {
            b"?" => {
                let status = self.status_line();
                self.reply(status);
            }
            b"!" => self.state = RunState::Hold,
            b"~" => {
                if self.state == RunState::Hold {
                    self.state = RunState::Run;
                }
            }
            b"\x18" => {
                self.laser_on = false;
                self.power = 0;
                self.state = RunState::Idle;
                self.reply("Grbl 1.1h ['$' for help]");
            }
            _ => {}
        }
        Ok(())
    }

    fn read_line(&mut self) -> io::Result<Option<String>> {
        Ok(self.pending.pop_front())
    }
}

fn words(line: &str) -> Vec<(char, f64)> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if !c.is_ascii_alphabetic() {
            continue;
        }
        let mut num = String::new();
        while let Some(&n) = chars.peek() {
            if n.is_ascii_digit() || n == '.' || n == '-' || n == '+' {
                num.push(n);
                chars.next();
            } else {
                break;
            }
        }
        let value = num.parse::<f64>().unwrap_or(f64::NAN);
        out.push((c, value));
    }
    out
}

fn word_value(words: &[(char, f64)], letter: char) -> Option<f64> {
    words
        .iter()
        .find(|(c, v)| *c == letter && v.is_finite())
        .map(|(_, v)| *v)
}

/// In-memory Marlin lookalike.
///
/// It reuses the GRBL motion model but answers the commands a Marlin laser
/// actually uses: `M115` identity, `M114` position, `M105` temperatures,
/// `M400` planner drain and `M112`/`M999` emergency stop/restart.
#[derive(Debug)]
pub struct SimulatedMarlin {
    inner: SimulatedGrbl,
    killed: bool,
}

impl Default for SimulatedMarlin {
    fn default() -> Self {
        Self {
            inner: SimulatedGrbl::new(),
            killed: false,
        }
    }
}

impl SimulatedMarlin {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn position(&self) -> [f64; 3] {
        self.inner.pos
    }

    pub fn set_position(&mut self, position: [f64; 3]) {
        self.inner.pos = position;
    }

    pub fn laser_on(&self) -> bool {
        self.inner.laser_on
    }

    pub fn killed(&self) -> bool {
        self.killed
    }

    pub fn received(&self) -> &[String] {
        &self.inner.received
    }

    fn command(&mut self, upper: &str) {
        if self.killed && upper != "M999" && upper != "M115" {
            self.inner.reply("Error:Printer halted. kill() called!");
            return;
        }
        match upper {
            "M115" => {
                self.inner.reply(
                    "FIRMWARE_NAME:Marlin 2.1.2.1 (simulator) SOURCE_CODE_URL:https://marlinfw.org PROTOCOL_VERSION:1.0 MACHINE_TYPE:Laser EXTRUDER_COUNT:1 UUID:simulated",
                );
                self.inner.reply("Cap:SERIAL_XON_XOFF:0");
                self.inner.reply("ok");
            }
            "M114" => {
                self.inner.reply(format!(
                    "X:{:.3} Y:{:.3} Z:{:.3} E:0.000 Count X:0 Y:0 Z:0",
                    self.inner.pos[0], self.inner.pos[1], self.inner.pos[2]
                ));
                self.inner.reply("ok");
            }
            "M105" => {
                self.inner.reply("T:0.00 /0.00 B:0.00 /0.00 @:0 B@:0");
                self.inner.reply("ok");
            }
            "M400" => self.inner.reply("ok"),
            "M112" => {
                self.killed = true;
                self.inner.laser_on = false;
                self.inner.power = 0;
                self.inner.reply("Error:Printer halted. kill() called!");
            }
            "M999" => {
                self.killed = false;
                self.inner.state = RunState::Idle;
                self.inner.reply("ok");
            }
            "G28" => {
                self.inner.pos = [0.0; 3];
                self.inner.state = RunState::Idle;
                self.inner.reply("ok");
            }
            _ => self.inner.execute(upper),
        }
    }
}

impl Transport for SimulatedMarlin {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            return Ok(());
        }
        self.inner.received.push(line.to_owned());
        let upper = line.to_ascii_uppercase();
        self.command(&upper);
        Ok(())
    }

    fn write_raw(&mut self, _bytes: &[u8]) -> io::Result<()> {
        Ok(())
    }

    fn read_line(&mut self) -> io::Result<Option<String>> {
        self.inner.read_line()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acknowledges_moves() {
        let mut sim = SimulatedGrbl::new();
        sim.write_line("G0 X10 Y5").unwrap();
        assert_eq!(sim.pos[0], 10.0);
        assert_eq!(sim.pos[1], 5.0);
        assert_eq!(sim.read_line().unwrap().as_deref(), Some("ok"));
    }

    #[test]
    fn tracks_laser_state() {
        let mut sim = SimulatedGrbl::new();
        sim.write_line("M3 S750").unwrap();
        assert_eq!(sim.power, 750);
        sim.write_line("G1 X1 Y1 F1000").unwrap();
        assert_eq!(sim.state, RunState::Run);
        sim.write_line("M5").unwrap();
        assert_eq!(sim.state, RunState::Idle);
        assert_eq!(sim.power, 0);
    }

    #[test]
    fn reports_status() {
        let mut sim = SimulatedGrbl::new();
        sim.pos = [1.0, 2.0, 0.0];
        sim.write_raw(b"?").unwrap();
        let status = sim.read_line().unwrap().unwrap();
        assert!(status.starts_with("<Idle|MPos:1.000,2.000,0.000"));
    }

    #[test]
    fn feed_hold_and_resume() {
        let mut sim = SimulatedGrbl::new();
        sim.state = RunState::Run;
        sim.write_raw(b"!").unwrap();
        assert_eq!(sim.state, RunState::Hold);
        sim.write_raw(b"~").unwrap();
        assert_eq!(sim.state, RunState::Run);
    }

    #[test]
    fn rejects_garbage() {
        let mut sim = SimulatedGrbl::new();
        sim.write_line("INVALID").unwrap();
        assert_eq!(sim.read_line().unwrap().as_deref(), Some("error:20"));
    }
}
