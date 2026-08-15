//! # pbe-net
//!
//! The **network on-ramp** — now a thin facade over the modular [`pbe_proto`]
//! protocol layer. Each modern protocol the engine speaks lives in its **own**
//! crate behind [`pbe_proto`]:
//!
//! | Scheme(s)        | Crate              |
//! |------------------|--------------------|
//! | `http`, `https`  | `pbe_proto_http`  |
//! | `ws`, `wss`      | `pbe_proto_ws`    |
//! | `data`           | `pbe_proto_data`  |
//!
//! This crate preserves the original [`fetch`] / [`fetch_bytes`] /
//! [`FetchedPage`] / [`FetchedBytes`] surface so existing callers
//! (`pbe-shell`, `pbe-orchestrator`) keep working unchanged, while routing
//! every URL through [`pbe_proto::fetch_bytes`] so the new `ws`/`wss`/`data:`
//! protocols are now reachable too. New callers should prefer the
//! [`pbe_proto`] API directly — it returns a single [`pbe_proto::Resource`]
//! for every protocol rather than the text/bytes pair this facade exposes.
//!
//! ## Doctrine: drive a sealed binary, link nothing
//!
//! Unchanged from before. No HTTP or TLS code is linked into the engine:
//! `pbe_proto_http` and `pbe_proto_ws` drive the **sealed system `curl`
//! binary** from outside via [`std::process`], so the network touches a
//! separate OS process and the engine only ever sees bytes handed back over a
//! pipe. `pbe_proto_data` is pure byte work — no I/O at all. See each
//! protocol crate's docs for its own security posture.
//!
//! ## No legacy protocols
//!
//! Only modern fetch protocols are routed: `http`/`https`, `ws`/`wss`, and
//! `data:`. Legacy schemes (`file://`, `ftp://`, `scp://`, …) are rejected
//! by [`pbe_proto`] as `UnsupportedScheme` rather than silently mis-handled.
//! `file:` access stays a browser-layer concern (`std::fs`), never a network
//! protocol.

pub use cap_http::HttpError;

// Re-export the modular protocol layer so callers that want the unified
// `Resource` API can reach it without adding a second dependency.
pub use pbe_proto::{fetch as proto_fetch, fetch_bytes as proto_fetch_bytes, FetchError, Resource};
// Per-protocol crates (pbe-proto-http, pbe-proto-ws, pbe-proto-data) are not
// re-exported here: this facade routes through pbe_proto, and callers that
// need a specific protocol crate's API (e.g. the WS frame codec) should depend
// on that crate directly. Per-protocol error types are reachable via
// pbe_proto::FetchError.

/// A page fetched from the network: the legacy text-body shape kept for
/// existing callers. `body` is a UTF-8 string via lossy decoding. For content
/// that may be binary (images, other non-text responses), use
/// [`fetch_bytes`] and [`FetchedBytes`] instead so the raw byte stream
/// survives without U+FFFD replacements corrupting non-UTF-8 sequences.
///
/// For new code, prefer [`pbe_proto::Resource`] (one type for every protocol).
#[derive(Clone, Debug)]
pub struct FetchedPage {
    /// The URL actually loaded (after redirects).
    pub final_url: String,
    /// HTTP status code (0 for `data:`/`ws`).
    pub status: u16,
    /// `Content-Type`, if reported.
    pub content_type: Option<String>,
    /// The response body, decoded as UTF-8 (lossy).
    pub body: String,
}

/// Same as [`FetchedPage`] but with the response body as raw bytes — the
/// shape that binary content (PNG/JPEG/BMP/font/binary blob) needs. HTML
/// and CSS callers can keep using [`fetch`] and treat the response as UTF-8.
///
/// For new code, prefer [`pbe_proto::Resource`] (one type for every protocol).
#[derive(Clone, Debug)]
pub struct FetchedBytes {
    /// The URL actually loaded (after redirects).
    pub final_url: String,
    /// HTTP status code (0 for `data:`/`ws`).
    pub status: u16,
    /// `Content-Type`, if reported.
    pub content_type: Option<String>,
    /// The response body as raw bytes — never UTF-8-lossy'd.
    pub body: Vec<u8>,
}

impl From<Resource> for FetchedPage {
    fn from(r: Resource) -> Self {
        let Resource {
            final_url,
            status,
            content_type,
            body,
        } = r;
        FetchedPage {
            final_url,
            status,
            content_type,
            body: String::from_utf8_lossy(&body).to_string(),
        }
    }
}

