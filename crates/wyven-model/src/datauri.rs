//! `data:` URI decoding.
//!
//! Both supported model formats embed their binary payloads as RFC 2397 data
//! URIs — glTF puts its vertex buffer and its PNG there, Blockbench puts the
//! texture there — so a Blockbench export is a single self-contained file. The
//! same helper resolves the sidecar case (a plain relative path) by reporting
//! that the URI is *not* inline, leaving the caller to read the file.

use base64::prelude::*;

/// What a glTF/bbmodel `uri` field points at.
pub enum Uri<'a> {
    /// Bytes carried inline in the URI itself.
    Inline(Vec<u8>),
    /// A path relative to the file that named it.
    Relative(&'a str),
}

/// Classify a `uri` field, decoding it when it is an inline `data:` URI.
///
/// Only base64 payloads are decoded: percent-encoded `data:` URIs are legal
/// glTF but no exporter emits them, and silently mis-decoding one would produce
/// corrupt geometry rather than an error.
pub fn parse(uri: &str) -> Result<Uri<'_>, String> {
    let Some(rest) = uri.strip_prefix("data:") else {
        return Ok(Uri::Relative(uri));
    };
    let (meta, payload) = rest
        .split_once(',')
        .ok_or_else(|| "data: URI has no comma separating metadata from payload".to_string())?;
    if !meta.ends_with(";base64") {
        return Err(format!(
            "unsupported data: URI encoding {meta:?} (need base64)"
        ));
    }
    let bytes = decode(payload)?;
    Ok(Uri::Inline(bytes))
}

/// Decode a standard-alphabet base64 payload.
pub fn decode(payload: &str) -> Result<Vec<u8>, String> {
    BASE64_STANDARD
        .decode(payload.trim())
        .map_err(|err| format!("invalid base64: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_an_inline_base64_payload() {
        let uri = "data:application/octet-stream;base64,aGVsbG8=";
        match parse(uri).expect("valid data URI") {
            Uri::Inline(bytes) => assert_eq!(bytes, b"hello"),
            Uri::Relative(_) => panic!("should have decoded inline"),
        }
    }

    #[test]
    fn a_plain_path_is_relative() {
        match parse("vine.png").expect("valid uri") {
            Uri::Relative(path) => assert_eq!(path, "vine.png"),
            Uri::Inline(_) => panic!("should not have decoded"),
        }
    }

    #[test]
    fn rejects_malformed_data_uris() {
        assert!(parse("data:image/png;base64").is_err(), "no comma");
        assert!(parse("data:image/png,%FF%00").is_err(), "not base64");
        assert!(parse("data:image/png;base64,!!!!").is_err(), "bad payload");
    }

    #[test]
    fn decoded_png_payloads_keep_their_magic() {
        // The first bytes of any PNG, base64-encoded.
        let bytes = decode("iVBORw0KGgo=").expect("valid base64");
        assert_eq!(&bytes[..4], b"\x89PNG");
    }
}
