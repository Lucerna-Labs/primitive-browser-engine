//! # pbe-proto
//!
//! The **protocol dispatch** layer — the single composition point every
//! browser caller goes through to turn a URL into a [`Resource`]. Each modern
//! protocol the engine speaks lives in its **own** crate behind this one:
//!
//! | Scheme(s)        | Crate             | Role |
//! |------------------|-------------------|------|
//! | `http`, `https`  | [`pbe_proto_http`] | Fetch a document by driving the sealed system `curl`. |
//! | `ws`, `wss`      | [`pbe_proto_ws`]   | WebSocket handshake + frame exchange. |
//! | `data`           | [`pbe_proto_data`] | RFC 2397 `data:` URI decode (text + base64). |
//!
//! ## Why a dispatch crate, not a monolith
//!
//! The browser engine doctrine is *composition from outside* — and that
//! applies to protocols as much as to rendering. Bundling HTTP + WebSocket +
//! `data:` decoding into one crate would be the same category of mistake as
//! the old bus wrapper: one concern growing to own three, each with its own
//! failure mode, its own upgrade cadence, and its own audit surface.
//!
//! Instead this crate is deliberately tiny: it owns the shared [`Resource`]
//! shape (the one type every protocol returns), the [`FetchError`] enum (the
//! one error every protocol raises through), and the [`fetch`]/[`fetch_bytes`]
//! routers that read a URL's scheme and hand off to the matching crate. A
//! protocol can be swapped, upgraded, or debugged in isolation — add a
//! crate, register it in [`fetch_bytes`], done; nothing else recompiles that
//! doesn't depend on it.
//!
//! ## Modern only — no legacy protocols
//!
//! Only the protocols a modern browser fetches over are routed here. Legacy
//! schemes (`file://`, `ftp://`, `scp://`, …) are **not** supported — they
//! are rejected as [`FetchError::UnsupportedScheme`] rather than silently
//! mis-handled. `file:` access stays a browser-layer concern (`std::fs`),
//! never a network protocol, exactly as before.
//!
//! ## Security posture unchanged
//!
//! Routing lives here; mechanism stays in the per-protocol crates. HTTP/HTTPS
//! still links **no** HTTP/TLS code — [`pbe_proto_http`] drives the sealed
//! system `curl` binary from outside via `std::process`, so the network touches
//! a separate OS process and the engine only ever sees bytes handed back over
//! a pipe. WebSocket reuses that same sealed `curl` for the TLS/HTTP upgrade
//! handshake (`--include` + HTTP `Upgrade`), so no crypto is linked there
//! either. `data:` is pure byte work, no I/O at all.

use pbe_proto_data as data;
use pbe_proto_http as http;
use pbe_proto_ws as ws;

pub use pbe_proto_data::DataError;
pub use pbe_proto_http::HttpError;
pub use pbe_proto_ws::WsError;
// The persistent-connection + frame-codec API. `fetch`/`fetch_bytes` route a
// URL to a one-shot Resource; for a live WebSocket a caller opens a
// connection directly via `ws::connect`.
pub use pbe_proto_ws::{connect as ws_connect, Opcode, Stream, WsConnection};

/// A fetched resource — the one shared return type every protocol produces.
///
/// `body` is the raw byte stream; text consumers decode it as UTF-8 (lossy is
/// fine for HTML/CSS, binary content like images must use [`fetch_bytes`] so
/// the bytes survive intact). `final_url` is the URL actually resolved (after
/// redirects for HTTP, the decoded `data:` URI for `data:`, the upgraded
/// endpoint for `ws`/`wss`).
#[derive(Clone, Debug)]
pub struct Resource {
    /// The URL actually resolved (after redirects / decode).
    pub final_url: String,
    /// HTTP status code where meaningful (HTTP); `0` for `data:`/`ws`.
    pub status: u16,
    /// `Content-Type`, if the protocol reports one.
    pub content_type: Option<String>,
    /// The response body as raw bytes — never UTF-8-lossy'd here.
    pub body: Vec<u8>,
}

impl Resource {
    /// The body as a UTF-8 string (lossy). For HTML/CSS callers that don't
    /// care about byte fidelity. Binary consumers must read [`Resource::body`]
    /// directly instead.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// Errors any protocol can raise, surfaced through one enum so callers have a
/// single `match` arm set regardless of which protocol handled the URL.
#[derive(Clone, Debug, PartialEq)]
pub enum FetchError {
    /// No registered protocol crate handles this URL scheme.
    UnsupportedScheme(String),
    /// The HTTP on-ramp failed (curl missing, non-zero exit, bad metadata).
    Http(HttpError),
    /// The WebSocket on-ramp failed (handshake rejected, frame error).
    Ws(WsError),
    /// A `data:` URI failed to decode (bad base64, malformed media type).
    Data(DataError),
}

impl From<HttpError> for FetchError {
    fn from(e: HttpError) -> Self {
        FetchError::Http(e)
    }
}

// Note: `WsError` and `HttpError` are the same underlying type
// (`cap_http::HttpError`), so a separate `From<WsError>` impl would collide.
// WebSocket callers construct `FetchError::Ws(_)` directly via the `?`-free
// path in `fetch_bytes`.
impl From<DataError> for FetchError {
    fn from(e: DataError) -> Self {
        FetchError::Data(e)
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::UnsupportedScheme(s) => {
                write!(f, "unsupported URL scheme (not a modern protocol): {s}")
            }
            FetchError::Http(e) => write!(f, "http: {e:?}"),
            FetchError::Ws(e) => write!(f, "ws: {e:?}"),
            FetchError::Data(e) => write!(f, "data: {e:?}"),
        }
    }
}

