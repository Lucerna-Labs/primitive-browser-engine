//! # pbe-proto-ws
//!
//! The **WebSocket protocol crate** (RFC 6455). One modern protocol, one crate,
//! independently swappable: the [`pbe_proto`] dispatch layer routes `ws`/`wss`
//! URLs here.
//!
//! ## Architecture
//!
//! WebSocket is two concerns under one protocol:
//!
//! 1. **The handshake** — an HTTP `Upgrade` request that establishes the
//!    connection. [`handshake`] drives the sealed system HTTP client with
//!    `--include` and returns just the upgrade response (the legacy
//!    one-shot entry point). [`WsConnection::connect`] performs the same
//!    handshake **in-process** over a real socket, then keeps the socket
//!    open for the frame exchange.
//! 2. **The frame codec** — once upgraded, messages are RFC 6455 frames
//!    (opcode + masked payload). [`encode_frame`] (client-to-server, always
//!    masked) and [`decode_frame`] (server-to-client, never masked) are pure
//!    functions, used both by [`WsConnection`] and standalone.
//!
//! ## Persistent connections — [`WsConnection`]
//!
//! [`WsConnection::connect`] opens a real TCP connection to the WebSocket
//! endpoint (TLS via `rustls` for `wss://`, raw for `ws://`), performs the
//! client opening handshake over that socket, and returns a connection you
//! can [`send`](WsConnection::send) and [`recv`](WsConnection::recv) text or
//! binary frames over until [`close`](WsConnection::close).
//!
//! This links a TLS stack (`rustls` with the `ring` provider + the
//! `webpki-roots` Mozilla trust store) into the engine — a deliberate
//! exception to the "drive a sealed binary, link nothing" posture HTTP still
//! keeps, because WebSocket is a *persistent* protocol: the sealed-binary
//! approach returns the handshake response and closes the data channel, so
//! it cannot carry the bidirectional frame stream a live connection needs.
//! HTTP, which is request/response over a fresh connection each time, has no
//! such need and stays sealed-binary-driven. `ring` (pure-Rust crypto) is
//! chosen over `aws-lc-rs` to avoid pulling a C build chain into the engine.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Command;
use std::time::Duration;

pub use cap_http::HttpError as WsError;

const TIMEOUT_SECS: u32 = 20;

/// A completed WebSocket handshake — the one-shot entry's concrete, immutable
/// output (drives the sealed HTTP client). For a live connection, use
/// [`WsConnection::connect`] instead. The dispatch layer's [`Resource`] is
/// built `From` this.
#[derive(Clone, Debug)]
pub struct Handshake {
    /// The upgraded endpoint URL (after any `wss://` TLS resolution).
    pub final_url: String,
    /// HTTP status of the upgrade response (101 on success).
    pub status: u16,
    /// The `Sec-WebSocket-Accept` header value the server returned, if any.
    pub accept: Option<String>,
    /// The raw handshake response bytes (headers + blank line).
    pub body: Vec<u8>,
}

