//! # pbe-net
//!
//! The **network on-ramp**: fetch a page so the engine can render the live web,
//! not just local files — done the composition way.
//!
//! ## Doctrine: drive a sealed binary, link nothing
//!
//! We do **not** link an HTTP/TLS stack into the engine. A crypto/HTTP crate is
//! a large, mutable, transitively-sprawling attack surface — the exact thing the
//! composition doctrine exists to eliminate. Instead we drive the **sealed
//! system `curl` binary** from outside via [`std::process`], the same way the
//! Spiderweb bus reaches encrypted transport by composing the system `ssh`
//! rather than reimplementing crypto ("crypto is a real wall for a zero-dep
//! crate"). `curl` ships in Windows System32 and on every Unix.
//!
//! What this buys, in security terms:
//! - **Zero linked network/crypto dependencies.** This crate's only dependency
//!   is the kit's `cap-http` *type contract*. `cargo` pulls in no TLS, no
//!   parser, no async runtime — nothing to audit, nothing that can be a linked
//!   CVE in our address space.
//! - **Process isolation.** The network touches a separate, sealed process; the
//!   engine only ever sees bytes handed back over a pipe. The fetch boundary is
//!   an OS process boundary, not a function call into mutable foreign code.
//! - **Immutable inputs/outputs.** A [`FetchedPage`] is a plain owned value; the
//!   primitive is a pure `url -> Result<FetchedPage>` with no shared state.
//!
//! No policy lives here — which URL, whether to allow non-HTTPS, etc. is the
//! orchestrator's decision.

use std::process::Command;

pub use cap_http::HttpError;

/// A page fetched from the network — the fetch stage's concrete, immutable output.
#[derive(Clone, Debug)]
pub struct FetchedPage {
    /// The URL actually loaded (after redirects).
    pub final_url: String,
    /// HTTP status code.
    pub status: u16,
    /// `Content-Type`, if reported.
    pub content_type: Option<String>,
    /// The response body, decoded as UTF-8 (lossy).
    pub body: String,
}

/// Sentinel that separates the response body from the trailing metadata block
/// `curl -w` appends. Chosen to be vanishingly unlikely to occur in real HTML.
const META_SENTINEL: &str = "\n__PBE_NET_META__\n";
/// Field separator inside the metadata block. A newline is safe because none of
/// the captured fields (status code, content-type, URL) contain a newline —
/// whereas content-type DOES contain spaces and `;` (e.g. "text/html; charset=utf-8"),
/// so a space-separated format would mis-parse.
const META_FIELD_SEP: &str = "\n__PBE_FIELD__\n";

/// Default network timeout (seconds) handed to the sealed `curl`.
const TIMEOUT_SECS: u32 = 20;
/// Max redirects `curl` will follow.
const MAX_REDIRECTS: u32 = 5;

/// Fetch a URL by driving the sealed system `curl`. Pure mechanism: a single
/// `url -> Result<FetchedPage>` with no shared or mutable engine state.
///
/// Security posture: HTTPS is enforced by [`curl`]'s own (sealed, OS-maintained)
/// TLS — we never link or touch crypto. Only `http`/`https` schemes are allowed;
/// anything else is rejected before a process is spawned (no `file://`, no
/// `scp://`, etc. reaching curl).
pub fn fetch(url: &str) -> Result<FetchedPage, HttpError> {
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

    let stdout = String::from_utf8_lossy(&output.stdout);
    let (body, meta) = match stdout.rsplit_once(META_SENTINEL) {
        Some((b, m)) => (b.to_string(), m),
        None => {
            return Err(HttpError::Connection(
                "curl output missing metadata sentinel".into(),
            ))
        }
    };

    // meta = "<code><sep><content_type><sep><effective_url>" (newline-delimited).
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
        // curl prints "(null)" / empty when the server sent no content-type.
        None | Some("") | Some("(null)") => None,
        Some(_) => ct,
    };

    Ok(FetchedPage {
        final_url,
        status,
        content_type,
        body,
    })
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

    // A live network fetch is covered by an ignored integration test in
    // pbe-stages (run explicitly with `--ignored`), so the default test run
    // stays hermetic and offline.
}