impl std::error::Error for FetchError {}

/// Route a URL to its protocol crate and return the resolved [`Resource`]
/// with a UTF-8 (lossy) body. Convenience wrapper over [`fetch_bytes`] for
/// text consumers (HTML, CSS); binary consumers should call [`fetch_bytes`]
/// directly so non-UTF-8 byte streams survive intact.
///
/// Supported modern schemes: `http`, `https`, `ws`, `wss`, `data`. Anything
/// else is [`FetchError::UnsupportedScheme`] — legacy protocols are
/// deliberately not handled.
pub fn fetch(url: &str) -> Result<Resource, FetchError> {
    let r = fetch_bytes(url)?;
    Ok(Resource {
        final_url: r.final_url,
        status: r.status,
        content_type: r.content_type,
        body: String::from_utf8_lossy(&r.body).to_string().into_bytes(),
    })
}

/// Route a URL to its protocol crate and return the resolved [`Resource`] with
/// the raw byte body. This is the binary-safe primitive image/font/blob
/// decoders need — no UTF-8 lossy round-trip corrupts the stream.
pub fn fetch_bytes(url: &str) -> Result<Resource, FetchError> {
    match scheme_of(url) {
        Scheme::Http => {
            let f = http::fetch_bytes(url)?;
            Ok(Resource {
                final_url: f.final_url,
                status: f.status,
                content_type: f.content_type,
                body: f.body,
            })
        }
        Scheme::WebSocket => {
            let h = ws::handshake(url).map_err(FetchError::Ws)?;
            Ok(Resource {
                final_url: h.final_url,
                status: h.status,
                content_type: h.accept.clone(),
                body: h.body,
            })
        }
        Scheme::Data => {
            let d = data::decode(url)?;
            Ok(Resource {
                final_url: url.to_string(),
                status: 0,
                content_type: Some(d.content_type),
                body: d.body,
            })
        }
        Scheme::Unsupported(s) => Err(FetchError::UnsupportedScheme(s)),
    }
}

/// Which modern protocol a URL's scheme maps to.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Scheme {
    /// `http://` / `https://`
    Http,
    /// `ws://` / `wss://`
    WebSocket,
    /// `data:`
    Data,
    /// Anything else (legacy or unknown) — not routed.
    Unsupported(String),
}

/// Classify a URL's scheme. Returns the literal scheme string for the
/// `Unsupported` variant so the error can name it.
fn scheme_of(url: &str) -> Scheme {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Scheme::Http
    } else if lower.starts_with("ws://") || lower.starts_with("wss://") {
        Scheme::WebSocket
    } else if lower.starts_with("data:") {
        Scheme::Data
    } else {
        // Grab the scheme token (up to the first ':') for a helpful error.
        let scheme = url
            .split_once(':')
            .map(|(s, _)| s.to_string())
            .unwrap_or_else(|| url.to_string());
        Scheme::Unsupported(scheme)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_http_to_http_scheme() {
        assert_eq!(scheme_of("https://example.com"), Scheme::Http);
        assert_eq!(scheme_of("HTTP://Example.COM/x"), Scheme::Http);
    }

    #[test]
    fn routes_ws_to_websocket_scheme() {
        assert_eq!(scheme_of("ws://echo.example.com"), Scheme::WebSocket);
        assert_eq!(scheme_of("WSS://echo.example.com"), Scheme::WebSocket);
    }

    #[test]
    fn routes_data_to_data_scheme() {
        assert_eq!(scheme_of("data:,hello"), Scheme::Data);
    }

    #[test]
    fn rejects_legacy_file_scheme() {
        let err = fetch_bytes("file:///etc/passwd").unwrap_err();
        assert!(matches!(err, FetchError::UnsupportedScheme(_)));
    }

    #[test]
    fn rejects_unknown_scheme_with_its_name() {
        let err = fetch_bytes("ftp://host/x").unwrap_err();
        match err {
            FetchError::UnsupportedScheme(s) => assert_eq!(s, "ftp"),
            other => panic!("expected UnsupportedScheme, got {other:?}"),
        }
    }

    #[test]
    fn reject_message_names_the_scheme() {
        let err = FetchError::UnsupportedScheme("gopher".into());
        assert!(err.to_string().contains("gopher"));
    }
}
