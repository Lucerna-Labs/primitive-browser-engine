//! # pbe-proto-http
//!
//! The **HTTP/HTTPS protocol crate** — fetches a URL by driving the sealed
//! system `curl` binary from outside via [`std::process`]. One modern
//! protocol, one crate, independently swappable: the [`pbe_proto`] dispatch
//! layer routes `http`/`https` URLs here, and nothing else in the engine
//! links an HTTP or TLS stack.
//!
//! ## Doctrine: drive a sealed binary, link nothing
//!
//! A crypto/HTTP crate is a large, mutable, transitively-sprawling attack
//! surface — the exact thing the composition doctrine exists to eliminate.
//! Instead we drive the **sealed system `curl` binary** from outside, the same
//! way the Spiderweb bus reaches encrypted transport by composing the system
//! `ssh` rather than reimplementing crypto. `curl` ships in Windows System32
//! and on every Unix.
//!
//! What this buys, in security terms:
//! - **Zero linked network/crypto dependencies.** This crate's only dependency
//!   is the kit's `cap-http` *type contract*. `cargo` pulls in no TLS, no
//!   parser, no async runtime — nothing to audit, nothing that can be a
//!   linked CVE in our address space.
//! - **Process isolation.** The network touches a separate, sealed process;
//!   the engine only ever sees bytes handed back over a pipe. The fetch
//!   boundary is an OS process boundary, not a function call into mutable
//!   foreign code.
//! - **Immutable inputs/outputs.** A [`Fetched`] is a plain owned value; the
//!   primitive is a pure `url -> Result<Fetched>` with no shared state.
//!
//! No policy lives here — which URL, whether to allow non-HTTPS, etc. is the
//! dispatch layer's decision.

use std::process::Command;

pub use cap_http::HttpError;

/// An HTTP-fetched resource — the HTTP on-ramp's concrete, immutable output.
/// `body` is the raw byte stream; text consumers decode it as UTF-8 (lossy is
/// fine for HTML/CSS), binary consumers (images, fonts) read it directly.
#[derive(Clone, Debug)]
pub struct Fetched {
    /// The URL actually loaded (after redirects).
    pub final_url: String,
    /// HTTP status code.
    pub status: u16,
    /// `Content-Type`, if reported.
    pub content_type: Option<String>,
    /// The response body as raw bytes — never UTF-8-lossy'd here.
    pub body: Vec<u8>,
}

impl Fetched {
    /// The body as a UTF-8 string (lossy). For HTML/CSS callers that don't
    /// care about byte fidelity.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// Sentinel that separates the response body from the trailing metadata block
/// `curl -w` appends. Chosen to be vanishingly unlikely to occur in real HTML.
const META_SENTINEL: &str = "\n__PBE_NET_META__\n";
/// Field separator inside the metadata block. A newline is safe because none
/// of the captured fields (status code, content-type, URL) contain a newline
/// — whereas content-type DOES contain spaces and `;` (e.g.
/// "text/html; charset=utf-8"), so a space-separated format would mis-parse.
const META_FIELD_SEP: &str = "\n__PBE_FIELD__\n";

/// Default network timeout (seconds) handed to the sealed `curl`.
const TIMEOUT_SECS: u32 = 20;
/// Max redirects `curl` will follow.
const MAX_REDIRECTS: u32 = 5;

/// Fetch an `http`/`https` URL by driving the sealed system `curl` and return
/// the response as a UTF-8 (lossy) string body. HTML and CSS callers want this
/// shape. For content that may be binary (images, fonts, blobs), use
/// [`fetch_bytes`] instead — String-decoding non-UTF-8 bytes replaces them
/// with U+FFFD, which corrupts PNG/JPEG/BMP streams.
///
/// Security posture: HTTPS is enforced by `curl`'s own (sealed, OS-maintained)
/// TLS — we never link or touch crypto. Only `http`/`https` schemes are
/// accepted; anything else is rejected before a process is spawned.
pub fn fetch(url: &str) -> Result<Fetched, HttpError> {
    let raw = fetch_raw(url)?;
    Ok(Fetched {
        final_url: raw.final_url,
        status: raw.status,
        content_type: raw.content_type,
        body: String::from_utf8_lossy(&raw.body).to_string().into_bytes(),
    })
}

/// Fetch an `http`/`https` URL by driving the sealed system `curl` and return
/// the response as raw bytes. This is the primitive image decoders + binary
/// consumers need — no UTF-8 lossy round-trip corrupts the stream. Same
/// security posture as [`fetch`] (http/https-only, curl-driven, no linked
/// crypto).
pub fn fetch_bytes(url: &str) -> Result<Fetched, HttpError> {
    let raw = fetch_raw(url)?;
    Ok(Fetched {
        final_url: raw.final_url,
        status: raw.status,
        content_type: raw.content_type,
        body: raw.body,
    })
}

/// Internal: the shared curl-driving + byte-level sentinel-split path used by
/// both [`fetch`] and [`fetch_bytes`]. Keeps the whole HTTP + metadata + args
/// concern in one place so a fix in one path fixes both.
struct RawFetch {
    final_url: String,
    status: u16,
    content_type: Option<String>,
    body: Vec<u8>,
}

fn fetch_raw(url: &str) -> Result<RawFetch, HttpError> {
    let scheme_ok = url.starts_with("https://") || url.starts_with("http://");
    if !scheme_ok {
        return Err(HttpError::Connection(format!(
            "refusing non-http(s) URL: {url}"
        )));
    }

    // `-w` appends "<sentinel><code><sep><content_type><sep><effective_url>"
    // AFTER the body on stdout, so one capture yields body + metadata with no
    // header parsing and no ambiguity across redirects. Fields are newline-
    // delimited because content-type contains spaces/semicolons.
    let writeout = format!(
        "{META_SENTINEL}%{{http_code}}{META_FIELD_SEP}%{{content_type}}{META_FIELD_SEP}%{{url_effective}}"
    );

    let output = Command::new("curl")
        .arg("--silent")
        .arg("--show-error")
        .arg("--location") // follow redirects
        .arg("--max-redirs")
        .arg(MAX_REDIRECTS.to_string())
        .arg("--max-time")
        .arg(TIMEOUT_SECS.to_string())
        .arg("--proto") // belt-and-braces: only let curl speak http(s)
        .arg("=http,https")
        .arg("--user-agent")
        .arg("pbe/0.1 (primitive browser engine)")
        .arg("--write-out")
        .arg(&writeout)
        .arg("--")
        .arg(url)
        .output()
        .map_err(|e| HttpError::Connection(format!("could not run curl: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(HttpError::Connection(format!(
            "curl failed ({}): {}",
            output.status,
            stderr.trim()
        )));
    }