impl From<Resource> for FetchedBytes {
    fn from(r: Resource) -> Self {
        FetchedBytes {
            final_url: r.final_url,
            status: r.status,
            content_type: r.content_type,
            body: r.body,
        }
    }
}

/// Fetch a URL by routing to the matching protocol crate and return the
/// response as UTF-8-decoded text. HTML and CSS callers want this shape. For
/// content that may be binary (images, fonts, blobs), use [`fetch_bytes`]
/// instead — String-decoding non-UTF-8 bytes replaces them with U+FFFD,
/// which corrupts PNG/JPEG/BMP streams.
///
/// Modern protocols routed: `http`, `https`, `ws`, `wss`, `data`. Legacy
/// schemes are rejected as [`FetchError::UnsupportedScheme`].
pub fn fetch(url: &str) -> Result<FetchedPage, HttpError> {
    let r = pbe_proto::fetch(url).map_err(fetch_error_to_http)?;
    Ok(FetchedPage::from(r))
}

/// Fetch a URL by routing to the matching protocol crate and return the
/// response as raw bytes. This is the primitive image decoders + binary
/// consumers need — no UTF-8 lossy round-trip corrupts the stream. Same
/// protocol routing as [`fetch`].
pub fn fetch_bytes(url: &str) -> Result<FetchedBytes, HttpError> {
    let r = pbe_proto::fetch_bytes(url).map_err(fetch_error_to_http)?;
    Ok(FetchedBytes::from(r))
}

/// Map the unified [`FetchError`] back onto the legacy [`HttpError`] this
/// facade has always returned. Unsupported schemes become `Connection`
/// errors (preserving the pre-modularisation rejection behaviour); `data`
/// and `ws` errors are flattened into `Connection` with a descriptive
/// message, since the legacy API has no variant for them.
fn fetch_error_to_http(e: FetchError) -> HttpError {
    match e {
        FetchError::UnsupportedScheme(s) => {
            HttpError::Connection(format!("refusing non-modern URL scheme: {s}"))
        }
        FetchError::Http(h) => h,
        FetchError::Ws(w) => HttpError::Connection(format!("websocket error: {w:?}")),
        FetchError::Data(d) => HttpError::Connection(format!("data uri error: {d:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_scheme_without_spawning() {
        let err = fetch("file:///etc/passwd").unwrap_err();
        assert!(matches!(err, HttpError::Connection(_)));
    }

    #[test]
    fn rejects_scp_scheme() {
        assert!(fetch("scp://host/secret").is_err());
    }

    #[test]
    fn fetch_bytes_rejects_non_http_scheme_without_spawning() {
        let err = fetch_bytes("file:///etc/passwd").unwrap_err();
        assert!(matches!(err, HttpError::Connection(_)));
    }

    #[test]
    fn fetch_bytes_rejects_scp_scheme() {
        assert!(fetch_bytes("scp://host/secret").is_err());
    }

    #[test]
    fn rejects_ftp_as_unsupported_modern_scheme() {
        let err = fetch("ftp://host/x").unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("ftp"), "expected scheme name in error: {msg}");
    }

    #[test]
    fn data_uri_decodes_through_facade() {
        // data: is a modern protocol, so it routes through and decodes.
        let page = fetch("data:,hello").unwrap();
        assert_eq!(page.body, "hello");
        assert_eq!(page.status, 0);
    }

    #[test]
    fn data_uri_bytes_decode_through_facade() {
        let page = fetch_bytes("data:text/plain;base64,aGVsbG8=").unwrap();
        assert_eq!(page.body, b"hello");
    }

    #[test]
    fn resource_converts_to_fetched_page() {
        let r = Resource {
            final_url: "data:,x".into(),
            status: 0,
            content_type: Some("text/plain".into()),
            body: b"x".to_vec(),
        };
        let p = FetchedPage::from(r);
        assert_eq!(p.body, "x");
    }

    #[test]
    fn resource_converts_to_fetched_bytes() {
        let r = Resource {
            final_url: "data:,x".into(),
            status: 200,
            content_type: None,
            body: vec![1, 2, 3],
        };
        let b = FetchedBytes::from(r);
        assert_eq!(b.body, vec![1, 2, 3]);
    }
}
