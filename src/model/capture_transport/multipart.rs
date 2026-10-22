//! A bounded inspection subset of multipart/form-data; never a forwarding parser.

use super::{CaptureDecodeError, Result};
use base64::Engine;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const PARTS: usize = 128;
const HEADER_BYTES: usize = 16 * 1024;
const TOTAL_HEADER_BYTES: usize = 64 * 1024;
const NAME_BYTES: usize = 1024;

fn token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn parameters(value: &str) -> Result<(&str, BTreeMap<String, String>)> {
    let (kind, tail) = value.split_once(';').unwrap_or((value, ""));
    let kind = kind.trim_matches([' ', '\t']);
    let mut bytes = tail.as_bytes();
    let mut result = BTreeMap::new();
    while !bytes.is_empty() {
        while bytes.first().is_some_and(|b| matches!(b, b' ' | b'\t')) {
            bytes = &bytes[1..];
        }
        let size = bytes.iter().take_while(|&&b| token(b)).count();
        if size == 0 || result.len() == 16 {
            return Err(CaptureDecodeError);
        }
        let name = std::str::from_utf8(&bytes[..size])
            .map_err(|_| CaptureDecodeError)?
            .to_ascii_lowercase();
        bytes = &bytes[size..];
        while bytes.first().is_some_and(|b| matches!(b, b' ' | b'\t')) {
            bytes = &bytes[1..];
        }
        if bytes.first() != Some(&b'=') {
            return Err(CaptureDecodeError);
        }
        bytes = &bytes[1..];
        while bytes.first().is_some_and(|b| matches!(b, b' ' | b'\t')) {
            bytes = &bytes[1..];
        }
        let mut content = Vec::new();
        if bytes.first() == Some(&b'"') {
            bytes = &bytes[1..];
            loop {
                let (&byte, rest) = bytes.split_first().ok_or(CaptureDecodeError)?;
                bytes = rest;
                if byte == b'"' {
                    break;
                }
                let byte = if byte == b'\\' {
                    let (&escaped, rest) = bytes.split_first().ok_or(CaptureDecodeError)?;
                    bytes = rest;
                    escaped
                } else {
                    byte
                };
                if byte < 0x20 || byte == 0x7f {
                    return Err(CaptureDecodeError);
                }
                content.push(byte);
            }
        } else {
            let size = bytes.iter().take_while(|&&b| token(b)).count();
            if size == 0 {
                return Err(CaptureDecodeError);
            }
            content.extend_from_slice(&bytes[..size]);
            bytes = &bytes[size..];
        }
        let content = String::from_utf8(content).map_err(|_| CaptureDecodeError)?;
        if result.insert(name, content).is_some() {
            return Err(CaptureDecodeError);
        }
        while bytes.first().is_some_and(|b| matches!(b, b' ' | b'\t')) {
            bytes = &bytes[1..];
        }
        if !bytes.is_empty() {
            if bytes[0] != b';' {
                return Err(CaptureDecodeError);
            }
            bytes = &bytes[1..];
            if bytes.is_empty() {
                return Err(CaptureDecodeError);
            }
        }
    }
    Ok((kind, result))
}

pub(super) fn media(value: &str) -> Result<Option<String>> {
    let kind = value
        .split(';')
        .next()
        .unwrap_or("")
        .trim_matches([' ', '\t']);
    // Nonmultipart responses retain the previous JSON-or-null inspection rule.
    if !kind.to_ascii_lowercase().starts_with("multipart/") {
        return Ok(None);
    }
    let (kind, mut fields) = parameters(value)?;
    if !kind.eq_ignore_ascii_case("multipart/form-data") {
        return Err(CaptureDecodeError);
    }
    let boundary = fields.remove("boundary").ok_or(CaptureDecodeError)?;
    if boundary.is_empty()
        || boundary.len() > 70
        || boundary.ends_with(' ')
        || !boundary
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"'()+_,-./:=? ".contains(&b))
    {
        return Err(CaptureDecodeError);
    }
    Ok(Some(boundary))
}

struct Part<'a> {
    name: String,
    media: Option<String>,
    body: &'a [u8],
}

fn part_headers(bytes: &[u8]) -> Result<(String, Option<String>)> {
    let text = std::str::from_utf8(bytes).map_err(|_| CaptureDecodeError)?;
    let mut disposition = None;
    let mut media = None;
    for line in text.split("\r\n") {
        let (name, value) = line.split_once(':').ok_or(CaptureDecodeError)?;
        if name.is_empty() || !name.bytes().all(token) {
            return Err(CaptureDecodeError);
        }
        let value = value.trim_matches([' ', '\t']);
        if name.eq_ignore_ascii_case("content-disposition") {
            if disposition.replace(value).is_some() {
                return Err(CaptureDecodeError);
            }
        } else if name.eq_ignore_ascii_case("content-type") {
            if media.is_some() {
                return Err(CaptureDecodeError);
            }
            media = Some(parameters(value)?.0.to_ascii_lowercase());
        } else {
            // Per-part encodings and unknown header semantics are not guessed.
            return Err(CaptureDecodeError);
        }
    }
    let (kind, mut fields) = parameters(disposition.ok_or(CaptureDecodeError)?)?;
    if !kind.eq_ignore_ascii_case("form-data") {
        return Err(CaptureDecodeError);
    }
    let name = fields.remove("name").ok_or(CaptureDecodeError)?;
    if name.trim().is_empty() || name.len() > NAME_BYTES {
        return Err(CaptureDecodeError);
    }
    // Filename parameters are bounded header text, never paths or stored data.
    Ok((name, media))
}