    // Split at the byte level: the sentinel is ASCII, so it's the same byte
    // sequence whether the surrounding body is UTF-8, Latin-1, or an
    // uncompressed PNG. `rfind_bytes` walks the tail to find the last
    // occurrence — real bodies never contain the sentinel pattern.
    let stdout = output.stdout;
    let sentinel_bytes = META_SENTINEL.as_bytes();
    let split_idx = rfind_bytes(&stdout, sentinel_bytes)
        .ok_or_else(|| HttpError::Connection("curl output missing metadata sentinel".into()))?;
    let body_bytes = stdout[..split_idx].to_vec();
    let meta_bytes = &stdout[split_idx + sentinel_bytes.len()..];
    // Metadata is always ASCII: HTTP status code, RFC-7231 content-type,
    // percent-encoded URL. UTF-8 decoding is safe here.
    let meta = std::str::from_utf8(meta_bytes)
        .map_err(|_| HttpError::Connection("curl metadata block not UTF-8".into()))?;

    let mut parts = meta.split(META_FIELD_SEP);
    let status = parts
        .next()
        .map(str::trim)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| HttpError::Connection("curl returned no status code".into()))?;
    let ct = parts.next().map(|s| s.trim().to_string());
    let final_url = parts
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| url.to_string());
    let content_type = match ct.as_deref() {
        None | Some("") | Some("(null)") => None,
        Some(_) => ct,
    };

    Ok(RawFetch {
        final_url,
        status,
        content_type,
        body: body_bytes,
    })
}

/// Byte-level `rfind` — the last index at which `needle` occurs inside
/// `haystack`. Written directly rather than pulled in from a crate to keep
/// this crate's zero-dep posture. Not the fastest possible (a Boyer-Moore
/// variant would be); for a ~20-byte sentinel at the tail of a response
/// body, straight backward scan is fine.
pub(crate) fn rfind_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    let last = haystack.len() - needle.len();
    let mut i = last as isize;
    while i >= 0 {
        if &haystack[i as usize..i as usize + needle.len()] == needle {
            return Some(i as usize);
        }
        i -= 1;
    }
    None
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
    fn rfind_bytes_finds_the_last_occurrence() {
        // Two occurrences of the sentinel-like needle; must return the second.
        let hay = b"aaaaXXXXbbbbXXXXcccc";
        let needle = b"XXXX";
        assert_eq!(rfind_bytes(hay, needle), Some(12));
    }

    #[test]
    fn rfind_bytes_returns_none_when_absent() {
        assert_eq!(rfind_bytes(b"nothing here", b"XXXX"), None);
    }

    #[test]
    fn rfind_bytes_survives_needle_larger_than_haystack() {
        assert_eq!(rfind_bytes(b"ab", b"abcdef"), None);
    }

    #[test]
    fn rfind_bytes_preserves_arbitrary_non_utf8_prefixes() {
        // A body that includes PNG magic + non-UTF-8 bytes, followed by the
        // sentinel + ASCII meta. The split must yield the exact original
        // body bytes (including the invalid UTF-8) and the ASCII trailer.
        let mut hay: Vec<u8> = Vec::new();
        hay.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]); // PNG sig
        hay.extend_from_slice(&[0xC0, 0xC1, 0xF5]); // invalid UTF-8 continuation
        hay.extend_from_slice(META_SENTINEL.as_bytes());
        hay.extend_from_slice(b"200\n__PBE_FIELD__\nimage/png\n__PBE_FIELD__\nhttp://x");
        let idx = rfind_bytes(&hay, META_SENTINEL.as_bytes()).expect("must find sentinel");
        assert_eq!(
            &hay[..idx],
            &[137, 80, 78, 71, 13, 10, 26, 10, 0xC0, 0xC1, 0xF5]
        );
    }
}
