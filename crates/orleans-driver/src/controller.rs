//! Driver selection and the common controller surface used by the CLI and UI.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::grbl::{DriverError, GrblClient, StatusProbe};
use crate::marlin::MarlinClient;
use crate::transport::Transport;

/// Firmware family understood by orleans.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Protocol {
    #[default]
    Grbl,
    Marlin,
}

impl Protocol {
    pub fn label(self) -> &'static str {
        match self {
            Protocol::Grbl => "GRBL",
            Protocol::Marlin => "Marlin",
        }
    }

    /// Default `S` value for 100 % power. Marlin's laser PWM is commonly
    /// configured as 0..=255 even when the G-code uses GRBL-style `S` words.
    pub fn default_max_s(self) -> u32 {
        match self {
            Protocol::Grbl => 1000,
            Protocol::Marlin => 255,
        }
    }

    /// GRBL feed hold is realtime and safe with M4/$32=1. Marlin has no
    /// equivalent safe pause primitive, so the UI offers Stop instead.
    pub fn supports_pause(self) -> bool {
        self == Protocol::Grbl
    }

    /// GRBL can persist a G54 work offset. Marlin's G92 is temporary.
    pub fn supports_saved_origin(self) -> bool {
        self == Protocol::Grbl
    }
}

/// One status poll. `status` uses the GRBL `<...>` shape so the editor can
/// keep a single parser for both drivers.
#[derive(Clone, Debug, Default)]
pub struct StatusPoll {
    pub status: Option<String>,
    pub lines: Vec<String>,
}

/// A detected controller and the evidence gathered while probing it.
pub struct Detection<T: Transport> {
    pub controller: Controller<T>,
    pub status: Option<String>,
    pub seen: Vec<String>,
    pub firmware: Option<String>,
}

/// Blocking controller connection with a common command surface.
pub enum Controller<T: Transport> {
    Grbl(GrblClient<T>),
    Marlin(MarlinClient<T>),
}

impl<T: Transport> Controller<T> {
    pub fn protocol(&self) -> Protocol {
        match self {
            Controller::Grbl(_) => Protocol::Grbl,
            Controller::Marlin(_) => Protocol::Marlin,
        }
    }

    pub fn transport(&self) -> &T {
        match self {
            Controller::Grbl(client) => client.transport(),
            Controller::Marlin(client) => client.transport(),
        }
    }

    pub fn transport_mut(&mut self) -> &mut T {
        match self {
            Controller::Grbl(client) => client.transport_mut(),
            Controller::Marlin(client) => client.transport_mut(),
        }
    }

    pub fn into_transport(self) -> T {
        match self {
            Controller::Grbl(client) => client.into_transport(),
            Controller::Marlin(client) => client.into_transport(),
        }
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        match self {
            Controller::Grbl(client) => client.set_timeout(timeout),
            Controller::Marlin(client) => client.set_timeout(timeout),
        }
    }

    pub fn wake(&mut self) -> Result<(), DriverError> {
        match self {
            Controller::Grbl(client) => client.wake(),
            Controller::Marlin(client) => client.wake(),
        }
    }

    /// Ask for one status snapshot.
    pub fn poll_status(&mut self, timeout: Duration) -> Result<StatusPoll, DriverError> {
        match self {
            Controller::Grbl(client) => {
                let probe = client.probe_status(timeout)?;
                Ok(StatusPoll {
                    status: probe.status,
                    lines: probe.seen,
                })
            }
            Controller::Marlin(client) => {
                let status = client.read_status(timeout)?;
                let line = status.as_grbl_status();
                Ok(StatusPoll {
                    status: line,
                    lines: status.raw,
                })
            }
        }
    }

    pub fn send(&mut self, line: &str) -> Result<(), DriverError> {
        match self {
            Controller::Grbl(client) => client.send(line),
            Controller::Marlin(client) => client.send(line),
        }
    }

    /// Start homing without blocking.
    pub fn home(&mut self) -> Result<(), DriverError> {
        match self {
            Controller::Grbl(client) => client.write_command("$H"),
            Controller::Marlin(client) => client.home(),
        }
    }

    /// Clear an alarm or Marlin's kill state.
    pub fn unlock(&mut self) -> Result<(), DriverError> {
        match self {
            Controller::Grbl(client) => client.write_command("$X"),
            Controller::Marlin(client) => client.unlock(),
        }
    }

    /// Stop motion and switch the laser off.
    pub fn stop(&mut self) -> Result<(), DriverError> {
        match self {
            Controller::Grbl(client) => client.stop(),
            Controller::Marlin(client) => client.stop(),
        }
    }

    pub fn jog(&mut self, dx: f64, dy: f64, feed: f64) -> Result<(), DriverError> {
        match self {
            Controller::Grbl(client) => {
                client.write_command(&format!("$J=G91 G21 X{dx:.3} Y{dy:.3} F{feed:.0}"))
            }
            Controller::Marlin(client) => client.jog(dx, dy, feed),
        }
    }