/// Perform the WebSocket opening handshake for a `ws://`/`wss://` URL by
/// driving the sealed system HTTP client with `--include` (so the raw HTTP
/// upgrade response is captured). A client key is generated; the response's
/// `Sec-WebSocket-Accept` is returned for verification.
///
/// This is the **one-shot** entry point: it returns the handshake response
/// and the connection is then closed by the sealed client. For a persistent
/// connection you can send/recv frames over, use [`WsConnection::connect`].
///
/// Security posture: the sealed client performs the TLS for `wss://` itself.
/// Only `ws`/`wss` schemes are accepted; anything else is rejected before a
/// process is spawned.
pub fn handshake(url: &str) -> Result<Handshake, WsError> {
    let scheme_ok = url.starts_with("ws://") || url.starts_with("wss://");
    if !scheme_ok {
        return Err(WsError::Connection(format!(
            "refusing non-ws(s) URL: {url}"
        )));
    }

    // The sealed client speaks http(s) natively, so map ws->http, wss->https
    // for the handshake transport. The Upgrade headers below turn the HTTP
    // request into a WebSocket one.
    let http_url = if let Some(rest) = url.strip_prefix("ws://") {
        format!("http://{rest}")
    } else if let Some(rest) = url.strip_prefix("wss://") {
        format!("https://{rest}")
    } else {
        return Err(WsError::Connection(format!(
            "refusing non-ws(s) URL: {url}"
        )));
    };

    let key = generate_key();
    let output = Command::new("curl")
        .arg("--silent")
        .arg("--show-error")
        .arg("--include") // emit headers + body
        .arg("--max-time")
        .arg(TIMEOUT_SECS.to_string())
        .arg("--proto")
        .arg("=http,https")
        .arg("--user-agent")
        .arg("pbe/0.1 (primitive browser engine)")
        .arg("--header")
        .arg("Upgrade: websocket")
        .arg("--header")
        .arg("Connection: Upgrade")
        .arg("--header")
        .arg(format!("Sec-WebSocket-Key: {key}"))
        .arg("--header")
        .arg("Sec-WebSocket-Version: 13")
        .arg("--")
        .arg(&http_url)
        .output()
        .map_err(|e| WsError::Connection(format!("could not run curl: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WsError::Connection(format!(
            "curl failed ({}): {}",
            output.status,
            stderr.trim()
        )));
    }

    // --include gives "HTTP/1.1 101 ...\r\nheaders\r\n\r\n[body]". Parse the
    // status line + the Sec-WebSocket-Accept header.
    let raw = output.stdout;
    let (status, accept) = parse_handshake_response(&raw);
    Ok(Handshake {
        final_url: url.to_string(),
        status,
        accept,
        body: raw,
    })
}

/// A live, persistent WebSocket connection. Created with
/// [`connect`](WsConnection::connect); send and recv text/binary frames
/// until [`close`](WsConnection::close).
///
/// The underlying transport is a TCP socket (raw for `ws://`, TLS for
/// `wss://` via `rustls`). Frames are encoded/decoded with the crate's
/// pure-Rust codec ([`encode_frame`]/[`decode_frame`]).
pub struct WsConnection {
    stream: Box<dyn Stream>,
    /// Buffer of bytes already read from the socket but not yet consumed by
    /// a complete frame (decode is incremental).
    read_buf: Vec<u8>,
}

/// The byte transport a [`WsConnection`] reads and writes over. Implemented
/// for raw TCP (`ws://`) and rustls TLS streams (`wss://`). A trait so the
/// connection logic is transport-agnostic and a test can substitute a
/// loopback pair.
pub trait Stream: Read + Write + Send {
    /// Set the read/write timeout for the underlying transport.
    fn set_timeout(&mut self, dur: Option<Duration>);
}

impl Stream for TcpStream {
    fn set_timeout(&mut self, dur: Option<Duration>) {
        let _ = TcpStream::set_read_timeout(self, dur);
        let _ = TcpStream::set_write_timeout(self, dur);
    }
}

/// Open a persistent WebSocket connection to a `ws://` or `wss://` URL.
///
/// Performs: DNS + TCP connect → optional TLS (rustls, `wss://`) → client
/// opening handshake over the socket (write the `Upgrade` request, read the
/// `101` response, verify `Sec-WebSocket-Accept`) → return a connection.
///
/// The returned connection is ready to [`send`](WsConnection::send) and
/// [`recv`](WsConnection::recv) frames.
pub fn connect(url: &str) -> Result<WsConnection, WsError> {
    let (host, port, use_tls) = parse_ws_url(url)?;
    let tcp = TcpStream::connect((host.as_str(), port))
        .map_err(|e| WsError::Connection(format!("tcp connect to {host}:{port}: {e}")))?;
    tcp.set_read_timeout(Some(Duration::from_secs(TIMEOUT_SECS as u64)))
        .ok();
    tcp.set_write_timeout(Some(Duration::from_secs(TIMEOUT_SECS as u64)))
        .ok();

    let key = generate_key();
    let req = build_handshake_request(&host, port, &key);

    let mut stream: Box<dyn Stream> = if use_tls {
        Box::new(connect_tls(&host, tcp)?)
    } else {
        Box::new(tcp)
    };

    stream
        .write_all(req.as_bytes())
        .map_err(|e| WsError::Connection(format!("write handshake: {e}")))?;

    // Read until "\r\n\r\n" — the end of the HTTP response headers.
    let mut resp = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        let n = stream
            .read(&mut byte)
            .map_err(|e| WsError::Connection(format!("read handshake: {e}")))?;
        if n == 0 {
            return Err(WsError::Connection("server closed during handshake".into()));
        }
        resp.push(byte[0]);
        if resp.len() >= 4 && &resp[resp.len() - 4..] == b"\r\n\r\n" {
            break;
        }
        if resp.len() > 64 * 1024 {
            return Err(WsError::Connection("handshake response too large".into()));
        }
    }

    let (status, accept) = parse_handshake_response(&resp);
    if status != 101 {
        return Err(WsError::Status(status));
    }
    // Verify the accept value matches the key (RFC 6455 §4.2.2).
    let expected = compute_accept(&key);
    if accept.as_deref() != Some(expected.as_str()) {
        return Err(WsError::Tls("Sec-WebSocket-Accept mismatch".into()));
    }

    Ok(WsConnection {
        stream,
        read_buf: Vec::new(),
    })
}

