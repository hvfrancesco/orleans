//! WebSocket transport for ESP3D / FluidNC web bridges.
//!
//! ESP3D's terminal WebSocket uses text frames with line discipline on
//! `/ws` and the `webui-v3` subprotocol. This transport presents that link as
//! the same line-oriented [`Transport`] used by the GRBL and Marlin clients.

use std::collections::VecDeque;
use std::io;
use std::net::TcpStream;
use std::time::Duration;

use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message, WebSocket};

use crate::transport::Transport;

pub struct WebSocketTransport {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    pending: VecDeque<String>,
    partial: Vec<u8>,
}

impl WebSocketTransport {
    /// Open an ESP3D-compatible WebSocket. A pathless URL defaults to `/ws`.
    pub fn open(url: &str) -> io::Result<Self> {
        if url.starts_with("http://") || url.starts_with("https://") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "use a ws:// URL for the ESP3D terminal bridge",
            ));
        }
        let normalized;
        let url = if url.starts_with("ws://") || url.starts_with("wss://") {
            url
        } else {
            normalized = format!("ws://{url}");
            &normalized
        };
        if url.starts_with("wss://") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "wss:// needs TLS support, which this build does not include",
            ));
        }
        let url = if url == "ws://" {
            url.to_owned()
        } else if url.ends_with('/') {
            format!("{url}ws")
        } else if !url.contains("/ws") {
            format!("{url}/ws")
        } else {
            url.to_owned()
        };
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(io::Error::other)?;
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_static("webui-v3"),
        );
        let (mut socket, _) = connect(request).map_err(io::Error::other)?;
        if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
            stream.set_read_timeout(Some(Duration::from_millis(20)))?;
        }
        Ok(Self {
            socket,
            pending: VecDeque::new(),
            partial: Vec::new(),
        })
    }
}

impl Transport for WebSocketTransport {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        self.socket
            .send(Message::text(format!("{line}\n")))
            .map_err(io::Error::other)
    }

    fn write_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        // ESP3D forwards terminal text only after a line terminator.
        let mut text = String::from_utf8_lossy(bytes).into_owned();
        text.push('\n');
        self.socket
            .send(Message::text(text))
            .map_err(io::Error::other)
    }

    fn read_line(&mut self) -> io::Result<Option<String>> {
        if let Some(line) = self.pending.pop_front() {
            return Ok(Some(line));
        }
        loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    self.partial.extend_from_slice(text.as_bytes());
                    while let Some(end) = self
                        .partial
                        .iter()
                        .position(|byte| *byte == b'\n' || *byte == b'\r')
                    {
                        let rest = self.partial.split_off(end + 1);
                        let mut raw = std::mem::replace(&mut self.partial, rest);
                        raw.truncate(end);
                        let line = String::from_utf8_lossy(&raw).trim().to_owned();
                        if !line.is_empty() {
                            self.pending.push_back(line);
                        }
                    }
                    if let Some(line) = self.pending.pop_front() {
                        return Ok(Some(line));
                    }
                }
                Ok(Message::Binary(data)) => {
                    self.partial.extend_from_slice(&data);
                }
                Ok(Message::Ping(data)) => {
                    let _ = self.socket.send(Message::Pong(data));
                }
                Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
                Ok(Message::Close(_))
                | Err(tungstenite::Error::ConnectionClosed)
                | Err(tungstenite::Error::AlreadyClosed) => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        "WebSocket closed",
                    ))
                }
                Err(tungstenite::Error::Io(err))
                    if err.kind() == io::ErrorKind::WouldBlock
                        || err.kind() == io::ErrorKind::TimedOut =>
                {
                    return Ok(None)
                }
                Err(err) => return Err(io::Error::other(err)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;
    use std::time::Instant;

    #[test]
    #[allow(clippy::result_large_err)]
    fn exchanges_line_commands() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = thread::spawn(move || {
            use tungstenite::handshake::server::{ErrorResponse, Request, Response};
            let (stream, _) = listener.accept().expect("accept");
            let callback = |_request: &Request, mut response: Response| {
                response.headers_mut().insert(
                    "Sec-WebSocket-Protocol",
                    HeaderValue::from_static("webui-v3"),
                );
                Ok::<Response, ErrorResponse>(response)
            };
            let mut socket = tungstenite::accept_hdr(stream, callback).expect("handshake");
            for _ in 0..2 {
                if let Ok(Message::Text(text)) = socket.read() {
                    if text.contains("G0 X1") {
                        socket
                            .send(Message::text("ok\n<Idle|MPos:0,0,0>\n"))
                            .expect("send");
                    }
                }
            }
        });

        let mut transport =
            WebSocketTransport::open(&format!("ws://127.0.0.1:{port}")).expect("open");
        transport.write_line("G0 X1").expect("write");
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut first = None;
        while Instant::now() < deadline && first.is_none() {
            first = transport.read_line().expect("read");
        }
        assert_eq!(first.as_deref(), Some("ok"));
        assert_eq!(
            transport.read_line().expect("read"),
            Some("<Idle|MPos:0,0,0>".to_owned())
        );
        transport.write_line("M5").expect("write");
        server.join().expect("server");
    }
}
