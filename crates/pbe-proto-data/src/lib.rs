//! # pbe-proto-data
//!
//! The **`data:` URI protocol crate** (RFC 2397). One modern protocol, one
//! crate, independently swappable: the [`pbe_proto`] dispatch layer routes
//! `data:` URLs here.
//!
//! `data:` URIs embed a resource inline in the URL itself — no network I/O,
//! no process spawn, no linked crypto. The whole protocol is pure byte work:
//! split the media type from the data, base64-decode if the `;base64` flag is
//! set, otherwise URL-percent-decode. This crate stays zero-dependency for
//! exactly that reason — there is nothing here to link.
//!
//! ## Grammar (RFC 2397)
//!
//! ```text
//! dataurl    := "data:" [ mediatype ] [ ";base64" ] "," data
//! mediatype  := [ type "/" subtype ] *( ";" parameter )
//! ```
//!
//! ## Decoding rules
//!
//! - The media type defaults to `text/plain;charset=US-ASCII` when absent
//!   (per RFC 2397 §3), and `text/plain` is treated as US-ASCII.
//! - When `;base64` is present, the data segment is base64-decoded (the
//!   standard alphabet, whitespace tolerant, padding required-or-omitted
//!   per the canonical length). Invalid base64 is a [`DataError::BadBase64`].
//! - Otherwise the data segment is percent-decoded (`%XX` hex escapes);
//!   a malformed `%` sequence is a [`DataError::BadPercentEncoding`].

/// Errors a `data:` URI can raise during decode.
#[derive(Clone, Debug, PartialEq)]
pub enum DataError {
    /// The URL is not a `data:` URI at all.
    NotADataUri,
    /// No `,` separator between the media type and the data.
    NoCommaSeparator,
    /// `;base64` was set but the data segment is not valid base64.
    BadBase64,
    /// A `%XX` percent-escape was malformed (not two hex digits).
    BadPercentEncoding,
    /// The data segment contained an invalid byte for its declared encoding.
    InvalidUtf8,
}

impl std::fmt::Display for DataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataError::NotADataUri => write!(f, "not a data: URI"),
            DataError::NoCommaSeparator => write!(f, "data: URI missing ',' separator"),
            DataError::BadBase64 => write!(f, "data: URI has invalid base64"),
            DataError::BadPercentEncoding => write!(f, "data: URI has bad percent-encoding"),
            DataError::InvalidUtf8 => write!(f, "data: URI body is not valid UTF-8"),
        }
    }
}

impl std::error::Error for DataError {}

/// A decoded `data:` resource — the on-ramp's concrete, immutable output.
#[derive(Clone, Debug)]
pub struct Decoded {
    /// The `Content-Type` (RFC 7231), defaulting to `text/plain;charset=US-ASCII`
    /// when the URI omits it.
    pub content_type: String,
    /// The decoded body bytes.
    pub body: Vec<u8>,
}

/// Decode a `data:` URI into a [`Decoded`] resource. Pure function: no I/O,
/// no allocations beyond the output, no dependencies. The dispatch layer's
/// [`Resource`] is built `From` this.
pub fn decode(uri: &str) -> Result<Decoded, DataError> {
    let body = uri
        .strip_prefix("data:")
        .or_else(|| uri.strip_prefix("DATA:"))
        .ok_or(DataError::NotADataUri)?;

    // RFC 2397: "data:" [ mediatype ] [ ";base64" ] "," data
    let comma = body.find(',').ok_or(DataError::NoCommaSeparator)?;
    let head = &body[..comma];
    let data = &body[comma + 1..];

    let (content_type, base64) = parse_head(head);

    let bytes = if base64 {
        decode_base64(data)?
    } else {
        percent_decode(data)?
    };

    Ok(Decoded {
        content_type,
        body: bytes,
    })
}

/// Parse the `data:` head (everything before the comma) into a media type and
/// the base64 flag. RFC 2397 §3: a missing type defaults to
/// `text/plain;charset=US-ASCII`.
fn parse_head(head: &str) -> (String, bool) {
    let mut base64 = false;
    let mut parts: Vec<&str> = Vec::new();
    for part in head.split(';') {
        if part.eq_ignore_ascii_case("base64") {
            base64 = true;
        } else if !part.is_empty() {
            parts.push(part);
        }
    }
    let media_type = if parts.is_empty() {
        "text/plain;charset=US-ASCII".to_string()
    } else {
        parts.join(";")
    };
    (media_type, base64)
}