impl WsConnection {
    /// Send a text message over the connection. The payload is encoded as a
    /// single final text frame, masked (client-to-server per RFC 6455 §5.3).
    pub fn send_text(&mut self, payload: &str) -> Result<(), WsError> {
        self.send(Opcode::Text, payload.as_bytes())
    }

    /// Send a binary message over the connection.
    pub fn send_binary(&mut self, payload: &[u8]) -> Result<(), WsError> {
        self.send(Opcode::Binary, payload)
    }

    /// Low-level send: encode a frame with the given opcode + payload and
    /// write it to the socket. A fresh random mask is generated per frame.
    fn send(&mut self, opcode: Opcode, payload: &[u8]) -> Result<(), WsError> {
        let mask = random_mask();
        let frame = encode_frame(opcode, payload, &mask);
        self.stream
            .write_all(&frame)
            .map_err(|e| WsError::Connection(format!("write frame: {e}")))
    }

    /// Receive the next message from the connection. Reads from the socket
    /// until a complete frame is available, then returns its opcode + payload.
    /// Control frames (ping/pong/close) are handled inline: a ping is
    /// answered with a pong, a close is acknowledged and returns `Ok(None)`.
    /// Returns `Ok(Some((opcode, payload)))` for a data or pong frame.
    pub fn recv(&mut self) -> Result<Option<(Opcode, Vec<u8>)>, WsError> {
        loop {
            // Try to decode a frame from the buffer first.
            if let Some((frame, consumed)) = decode_frame(&self.read_buf) {
                self.read_buf.drain(..consumed);
                match frame.opcode {
                    Opcode::Ping => {
                        // Answer with a pong carrying the ping's payload.
                        let mask = random_mask();
                        let pong = encode_frame(Opcode::Pong, &frame.payload, &mask);
                        self.stream
                            .write_all(&pong)
                            .map_err(|e| WsError::Connection(format!("write pong: {e}")))?;
                        continue;
                    }
                    Opcode::Close => {
                        // Acknowledge close and signal end.
                        let mask = random_mask();
                        let ack = encode_frame(Opcode::Close, &frame.payload, &mask);
                        let _ = self.stream.write_all(&ack);
                        return Ok(None);
                    }
                    Opcode::Pong => continue,
                    _ => return Ok(Some((frame.opcode, frame.payload))),
                }
            }
            // Need more bytes.
            let mut tmp = [0u8; 4096];
            let n = self
                .stream
                .read(&mut tmp)
                .map_err(|e| WsError::Connection(format!("read frame: {e}")))?;
            if n == 0 {
                return Err(WsError::Connection("connection closed by peer".into()));
            }
            self.read_buf.extend_from_slice(&tmp[..n]);
        }
    }

    /// Send a close frame and shut down the connection cleanly.
    pub fn close(&mut self) -> Result<(), WsError> {
        let mask = random_mask();
        let frame = encode_frame(Opcode::Close, &[], &mask);
        let _ = self.stream.write_all(&frame);
        let _ = self.stream.flush();
        Ok(())
    }
}

