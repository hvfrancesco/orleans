//! Byte transport abstraction.
//!
//! The streaming state machine must not care whether the controller is on a
//! USB serial port, a TCP socket, or a simulator. Everything it needs is
//! "write a line" plus "read whatever is available".

use std::io;

/// A bidirectional, line-oriented link to a controller.
pub trait Transport {
    /// Write one command, without the trailing newline (the transport adds it).
    fn write_line(&mut self, line: &str) -> io::Result<()>;

    /// Write raw bytes. Used for GRBL's single-byte realtime commands
    /// (`?`, `!`, `~`, Ctrl-X).
    fn write_raw(&mut self, bytes: &[u8]) -> io::Result<()>;

    /// Read one line if one is available. `Ok(None)` means "nothing yet",
    /// which the caller treats as a poll. It must not block indefinitely.
    fn read_line(&mut self) -> io::Result<Option<String>>;
}

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        (**self).write_line(line)
    }

    fn write_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        (**self).write_raw(bytes)
    }

    fn read_line(&mut self) -> io::Result<Option<String>> {
        (**self).read_line()
    }
}

impl<T: Transport + ?Sized> Transport for &mut T {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        (**self).write_line(line)
    }

    fn write_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        (**self).write_raw(bytes)
    }

    fn read_line(&mut self) -> io::Result<Option<String>> {
        (**self).read_line()
    }
}

/// A real USB/ACM serial link, available with the `serial` feature.
#[cfg(feature = "serial")]
pub struct SerialTransport {
    port: Box<dyn serialport::SerialPort>,
    buffer: Vec<u8>,
}

#[cfg(feature = "serial")]
impl SerialTransport {
    /// Open e.g. `/dev/ttyUSB0` at 115200 baud, the usual GRBL setting.
    pub fn open(path: &str, baud: u32) -> Result<Self, serialport::Error> {
        let port = serialport::new(path, baud)
            .timeout(std::time::Duration::from_millis(20))
            .open()?;
        Ok(Self {
            port,
            buffer: Vec::with_capacity(128),
        })
    }
}

#[cfg(feature = "serial")]
impl Transport for SerialTransport {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        self.port.write_all(line.as_bytes())?;
        self.port.write_all(b"\n")?;
        self.port.flush()
    }

    fn write_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.port.write_all(bytes)?;
        self.port.flush()
    }

    fn read_line(&mut self) -> io::Result<Option<String>> {
        let mut byte = [0u8; 1];
        loop {
            match self.port.read(&mut byte) {
                Ok(0) => return Ok(None),
                Ok(_) => {
                    if byte[0] == b'\n' {
                        let line = String::from_utf8_lossy(&self.buffer)
                            .trim_end_matches('\r')
                            .to_owned();
                        self.buffer.clear();
                        return Ok(Some(line));
                    }
                    self.buffer.push(byte[0]);
                }
                Err(err) if err.kind() == io::ErrorKind::TimedOut => return Ok(None),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        }
    }
}
