//! Network transport: GRBL over raw TCP, as exposed by Grbl_ESP32, FluidNC and
//! ESP3D on port 23 (Telnet).
//!
//! A Telnet connection is a byte stream, not a line-oriented protocol, so the
//! transport keeps its own buffer, handles partial reads and strips Telnet IAC
//! negotiation sequences (`0xFF …`) that some WiFi bridges emit on connect.
//! GRBL's own traffic is plain ASCII.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::transport::Transport;

/// Default Telnet port used by GRBL WiFi bridges.
pub const TELNET_PORT: u16 = 23;

/// Parse `tcp://host[:port]`, `telnet://host[:port]`, or a bare `host[:port]`.
/// Telnet's port 23 is the default.
pub fn parse_endpoint(input: &str) -> Result<(String, u16), String> {
    let raw = input.trim();
    let raw = raw
        .strip_prefix("tcp://")
        .or_else(|| raw.strip_prefix("telnet://"))
        .unwrap_or(raw)
        .trim_end_matches('/');
    if raw.is_empty() {
        return Err("empty network address".to_owned());
    }
    if let Some((host, port)) = raw.rsplit_once(':') {
        let port: u16 = port
            .parse()
            .map_err(|_| format!("invalid port in {input:?}"))?;
        if host.is_empty() {
            return Err(format!("missing host in {input:?}"));
        }
        return Ok((host.to_owned(), port));
    }
    Ok((raw.to_owned(), TELNET_PORT))
}

pub struct TcpTransport {
    stream: TcpStream,
    /// Bytes received but not yet terminated by a newline.
    line: Vec<u8>,
    filter: TelnetFilter,
}

impl TcpTransport {
    /// Connect to `host:port`, trying every resolved address.
    pub fn connect(host: &str, port: u16, timeout: Duration) -> io::Result<Self> {
        let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs()?.collect();
        if addrs.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{host}:{port} did not resolve"),
            ));
        }

        let mut last_err = None;
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, timeout) {
                Ok(stream) => {
                    // A short read timeout gives us a poll-like `read_line`
                    // without spawning a reader thread.
                    stream.set_read_timeout(Some(Duration::from_millis(20)))?;
                    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
                    stream.set_nodelay(true)?;
                    return Ok(Self {
                        stream,
                        line: Vec::with_capacity(256),
                        filter: TelnetFilter::default(),
                    });
                }
                Err(err) => last_err = Some(err),
            }
        }
        Err(last_err.expect("at least one address"))
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.stream.peer_addr()
    }
}

impl Transport for TcpTransport {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        self.stream.write_all(line.as_bytes())?;
        self.stream.write_all(b"\n")?;
        self.stream.flush()
    }

    fn write_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.stream.write_all(bytes)?;
        self.stream.flush()
    }

    fn read_line(&mut self) -> io::Result<Option<String>> {
        loop {
            if let Some(pos) = self.line.iter().position(|b| *b == b'\n') {
                let raw: Vec<u8> = self.line.drain(..=pos).collect();
                let text = String::from_utf8_lossy(&raw[..raw.len() - 1])
                    .trim_end_matches('\r')
                    .trim()
                    .to_owned();
                if !text.is_empty() {
                    return Ok(Some(text));
                }
                continue;
            }

            let mut chunk = [0u8; 512];
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "controller closed the connection",
                    ))
                }
                Ok(n) => {
                    let mut clean = Vec::with_capacity(n);
                    self.filter.feed(&chunk[..n], &mut clean);
                    self.line.extend_from_slice(&clean);
                }
                Err(err)
                    if err.kind() == io::ErrorKind::WouldBlock
                        || err.kind() == io::ErrorKind::TimedOut =>
                {
                    return Ok(None);
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        }
    }
}

/// Removes Telnet negotiation from a byte stream, statefully (a sequence can
/// be split across two reads).
#[derive(Default)]
struct TelnetFilter {
    state: FilterState,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum FilterState {
    #[default]
    Data,
    Iac,
    /// Waiting for the option byte after WILL/WONT/DO/DONT.
    Option,
}

impl TelnetFilter {
    fn feed(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        for &byte in bytes {
            match self.state {
                FilterState::Data => {
                    if byte == 0xFF {
                        self.state = FilterState::Iac;
                    } else if byte != b'\r' && byte != 0 {
                        out.push(byte);
                    }
                }
                FilterState::Iac => {
                    self.state = if (251..=254).contains(&byte) {
                        FilterState::Option
                    } else {
                        FilterState::Data
                    };
                }
                FilterState::Option => self.state = FilterState::Data,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grbl::GrblClient;
    use crate::mock::SimulatedGrbl;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn telnet_filter_strips_negotiation() {
        let mut filter = TelnetFilter::default();
        let mut out = Vec::new();
        // IAC WILL ECHO, "ok\r\n", IAC DONT LINEMODE, "Idle".
        filter.feed(&[0xFF, 251, 1, b'o', b'k', b'\r', b'\n'], &mut out);
        filter.feed(&[0xFF, 254, 1, b'I', b'd', b'l', b'e'], &mut out);
        assert_eq!(out, b"ok\nIdle");
    }

    #[test]
    fn parses_endpoints() {
        assert_eq!(
            parse_endpoint("tcp://192.168.1.50:23").unwrap(),
            ("192.168.1.50".to_owned(), 23)
        );
        assert_eq!(
            parse_endpoint("192.168.1.50").unwrap(),
            ("192.168.1.50".to_owned(), TELNET_PORT)
        );
        assert_eq!(
            parse_endpoint("telnet://laser.local:2323/").unwrap(),
            ("laser.local".to_owned(), 2323)
        );
        assert!(parse_endpoint("tcp://host:not-a-port").is_err());
        assert!(parse_endpoint("tcp://:23").is_err());
        assert!(parse_endpoint("  ").is_err());
    }

    #[test]
    fn telnet_filter_handles_split_sequences() {
        let mut filter = TelnetFilter::default();
        let mut out = Vec::new();
        filter.feed(&[0xFF], &mut out);
        filter.feed(&[251], &mut out);
        filter.feed(&[1, b'o', b'k'], &mut out);
        assert_eq!(out, b"ok");
    }

    fn spawn_server() -> (u16, mpsc::Receiver<[f64; 3]>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut writer = stream;
            let mut sim = SimulatedGrbl::new();
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
                let cmd = line.trim_end_matches(['\r', '\n']);
                if cmd.is_empty() {
                    continue;
                }
                if sim.write_line(cmd).is_err() {
                    break;
                }
                while let Some(resp) = sim.read_line().expect("response") {
                    writeln!(writer, "{resp}").expect("write");
                }
                writer.flush().expect("flush");
            }
            let _ = tx.send(sim.pos);
        });
        (port, rx, handle)
    }

    #[test]
    fn streams_over_tcp_to_a_local_controller() {
        let (port, rx, handle) = spawn_server();
        let transport =
            TcpTransport::connect("127.0.0.1", port, Duration::from_secs(2)).expect("connect");
        let mut client = GrblClient::new(transport);
        let acks = client
            .run("G21\nG90\nG0 X10 Y20\nM3 S500\nG1 X30 Y20 F1000\nM5\n")
            .expect("run");
        assert_eq!(acks, 6);
        drop(client);

        let pos = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("final position");
        handle.join().expect("server");
        assert_eq!(pos, [30.0, 20.0, 0.0]);
    }
}