/// Parse a `ws://`/`wss://` URL into (host, port, use_tls).
fn parse_ws_url(url: &str) -> Result<(String, u16, bool), WsError> {
    let (use_tls, rest) = if let Some(r) = url.strip_prefix("wss://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("ws://") {
        (false, r)
    } else {
        return Err(WsError::Connection(format!(
            "refusing non-ws(s) URL: {url}"
        )));
    };
    // Strip the path/query: the host is up to the first '/' or '?'.
    let authority = rest.split(['/', '?']).next().unwrap_or(rest);
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>()
                .map_err(|_| WsError::Connection(format!("invalid port in URL: {url}")))?,
        ),
        None => (authority.to_string(), if use_tls { 443 } else { 80 }),
    };
    Ok((host, port, use_tls))
}

/// Build the HTTP `Upgrade` request bytes to send over the socket.
fn build_handshake_request(host: &str, port: u16, key: &str) -> String {
    // The Host header includes the port unless it's the scheme default.
    let host_header = if (port == 80) || (port == 443) {
        host.to_string()
    } else {
        format!("{host}:{port}")
    };
    format!(
        "GET / HTTP/1.1\r\n\
         Host: {host_header}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\n\
         Sec-WebSocket-Version: 13\r\n\
         \r\n"
    )
}

/// Connect a rustls TLS stream over the given TCP socket for `host`.
fn connect_tls(
    host: &str,
    tcp: TcpStream,
) -> Result<rustls::StreamOwned<rustls::ClientConnection, TcpStream>, WsError> {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let conn = rustls::ClientConnection::new(
        std::sync::Arc::new(config),
        rustls::pki_types::ServerName::try_from(host.to_string())
            .map_err(|e| WsError::Tls(format!("invalid server name {host}: {e}")))?,
    )
    .map_err(|e| WsError::Tls(format!("tls connect: {e}")))?;
    let mut stream = rustls::StreamOwned::new(conn, tcp);
    stream
        .flush()
        .map_err(|e| WsError::Tls(format!("tls handshake: {e}")))?;
    Ok(stream)
}

impl Stream for rustls::StreamOwned<rustls::ClientConnection, TcpStream> {
    fn set_timeout(&mut self, dur: Option<Duration>) {
        let _ = self.get_ref().set_read_timeout(dur);
        let _ = self.get_ref().set_write_timeout(dur);
    }
}

/// Generate a client key (16 random bytes, base64). RFC 6455 §4.1.
fn generate_key() -> String {
    base64_encode(&random_bytes(16))
}

/// A random 4-byte masking key per frame (RFC 6455 §5.3).
fn random_mask() -> [u8; 4] {
    let b = random_bytes(4);
    [b[0], b[1], b[2], b[3]]
}

/// Fill `n` bytes from the OS RNG (via `getrandom` through `ring`).
fn random_bytes(n: usize) -> Vec<u8> {
    use ring::rand::{SecureRandom, SystemRandom};
    let rng = SystemRandom::new();
    let mut out = vec![0u8; n];
    rng.fill(&mut out).expect("ring rng failure");
    out
}

