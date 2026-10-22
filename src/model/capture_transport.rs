//! Inspect bounded captured copies after forwarding. No network or file access.
//!
//! Decoding runs in the single collector writer, while its capture permit is
//! still owned. Output limits do not include the codec context or JSON tree.

mod multipart;

use super::types::Capture;
use serde_json::Value;
use std::borrow::Cow;
use std::io::Read;

const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 2048;
pub(crate) const DECODE_ERROR_MESSAGE: &str =
    "Captured body could not be decoded within the inspection limits";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CaptureDecodeError;

impl std::fmt::Display for CaptureDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(DECODE_ERROR_MESSAGE)
    }
}
impl std::error::Error for CaptureDecodeError {}

type Result<T> = std::result::Result<T, CaptureDecodeError>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Coding {
    #[default]
    Identity,
    Gzip,
    Zstd,
    Unsupported,
}

#[derive(Clone, Debug, Default)]
enum Media {
    #[default]
    Json,
    Multipart(String),
    Unsupported,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BodyTransport {
    coding: Coding,
    media: Media,
}

/// Header facts needed to inspect a live bounded body. Never serialized.
#[derive(Clone, Debug)]
pub struct CaptureTransport {
    pub(crate) request: BodyTransport,
    pub(crate) response: BodyTransport,
    limit: usize,
}

impl CaptureTransport {
    pub(crate) fn new(request: BodyTransport, limit: usize) -> Self {
        Self {
            request,
            response: BodyTransport::default(),
            limit,
        }
    }
}

impl BodyTransport {
    /// Only a bounded multipart boundary is retained from raw header values.
    pub(crate) fn from_headers<'a>(
        encoding: impl Iterator<Item = &'a [u8]>,
        content_type: impl Iterator<Item = &'a [u8]>,
    ) -> Self {
        fn single<'a>(mut values: impl Iterator<Item = &'a [u8]>) -> Result<Option<&'a str>> {
            let first = values.next();
            if values.next().is_some() {
                return Err(CaptureDecodeError);
            }
            first
                .map(|bytes| {
                    if bytes.len() > MAX_HEADER_BYTES {
                        return Err(CaptureDecodeError);
                    }
                    std::str::from_utf8(bytes).map_err(|_| CaptureDecodeError)
                })
                .transpose()
        }
        let coding = match single(encoding) {
            Ok(None) => Coding::Identity,
            Ok(Some(value))
                if value
                    .trim_matches([' ', '\t'])
                    .eq_ignore_ascii_case("identity") =>
            {
                Coding::Identity
            }
            Ok(Some(value))
                if value.trim_matches([' ', '\t']).eq_ignore_ascii_case("gzip")
                    || value
                        .trim_matches([' ', '\t'])
                        .eq_ignore_ascii_case("x-gzip") =>
            {
                Coding::Gzip
            }
            Ok(Some(value)) if value.trim_matches([' ', '\t']).eq_ignore_ascii_case("zstd") => {
                Coding::Zstd
            }
            _ => Coding::Unsupported,
        };
        let media = match single(content_type) {
            Ok(None) => Media::Json,
            Ok(Some(value)) => match multipart::media(value) {
                Ok(Some(boundary)) => Media::Multipart(boundary),
                Ok(None) => Media::Json,
                Err(_) => Media::Unsupported,
            },
            Err(_) => Media::Unsupported,
        };
        Self { coding, media }
    }
}

fn bounded_read(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        // Read one extra byte only into fixed scratch space to detect overflow.
        let remaining = limit - output.len();
        let count = reader
            .read(&mut chunk[..8192.min(remaining + 1)])
            .map_err(|_| CaptureDecodeError)?;
        if count == 0 {
            return Ok(output);
        }
        if count > remaining {
            return Err(CaptureDecodeError);
        }
        output
            .try_reserve_exact(count)
            .map_err(|_| CaptureDecodeError)?;
        output.extend_from_slice(&chunk[..count]);
    }
}

fn decode(bytes: &[u8], coding: Coding, limit: usize) -> Result<Cow<'_, [u8]>> {
    if limit == 0 || limit > MAX_CAPTURE_BYTES || bytes.len() > limit {
        return Err(CaptureDecodeError);
    }
    match coding {
        Coding::Identity => Ok(Cow::Borrowed(bytes)),
        Coding::Unsupported => Err(CaptureDecodeError),
        Coding::Gzip => {
            // bufread consumes one member exactly; trailer validation happens
            // while reading to EOF. Additional members/garbage are unsupported.
            let mut decoder = flate2::bufread::GzDecoder::new(bytes);
            let output = bounded_read(&mut decoder, limit)?;
            if !decoder.get_ref().is_empty() {
                return Err(CaptureDecodeError);
            }
            Ok(Cow::Owned(output))
        }
        Coding::Zstd => {
            // Do not accept skippable/magicless frames or a prefix before one.
            if !bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
                return Err(CaptureDecodeError);
            }
            let mut decoder = zstd::stream::read::Decoder::with_buffer(bytes)
                .map_err(|_| CaptureDecodeError)?
                .single_frame();
            decoder.window_log_max(23).map_err(|_| CaptureDecodeError)?;
            let output = bounded_read(&mut decoder, limit)?;
            if !decoder.finish().is_empty() {
                return Err(CaptureDecodeError);
            }
            Ok(Cow::Owned(output))
        }
    }
}

fn value(
    bytes: &[u8],
    transport: &BodyTransport,
    limit: usize,
    capture_state: bool,
    request: bool,
) -> Result<Value> {
    let decoded = decode(bytes, transport.coding, limit)?;
    match &transport.media {
        Media::Unsupported => Err(CaptureDecodeError),
        Media::Multipart(boundary) if request => {
            multipart::request(&decoded, boundary, limit, capture_state)
        }
        Media::Multipart(_) => Err(CaptureDecodeError),
        Media::Json if transport.coding == Coding::Identity => {
            // Preserve existing identity-body behavior for empty/error bodies
            // and unparseable ordinary JSON; normal validation describes them.
            Ok(serde_json::from_slice(&decoded).unwrap_or(Value::Null))
        }
        Media::Json => serde_json::from_slice(&decoded).map_err(|_| CaptureDecodeError),
    }
}

/// Original JSON fields survive until the existing secret discovery/pruning.
/// Multipart opt-in adds inline bytes but keeps the original part provenance;
/// the retained inspection copy is not a guaranteed replay request.
pub(crate) fn decoded_values(capture: &Capture, capture_state: bool) -> Result<(Value, Value)> {
    let Some(transport) = &capture.transport else {
        return Ok((
            serde_json::from_slice(&capture.request).unwrap_or(Value::Null),
            serde_json::from_slice(&capture.response).unwrap_or(Value::Null),
        ));
    };
    let request = value(
        &capture.request,
        &transport.request,
        transport.limit,
        capture_state,
        true,
    )?;
    let response = value(
        &capture.response,
        &transport.response,
        transport.limit,
        capture_state,
        false,
    )?;
    Ok((request, response))
}

#[cfg(test)]
#[path = "capture_transport/tests.rs"]
mod tests;
