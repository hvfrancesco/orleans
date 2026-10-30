//! Controller drivers for laser cutters.
//!
//! Currently: GRBL/GrblHAL/FluidNC and Marlin lasers over USB serial or raw TCP
//! (Telnet), with simulators for development. Ruida/TopWisdom and galvo heads
//! are future work; they only need to implement [`Transport`] and a matching
//! streamer.

pub mod controller;
pub mod gcode;
pub mod grbl;
pub mod marlin;
pub mod mock;
pub mod net;
pub mod transport;

pub use controller::{Controller, Detection, Protocol, StatusPoll};
pub use gcode::{GcodeDialect, GcodeOptions, LaserMode};
pub use grbl::{DriverError, GrblClient, GrblConfig};
pub use marlin::{MarlinClient, MarlinConfig, MarlinStatus};
pub use mock::{RunState, SimulatedGrbl, SimulatedMarlin};
pub use net::{TcpTransport, TELNET_PORT};
pub use transport::Transport;

#[cfg(feature = "serial")]
pub use transport::SerialTransport;

/// Open a serial link, returning a boxed transport so callers do not need the
/// feature flag in their signatures.
#[cfg(feature = "serial")]
pub fn open_serial(path: &str, baud: u32) -> Result<SerialTransport, serialport::Error> {
    SerialTransport::open(path, baud)
}