/// Compute the expected `Sec-WebSocket-Accept` value for a given key:
/// base64(sha1(key + GUID)). RFC 6455 §4.2.2.
fn compute_accept(key: &str) -> String {
    use ring::digest::{digest, SHA1_FOR_LEGACY_USE_ONLY};
    let mut data = key.as_bytes().to_vec();
    data.extend_from_slice(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let d = digest(&SHA1_FOR_LEGACY_USE_ONLY, &data);
    base64_encode(d.as_ref())
}

/// Standard base64 encode of `bytes` (RFC 4648). Dependency-free.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };
        let triple = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(ALPHA[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHA[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHA[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Parse the HTTP status code and `Sec-WebSocket-Accept` header from a raw
/// response. Returns `(0, None)` if the status line is absent.
fn parse_handshake_response(raw: &[u8]) -> (u16, Option<String>) {
    let text = String::from_utf8_lossy(raw);
    let mut status = 0u16;
    let mut accept = None;
    for line in text.split("\r\n") {
        let line = line.trim();
        if line.starts_with("HTTP/") {
            // "HTTP/1.1 101 Switching Protocols"
            let mut parts = line.split_whitespace();
            parts.next(); // "HTTP/1.1"
            if let Some(code) = parts.next() {
                status = code.parse().unwrap_or(0);
            }
        } else if let Some(rest) = line
            .strip_prefix("Sec-WebSocket-Accept:")
            .or_else(|| line.strip_prefix("sec-websocket-accept:"))
        {
            accept = Some(rest.trim().to_string());
        }
    }
    (status, accept)
}

/// WebSocket opcode (RFC 6455 §5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opcode {
    /// Continuation of a fragmented message.
    Continuation = 0x0,
    /// A text data frame.
    Text = 0x1,
    /// A binary data frame.
    Binary = 0x2,
    /// A connection close frame.
    Close = 0x8,
    /// A ping frame.
    Ping = 0x9,
    /// A pong frame.
    Pong = 0xA,
}

impl Opcode {
    fn from_byte(b: u8) -> Option<Self> {
        match b & 0x0f {
            0x0 => Some(Opcode::Continuation),
            0x1 => Some(Opcode::Text),
            0x2 => Some(Opcode::Binary),
            0x8 => Some(Opcode::Close),
            0x9 => Some(Opcode::Ping),
            0xA => Some(Opcode::Pong),
            _ => None,
        }
    }
}

/// A decoded WebSocket frame (server-to-client, never masked).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Whether this frame is the final one in a message.
    pub fin: bool,
    /// The frame opcode.
    pub opcode: Opcode,
    /// The frame payload bytes.
    pub payload: Vec<u8>,
}

/// Encode a client-to-server WebSocket frame (always masked, per RFC 6455
/// §5.3). Returns the wire bytes. The masking key is supplied by the
/// caller so tests are deterministic; a real client uses random bytes.
pub fn encode_frame(opcode: Opcode, payload: &[u8], mask_key: &[u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    let mut byte0 = 0x80u8; // FIN set
    byte0 |= opcode as u8;
    out.push(byte0);

    let len = payload.len();
    let mask_bit = 0x80u8;
    if len < 126 {
        out.push(mask_bit | (len as u8));
    } else if len <= u16::MAX as usize {
        out.push(mask_bit | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(mask_bit | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }

    out.extend_from_slice(mask_key);
    for (i, &b) in payload.iter().enumerate() {
        out.push(b ^ mask_key[i % 4]);
    }
    out
}

/// Decode a single server-to-client WebSocket frame from `buf`. Returns the
/// decoded frame and the number of bytes consumed, or `None` if `buf` does
/// not yet contain a complete frame. Server frames are never masked
/// (RFC 6455 §5.3), so a masked server frame is rejected as a protocol
/// error.
pub fn decode_frame(buf: &[u8]) -> Option<(Frame, usize)> {
    if buf.len() < 2 {
        return None;
    }
    let byte0 = buf[0];
    let byte1 = buf[1];
    let fin = (byte0 & 0x80) != 0;
    let opcode = Opcode::from_byte(byte0)?;
    let masked = (byte1 & 0x80) != 0;
    if masked {
        // Server-to-client frames must not be masked.
        return None;
    }

    let mut len = (byte1 & 0x7f) as usize;
    let mut idx = 2usize;
    if len == 126 {
        if buf.len() < idx + 2 {
            return None;
        }
        len = u16::from_be_bytes([buf[idx], buf[idx + 1]]) as usize;
        idx += 2;
    } else if len == 127 {
        if buf.len() < idx + 8 {
            return None;
        }
        len = u64::from_be_bytes([
            buf[idx],
            buf[idx + 1],
            buf[idx + 2],
            buf[idx + 3],
            buf[idx + 4],
            buf[idx + 5],
            buf[idx + 6],
            buf[idx + 7],
        ]) as usize;
        idx += 8;
    }

    if buf.len() < idx + len {
        return None;
    }
    let payload = buf[idx..idx + len].to_vec();
    Some((
        Frame {
            fin,
            opcode,
            payload,
        },
        idx + len,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor, Read, Write};
    use std::sync::{Arc, Mutex};

    /// A test stream: writes append to a shared buffer the "server" reads;
    /// reads pull from a shared buffer the "server" writes. Lets us test
    /// WsConnection's send/recv loop offline against a scripted peer.
    #[derive(Clone)]
    struct LoopbackStream {
        to_server: Arc<Mutex<Vec<u8>>>,
        from_server: Arc<Mutex<Cursor<Vec<u8>>>>,
    }

    impl LoopbackStream {
        fn new(server_writes: Vec<u8>) -> (Self, Self) {
            let to_server = Arc::new(Mutex::new(Vec::new()));
            let from_server = Arc::new(Mutex::new(Cursor::new(server_writes)));
            (
                LoopbackStream {
                    to_server: to_server.clone(),
                    from_server: from_server.clone(),
                },
                LoopbackStream {
                    to_server,
                    from_server,
                },
            )
        }
    }

    impl Read for LoopbackStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let mut c = self.from_server.lock().unwrap();
            c.read(buf)
        }
    }

    impl Write for LoopbackStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut s = self.to_server.lock().unwrap();
            s.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Stream for LoopbackStream {
        fn set_timeout(&mut self, _dur: Option<Duration>) {}
    }

    #[test]
    fn rejects_non_ws_scheme_without_spawning() {
        let err = handshake("http://example.com").unwrap_err();
        assert!(matches!(err, WsError::Connection(_)));
    }

    #[test]
    fn rejects_file_scheme() {
        assert!(handshake("file:///etc/passwd").is_err());
    }

    #[test]
    fn parse_ws_url_wss_default_port() {
        let (host, port, tls) = parse_ws_url("wss://echo.example.com").unwrap();
        assert_eq!(host, "echo.example.com");
        assert_eq!(port, 443);
        assert!(tls);
    }

    #[test]
    fn parse_ws_url_ws_default_port() {
        let (host, port, tls) = parse_ws_url("ws://echo.example.com").unwrap();
        assert_eq!(host, "echo.example.com");
        assert_eq!(port, 80);
        assert!(!tls);
    }

    #[test]
    fn parse_ws_url_explicit_port() {
        let (host, port, tls) = parse_ws_url("wss://echo.example.com:8443/path").unwrap();
        assert_eq!(host, "echo.example.com");
        assert_eq!(port, 8443);
        assert!(tls);
    }

    #[test]
    fn parse_ws_url_rejects_non_ws() {
        assert!(parse_ws_url("https://example.com").is_err());
        assert!(parse_ws_url("ftp://example.com").is_err());
    }

    #[test]
    fn build_handshake_request_is_well_formed() {
        let req = build_handshake_request("example.com", 80, "dGhlIHNhbXBsZSBub25jZQ==");
        assert!(req.starts_with("GET / HTTP/1.1\r\n"));
        assert!(req.contains("Host: example.com\r\n"));
        assert!(req.contains("Upgrade: websocket\r\n"));
        assert!(req.contains("Connection: Upgrade\r\n"));
        assert!(req.contains("Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"));
        assert!(req.contains("Sec-WebSocket-Version: 13\r\n"));
        assert!(req.ends_with("\r\n\r\n"));
    }

    #[test]
    fn build_handshake_request_includes_non_default_port() {
        let req = build_handshake_request("example.com", 8080, "k");
        assert!(req.contains("Host: example.com:8080\r\n"));
    }

    #[test]
    fn compute_accept_matches_rfc_6455_test_vector() {
        // RFC 6455 §4.2.2 example: key "dGhlIHNhbXBsZSBub25jZQ==" -> accept
        // "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=".
        let accept = compute_accept("dGhlIHNhbXBsZSBub25jZQ==");
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn generates_a_24_char_base64_key() {
        // 16 bytes base64-encoded is always 24 chars (with padding).
        let key = generate_key();
        assert_eq!(key.len(), 24);
        assert!(key.ends_with('='));
    }

    #[test]
    fn base64_encode_round_trips_with_known_vector() {
        let enc = base64_encode(b"hello, websocket!");
        assert_eq!(enc, "aGVsbG8sIHdlYnNvY2tldCE=");
    }

    #[test]
    fn encode_text_frame_short_payload() {
        let out = encode_frame(Opcode::Text, b"hi", &[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(out[0], 0x81);
        assert_eq!(out[1], 0x82);
        assert_eq!(&out[2..6], &[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(out[6], b'h' ^ 0x12);
        assert_eq!(out[7], b'i' ^ 0x34);
    }

    #[test]
    fn encode_decode_round_trip_text() {
        let payload = b"hello websocket";
        let mask = [0xAA, 0xBB, 0xCC, 0xDD];
        let wire = encode_frame(Opcode::Text, payload, &mask);
        // The encoded client frame is masked, so the payload bytes differ.
        assert_eq!(
            wire[6..6 + payload.len()].to_vec(),
            payload
                .iter()
                .enumerate()
                .map(|(i, &b)| b ^ mask[i % 4])
                .collect::<Vec<u8>>()
        );
        // Decode needs the mask stripped; since we can't unmask without the
        // key in the codec, simulate a server frame (unmasked) and decode that.
        let mut server_wire = vec![0x81, payload.len() as u8];
        server_wire.extend_from_slice(payload);
        let (frame, n) = decode_frame(&server_wire).unwrap();
        assert_eq!(n, server_wire.len());
        assert!(frame.fin);
        assert_eq!(frame.opcode, Opcode::Text);
        assert_eq!(frame.payload, payload.to_vec());
    }

    #[test]
    fn decode_rejects_masked_server_frame() {
        let wire = [0x81, 0x82, 0x00, 0x00, 0x00, 0x00, 0, 0];
        assert!(decode_frame(&wire).is_none());
    }

    #[test]
    fn decode_handles_extended_16_bit_length() {
        let payload = vec![0x41u8; 200];
        let mut wire = vec![0x82, 126];
        wire.extend_from_slice(&(200u16).to_be_bytes());
        wire.extend_from_slice(&payload);
        let (frame, n) = decode_frame(&wire).unwrap();
        assert_eq!(n, wire.len());
        assert_eq!(frame.opcode, Opcode::Binary);
        assert_eq!(frame.payload.len(), 200);
    }

    #[test]
    fn decode_returns_none_when_incomplete() {
        assert!(decode_frame(&[0x81]).is_none());
        assert!(decode_frame(&[0x81, 0x05, b'a']).is_none());
    }

    #[test]
    fn decode_ping_frame() {
        let wire = [0x89, 0x00];
        let (frame, n) = decode_frame(&wire).unwrap();
        assert_eq!(n, 2);
        assert_eq!(frame.opcode, Opcode::Ping);
        assert!(frame.payload.is_empty());
    }

    #[test]
    fn parse_handshake_response_extracts_status_and_accept() {
        let raw = b"HTTP/1.1 101 Switching Protocols\r\n\
                   Upgrade: websocket\r\n\
                   Connection: Upgrade\r\n\
                   Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\
                   \r\n";
        let (status, accept) = parse_handshake_response(raw);
        assert_eq!(status, 101);
        assert_eq!(accept.as_deref(), Some("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));
    }

    #[test]
    fn parse_handshake_response_tolerates_lowercase_accept_header() {
        let raw = b"HTTP/1.1 101 ok\r\nsec-websocket-accept: abc=\r\n\r\n";
        let (status, accept) = parse_handshake_response(raw);
        assert_eq!(status, 101);
        assert_eq!(accept.as_deref(), Some("abc="));
    }

    #[test]
    fn parse_handshake_response_missing_status() {
        let raw = b"garbage no status line\r\n\r\n";
        let (status, accept) = parse_handshake_response(raw);
        assert_eq!(status, 0);
        assert_eq!(accept, None);
    }

    /// Build a `WsConnection` over a loopback pair whose "server" side has
    /// already written the given bytes. Lets us test recv() offline.
    fn conn_over(server_writes: Vec<u8>) -> (WsConnection, LoopbackStream) {
        let (client_stream, server_stream) = LoopbackStream::new(server_writes);
        let conn = WsConnection {
            stream: Box::new(client_stream),
            read_buf: Vec::new(),
        };
        (conn, server_stream)
    }

    #[test]
    fn recv_reads_a_text_frame_from_the_stream() {
        // A server-sent text frame "hi" (unmasked).
        let mut server_bytes = vec![0x81, 0x02, b'h', b'i'];
        let (mut conn, _server) = conn_over(std::mem::take(&mut server_bytes));
        let msg = conn.recv().unwrap().unwrap();
        assert_eq!(msg.0, Opcode::Text);
        assert_eq!(msg.1, b"hi".to_vec());
    }

    #[test]
    fn recv_answers_a_ping_with_a_pong() {
        // Server sends a ping with payload "x", then a text frame "yo".
        let server_bytes: Vec<u8> = vec![
            0x89, 0x01, b'x', // ping
            0x81, 0x02, b'y', b'o', // text
        ];
        let (mut conn, server) = conn_over(server_bytes);
        let msg = conn.recv().unwrap().unwrap();
        assert_eq!(msg.0, Opcode::Text);
        assert_eq!(msg.1, b"yo".to_vec());
        // The pong we wrote back should be on the server's recv buffer.
        let written = server.to_server.lock().unwrap().clone();
        // Pong frame: FIN+pong(0xA), masked, len 1, 4 mask bytes, 1 masked byte.
        assert_eq!(written[0], 0x8A);
        assert_eq!(written[1] & 0x80, 0x80); // mask bit set
        assert_eq!(written[1] & 0x7f, 1); // payload len 1
    }

    #[test]
    fn recv_on_close_returns_none() {
        // Server sends a close frame (no payload).
        let server_bytes: Vec<u8> = vec![0x88, 0x00];
        let (mut conn, _server) = conn_over(server_bytes);
        assert!(conn.recv().unwrap().is_none());
    }

    #[test]
    fn recv_assembles_a_frame_across_reads() {
        // The same text frame "hi" but fed in two chunks via a stream that
        // yields 1 byte per read until exhausted.
        struct OneByteStream {
            buf: Vec<u8>,
            pos: usize,
            written: Vec<u8>,
        }
        impl Read for OneByteStream {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.pos >= self.buf.len() {
                    return Ok(0);
                }
                buf[0] = self.buf[self.pos];
                self.pos += 1;
                Ok(1)
            }
        }
        impl Write for OneByteStream {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.written.extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        impl Stream for OneByteStream {
            fn set_timeout(&mut self, _d: Option<Duration>) {}
        }
        let s = OneByteStream {
            buf: vec![0x81, 0x02, b'h', b'i'],
            pos: 0,
            written: Vec::new(),
        };
        let mut conn = WsConnection {
            stream: Box::new(s),
            read_buf: Vec::new(),
        };
        let msg = conn.recv().unwrap().unwrap();
        assert_eq!(msg.1, b"hi".to_vec());
    }

    #[test]
    fn send_text_writes_a_masked_text_frame() {
        // Build a connection whose server side records what we write. Hold a
        // clone of the shared write-buffer (LoopbackStream uses Arc<Mutex>)
        // so we can inspect what send_text wrote without downcasting the
        // boxed trait object.
        let (client_stream, _server) = LoopbackStream::new(Vec::new());
        let write_buf = client_stream.to_server.clone();
        let mut conn = WsConnection {
            stream: Box::new(client_stream),
            read_buf: Vec::new(),
        };
        conn.send_text("hi").unwrap();
        let written = write_buf.lock().unwrap().clone();
        assert_eq!(written[0], 0x81); // FIN + text
        assert_eq!(written[1], 0x82); // masked, len 2
        let mask = &written[2..6];
        let payload = &written[6..8];
        let unmasked: Vec<u8> = payload
            .iter()
            .enumerate()
            .map(|(i, &b)| b ^ mask[i % 4])
            .collect();
        assert_eq!(unmasked, b"hi");
    }
}