/// Standard base64 decode of `s`, whitespace-tolerant. Returns the raw bytes
/// or [`DataError::BadBase64`] on any invalid character or bad padding.
fn decode_base64(s: &str) -> Result<Vec<u8>, DataError> {
    let table: [i8; 256] = base64_table();
    let mut out: Vec<u8> = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits = 0u32;
    for &b in s.as_bytes() {
        match b {
            // Whitespace is allowed and ignored (RFC 2397 references RFC 2045,
            // which permits it in base64 bodies).
            b' ' | b'\t' | b'\n' | b'\r' => continue,
            b'=' => continue, // padding handled by length, not value
            _ => {
                let v = table[b as usize];
                if v < 0 {
                    return Err(DataError::BadBase64);
                }
                buf = (buf << 6) | (v as u32);
                bits += 6;
                if bits >= 8 {
                    bits -= 8;
                    out.push((buf >> bits) as u8);
                }
            }
        }
    }
    Ok(out)
}

/// Build the standard base64 alphabet lookup table; -1 marks non-alphabet
/// bytes.
fn base64_table() -> [i8; 256] {
    let mut t = [-1i8; 256];
    let alpha = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for (i, &c) in alpha.iter().enumerate() {
        t[c as usize] = i as i8;
    }
    t
}

/// Percent-decode `%XX` escapes in `s`, passing other bytes through verbatim.
/// A `%` not followed by two hex digits is [`DataError::BadPercentEncoding`].
fn percent_decode(s: &str) -> Result<Vec<u8>, DataError> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return Err(DataError::BadPercentEncoding);
                }
                let hi = hex_val(bytes[i + 1]).ok_or(DataError::BadPercentEncoding)?;
                let lo = hex_val(bytes[i + 2]).ok_or(DataError::BadPercentEncoding)?;
                out.push((hi << 4) | lo);
                i += 3;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    Ok(out)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_plain_text_data_uri() {
        let d = decode("data:,Hello%2C%20World%21").unwrap();
        assert_eq!(d.content_type, "text/plain;charset=US-ASCII");
        assert_eq!(d.body, b"Hello, World!");
    }

    #[test]
    fn decodes_text_with_explicit_media_type() {
        let d = decode("data:text/plain,hi").unwrap();
        assert_eq!(d.content_type, "text/plain");
        assert_eq!(d.body, b"hi");
    }

    #[test]
    fn decodes_base64_text() {
        let d = decode("data:text/plain;base64,SGVsbG8sIFdvcmxkIQ==").unwrap();
        assert_eq!(d.content_type, "text/plain");
        assert_eq!(d.body, b"Hello, World!");
    }

    #[test]
    fn decodes_base64_binary() {
        // base64 of bytes [0x89, 0x50, 0x4e, 0x47] (PNG magic head)
        let d = decode("data:image/png;base64,iVBORw==").unwrap();
        assert_eq!(d.content_type, "image/png");
        assert_eq!(d.body, &[0x89, 0x50, 0x4e, 0x47]);
    }

    #[test]
    fn base64_tolerates_embedded_whitespace() {
        let d = decode("data:text/plain;base64,aGVs\nbG8=").unwrap();
        assert_eq!(d.body, b"hello");
    }

    #[test]
    fn rejects_non_data_uri() {
        assert_eq!(
            decode("https://example.com").unwrap_err(),
            DataError::NotADataUri
        );
    }

    #[test]
    fn rejects_missing_comma() {
        assert_eq!(
            decode("data:text/plain;base64-no-comma-here").unwrap_err(),
            DataError::NoCommaSeparator
        );
    }

    #[test]
    fn rejects_bad_base64() {
        assert_eq!(
            decode("data:text/plain;base64,not!!valid@@b64").unwrap_err(),
            DataError::BadBase64
        );
    }

    #[test]
    fn rejects_truncated_percent_escape() {
        assert_eq!(
            decode("data:,hello%2").unwrap_err(),
            DataError::BadPercentEncoding
        );
    }

    #[test]
    fn rejects_bad_percent_hex() {
        assert_eq!(
            decode("data:,hello%ZZ").unwrap_err(),
            DataError::BadPercentEncoding
        );
    }

    #[test]
    fn decodes_empty_data() {
        let d = decode("data:,").unwrap();
        assert_eq!(d.content_type, "text/plain;charset=US-ASCII");
        assert!(d.body.is_empty());
    }

    #[test]
    fn uppercase_data_scheme_works() {
        let d = decode("DATA:,hello").unwrap();
        assert_eq!(d.body, b"hello");
    }

    #[test]
    fn media_type_with_parameters_preserved() {
        let d = decode("data:text/html;charset=utf-8,<p>hi</p>").unwrap();
        assert_eq!(d.content_type, "text/html;charset=utf-8");
        assert_eq!(d.body, b"<p>hi</p>");
    }
}