    pub fn jog_to(&mut self, x: f64, y: f64, feed: f64) -> Result<(), DriverError> {
        match self {
            Controller::Grbl(client) => {
                client.write_command(&format!("$J=G90 G21 X{x:.3} Y{y:.3} F{feed:.0}"))
            }
            Controller::Marlin(client) => client.jog_to(x, y, feed),
        }
    }

    pub fn jog_cancel(&mut self) -> Result<(), DriverError> {
        match self {
            Controller::Grbl(client) => client.jog_cancel(),
            Controller::Marlin(client) => client.jog_cancel(),
        }
    }

    pub fn set_origin(&mut self) -> Result<(), DriverError> {
        match self {
            Controller::Grbl(client) => client.write_command("G92 X0 Y0"),
            Controller::Marlin(client) => client.set_origin(),
        }
    }

    pub fn save_origin(&mut self) -> Result<(), DriverError> {
        match self {
            Controller::Grbl(client) => client.write_command("G10 L20 P1 X0 Y0"),
            Controller::Marlin(_) => Err(DriverError::Unsupported("saved work origin")),
        }
    }

    pub fn run_pausable<F>(
        &mut self,
        gcode: &str,
        progress: F,
        stop: &AtomicBool,
        pause: &AtomicBool,
    ) -> Result<usize, DriverError>
    where
        F: FnMut(usize, usize),
    {
        match self {
            Controller::Grbl(client) => client.run_pausable(gcode, progress, stop, pause),
            Controller::Marlin(client) => client.run_pausable(gcode, progress, stop, pause),
        }
    }
}

impl<T: Transport> Controller<T> {
    /// Probe for GRBL first, then ask for a Marlin identity. The first driver
    /// that answers wins. The returned evidence is useful for diagnostics when
    /// neither protocol is recognised.
    pub fn detect(
        transport: T,
        grbl_timeout: Duration,
        marlin_timeout: Duration,
    ) -> Result<Detection<T>, String> {
        let mut grbl = GrblClient::new(transport);
        let probe: StatusProbe = grbl
            .probe_status(grbl_timeout)
            .map_err(|err| format!("probing GRBL: {err}"))?;
        if let Some(status) = probe.status {
            return Ok(Detection {
                controller: Controller::Grbl(grbl),
                status: Some(status),
                seen: probe.seen,
                firmware: None,
            });
        }

        let mut seen = probe.seen;
        let mut marlin = MarlinClient::new(grbl.into_transport());
        let identity = marlin
            .probe_identity(marlin_timeout)
            .map_err(|err| format!("probing Marlin: {err}"))?;
        seen.extend(identity.lines.iter().cloned());
        if identity.is_marlin {
            let status = marlin
                .read_status(Duration::from_millis(750))
                .ok()
                .and_then(|status| status.as_grbl_status());
            return Ok(Detection {
                controller: Controller::Marlin(marlin),
                status,
                seen,
                firmware: identity.firmware,
            });
        }

        Err(format!(
            "no GRBL status report and no Marlin identity; received: {}",
            if seen.is_empty() {
                "nothing".to_owned()
            } else {
                seen.iter()
                    .take(12)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" | ")
            }
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{SimulatedGrbl, SimulatedMarlin};

    #[test]
    fn detects_grbl_by_status_report() {
        let detection = Controller::detect(
            SimulatedGrbl::new(),
            Duration::from_millis(200),
            Duration::from_millis(200),
        )
        .expect("detect GRBL");
        assert_eq!(detection.controller.protocol(), Protocol::Grbl);
        assert!(detection.status.is_some());
        assert!(detection.firmware.is_none());
    }

    #[test]
    fn detects_marlin_by_m115_identity() {
        let detection = Controller::detect(
            SimulatedMarlin::new(),
            Duration::from_millis(120),
            Duration::from_millis(200),
        )
        .expect("detect Marlin");
        assert_eq!(detection.controller.protocol(), Protocol::Marlin);
        assert!(detection
            .firmware
            .as_deref()
            .is_some_and(|line| line.contains("Marlin")));
        assert!(detection
            .status
            .as_deref()
            .is_some_and(|line| line.starts_with("<Idle|WPos:")));
    }

    #[test]
    fn marlin_has_no_saved_work_origin() {
        let mut controller = Controller::Marlin(MarlinClient::new(SimulatedMarlin::new()));
        let error = controller.save_origin().unwrap_err();
        assert!(matches!(
            error,
            DriverError::Unsupported("saved work origin")
        ));
    }

    #[test]
    fn protocol_defaults_are_conservative() {
        assert_eq!(Protocol::Grbl.default_max_s(), 1000);
        assert_eq!(Protocol::Marlin.default_max_s(), 255);
        assert!(Protocol::Grbl.supports_pause());
        assert!(!Protocol::Marlin.supports_pause());
    }
}
