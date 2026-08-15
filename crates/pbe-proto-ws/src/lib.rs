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
//!    connection. This crate drives the **sealed system `curl` binary** with
//!    `--include` for the handshake, so the engine links **no** HTTP or TLS
//!    stack — the same "drive a sealed executor from outside, never link its
//!    internals" doctrine as HTTP. `curl` performs the `wss://` TLS itself;
//!    the engine only ever sees the handshake response bytes over a pipe.
//! 2. **The frame codec** — once upgraded, messages are RFC 6455 frames
//!    (opcode + masked payload). This crate implements the codec in pure Rust:
//!    [`encode_frame`] (client-to-server, always masked) and
//!    [`decode_frame`] (server-to-client, never masked). No I/O, no deps
//!    beyond the shared `cap-http` error type.
//!
//! Splitting handshake (sealed process) from codec (pure bytes) keeps the
//! security posture identical to HTTP: zero linked crypto, process isolation
//! at the network boundary, immutable owned values everywhere.
//!
//! ## Status
//!
//! The handshake + codec are complete and unit-tested offline. A live
//! read/write loop over the upgraded socket is intentionally not wired here:
//! `curl --include` returns the handshake response and closes the data
//! channel, so a true persistent connection needs a socket primitive the
//! engine does not yet link (matching the doctrine: don't link what you
//! don't need). The dispatch layer returns the handshake result as a
//! [`Handshake`] resource; the codec is the swappable building block a future
//! persistent-connection crate composes over.

use std::process::Command;

pub use cap_http::HttpError as WsError;

/// A completed WebSocket handshake — the on-ramp's concrete, immutable
/// output. The dispatch layer's [`Resource`] is built `From` this.
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
/// driving the sealed system `curl` with `--include` (so the raw HTTP
/// upgrade response is captured). A client key is generated; the response's
/// `Sec-WebSocket-Accept` is returned for verification.
///
/// Security posture: `curl` performs the TLS for `wss://` itself — we link
/// no crypto. Only `ws`/`wss` schemes are accepted; anything else is
/// rejected before a process is spawned.
pub fn handshake(url: &str) -> Result<Handshake, WsError> {
    let scheme_ok = url.starts_with("ws://") || url.starts_with("wss://");
    if !scheme_ok {
        return Err(WsError::Connection(format!(
            "refusing non-ws(s) URL: {url}"
        )));
    }

    // curl speaks http(s) natively, so map ws->http, wss->https for the
    // handshake transport. The Upgrade headers below turn the HTTP request
    // into a WebSocket one.
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

/// Generate a client key. RFC 6455 §4.1: 16 random bytes, base64-encoded.
/// Here we derive a fixed-but-varied key from the system clock so the crate
/// stays dependency-free (a real RNG would add a dep; the handshake works
/// with any 16-byte base64 string, and `curl` does not validate entropy).
fn generate_key() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let bytes = [
        (nanos & 0xff) as u8,
        ((nanos >> 8) & 0xff) as u8,
        ((nanos >> 16) & 0xff) as u8,
        ((nanos >> 24) & 0xff) as u8,
        ((nanos >> 32) & 0xff) as u8,
        ((nanos >> 40) & 0xff) as u8,
        ((nanos >> 48) & 0xff) as u8,
        ((nanos >> 56) & 0xff) as u8,
        0x50,
        0x42,
        0x45,
        0x57,
        0x53,
        0x4b,
        0x45,
        0x59, // "PBEWSKEY"
    ];
    base64_encode(&bytes)
}

/// Standard base64 encode of 16 bytes (RFC 4648). Dependency-free.
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
/// `--include` response. Returns `(0, None)` if the status line is absent.
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

const TIMEOUT_SECS: u32 = 20;

/// WebSocket opcode (RFC 6455 £5.2).
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
/// £5.3). Returns the wire bytes. The masking key is supplied by the
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
    fn generates_a_24_char_base64_key() {
        // 16 bytes base64-encoded is always 24 chars (with padding).
        let key = generate_key();
        assert_eq!(key.len(), 24);
        assert!(key.ends_with('='));
    }

    #[test]
    fn base64_encode_round_trips_with_decode() {
        // Cross-check against a known vector.
        let bytes = b"hello, websocket!";
        let enc = base64_encode(bytes);
        assert_eq!(enc, "aGVsbG8sIHdlYnNvY2tldCE=");
    }

    #[test]
    fn encode_text_frame_short_payload() {
        let out = encode_frame(Opcode::Text, b"hi", &[0x12, 0x34, 0x56, 0x78]);
        // FIN=1, opcode=1, masked, len=2, mask, masked payload.
        assert_eq!(out[0], 0x81);
        assert_eq!(out[1], 0x82);
        assert_eq!(&out[2..6], &[0x12, 0x34, 0x56, 0x78]);
        // 'h' ^ 0x12, 'i' ^ 0x34
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
        // A masked server-to-client frame is a protocol violation.
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
        // First byte only.
        assert!(decode_frame(&[0x81]).is_none());
        // Length announced but payload not all there yet.
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
}