fn parts<'a>(bytes: &'a [u8], boundary: &str) -> Result<Vec<Part<'a>>> {
    let first = format!("--{boundary}\r\n");
    let delimiter = format!("\r\n--{boundary}");
    let mut offset = first.len();
    if !bytes.starts_with(first.as_bytes()) {
        return Err(CaptureDecodeError);
    }
    let mut result = Vec::new();
    let mut names = BTreeSet::new();
    let mut header_work = 0;
    loop {
        if result.len() == PARTS {
            return Err(CaptureDecodeError);
        }
        let end = bytes.len().min(offset.saturating_add(HEADER_BYTES + 4));
        let size =
            memchr::memmem::find(&bytes[offset..end], b"\r\n\r\n").ok_or(CaptureDecodeError)?;
        header_work += size;
        if size > HEADER_BYTES || header_work > TOTAL_HEADER_BYTES {
            return Err(CaptureDecodeError);
        }
        let (name, media) = part_headers(&bytes[offset..offset + size])?;
        if !names.insert(name.clone()) {
            return Err(CaptureDecodeError);
        }
        let start = offset + size + 4;
        let mut search = start;
        let (body_end, next, final_part) = loop {
            let found = memchr::memmem::find(&bytes[search..], delimiter.as_bytes())
                .ok_or(CaptureDecodeError)?
                + search;
            let suffix = found + delimiter.len();
            let remainder = &bytes[suffix..];
            if remainder.starts_with(b"\r\n") {
                break (found, suffix + 2, false);
            }
            if remainder.starts_with(b"--")
                && (remainder.len() == 2 || remainder[2..].starts_with(b"\r\n"))
            {
                break (found, suffix + 2, true);
            }
            // Boundary-like bytes with an invalid delimiter suffix are data.
            search = found + delimiter.len();
        };
        result.push(Part {
            name,
            media,
            body: &bytes[start..body_end],
        });
        if final_part {
            if !matches!(&bytes[next..], b"" | b"\r\n") {
                return Err(CaptureDecodeError);
            }
            return Ok(result);
        }
        offset = next;
    }
}

fn image_format(bytes: &[u8]) -> Result<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok("png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Ok("jpg")
    } else {
        Err(CaptureDecodeError)
    }
}

pub(super) fn request(
    bytes: &[u8],
    boundary: &str,
    limit: usize,
    capture_state: bool,
) -> Result<Value> {
    let parts = parts(bytes, boundary)?;
    let first = parts.first().ok_or(CaptureDecodeError)?;
    if first.name != "request" || first.media.as_deref() != Some("application/json") {
        return Err(CaptureDecodeError);
    }
    let mut request: Value = serde_json::from_slice(first.body).map_err(|_| CaptureDecodeError)?;
    if !request.is_object() {
        return Err(CaptureDecodeError);
    }
    let mut retained_size = super::super::budget::json_size(&request, limit, "capture decoding")
        .map_err(|_| CaptureDecodeError)?;
    let files: BTreeMap<_, _> = parts
        .iter()
        .skip(1)
        .map(|part| (part.name.as_str(), part))
        .collect();
    let mut claimed = BTreeSet::new();
    if files.is_empty() && request.get("assets").is_none() {
        return Ok(request);
    }
    let assets = request
        .get_mut("assets")
        .and_then(Value::as_object_mut)
        .ok_or(CaptureDecodeError)?;
    for asset in assets.values_mut() {
        let entry = asset.as_object_mut().ok_or(CaptureDecodeError)?;
        let Some(name) = entry.get("part") else {
            continue;
        };
        let name = name.as_str().ok_or(CaptureDecodeError)?;
        let part = *files.get(name).ok_or(CaptureDecodeError)?;
        if !claimed.insert(name.to_owned()) {
            return Err(CaptureDecodeError);
        }
        if entry.get("type").is_some_and(|v| v != "image") || entry.contains_key("data_base64") {
            return Err(CaptureDecodeError);
        }
        let format = image_format(part.body)?;
        if entry
            .get("format")
            .is_some_and(|value| value != format && !(format == "jpg" && value == "jpeg"))
        {
            return Err(CaptureDecodeError);
        }
        if part.media.as_deref().is_some_and(|media| {
            !matches!(
                media,
                "application/octet-stream" | "image/png" | "image/jpg" | "image/jpeg"
            )
        }) {
            return Err(CaptureDecodeError);
        }
        if part.media.as_deref().is_some_and(|media| {
            media == "image/png" && format != "png"
                || matches!(media, "image/jpg" | "image/jpeg") && format != "jpg"
        }) {
            return Err(CaptureDecodeError);
        }
        if capture_state {
            let encoded = part
                .body
                .len()
                .checked_add(2)
                .and_then(|n| n.checked_div(3))
                .and_then(|n| n.checked_mul(4))
                .ok_or(CaptureDecodeError)?;
            // Conservative accounting before allocating any base64 string.
            retained_size = retained_size
                .checked_add(encoded)
                .and_then(|n| n.checked_add(40))
                .filter(|&n| n <= limit)
                .ok_or(CaptureDecodeError)?;
            entry
                .entry("format")
                .or_insert_with(|| Value::String(format.into()));
            entry.insert(
                "data_base64".into(),
                Value::String(base64::engine::general_purpose::STANDARD.encode(part.body)),
            );
            // Keep original part metadata through privacy secret discovery.
        }
    }
    if claimed.len() != files.len() {
        return Err(CaptureDecodeError);
    }
    Ok(request)
}
