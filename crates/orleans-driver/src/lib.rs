//! Controller drivers for laser cutters.
//!
//! Currently: GRBL (and GrblHAL/FluidNC/Marlin-laser) over serial, with a
//! simulator for development. Ruida/TopWisdom and galvo heads are future work;
//! they only need to implement [`Transport`] and a matching streamer.

pub mod gcode;
pub mod grbl;
pub mod mock;
pub mod transport;

pub use gcode::{GcodeOptions, LaserMode};
pub use grbl::{DriverError, GrblClient, GrblConfig};
pub use mock::{RunState, SimulatedGrbl};
pub use transport::Transport;

#[cfg(feature = "serial")]
pub use transport::SerialTransport;

/// Open a serial link, returning a boxed transport so callers do not need the
/// feature flag in their signatures.
#[cfg(feature = "serial")]
pub fn open_serial(path: &str, baud: u32) -> Result<SerialTransport, serialport::Error> {
    SerialTransport::open(path, baud)
}
