use super::*;
use crate::model::{CaptureId, NormalizeOptions, normalize};
use serde_json::json;
use std::io::Write;

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

fn zstd(bytes: &[u8]) -> Vec<u8> {
    zstd::stream::encode_all(bytes, 1).unwrap()
}

fn transport(encoding: &[&str], media: Option<&str>) -> BodyTransport {
    BodyTransport::from_headers(
        encoding.iter().map(|s| s.as_bytes()),
        media.map(str::as_bytes).into_iter(),
    )
}

fn body(request: &Value, names: &[&str], image: &[u8]) -> Vec<u8> {
    let mut bytes = format!("--fixture\r\nContent-Disposition: form-data; name=\"request\"\r\nContent-Type: application/json\r\n\r\n{request}\r\n").into_bytes();
    for name in names {
        bytes.extend_from_slice(format!("--fixture\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"ignored.png\"\r\nContent-Type: image/png\r\n\r\n").as_bytes());
        bytes.extend_from_slice(image);
        bytes.extend_from_slice(b"\r\n");
    }
    bytes.extend_from_slice(b"--fixture--\r\n");
    bytes
}

fn fixture_request() -> Value {
    json!({"model":"synthetic","state":{"photo":"photo"},"assets":{"photo":{"type":"image","part":"asset-0"}},
           "questions":{"q":{"type":"noul","instructions":"Inspect the example"}}})
}

fn capture(
    request: Vec<u8>,
    response: Vec<u8>,
    request_format: BodyTransport,
    response_format: BodyTransport,
    limit: usize,
) -> Capture {
    Capture {
        id: CaptureId::new_live(),
        source: "decode-test".into(),
        status: 200,
        request,
        response,
        capture_complete: true,
        transport: Some(CaptureTransport {
            request: request_format,
            response: response_format,
            limit,
        }),
        ..Default::default()
    }
}

#[test]
fn content_coding_headers_are_bounded_and_unambiguous() {
    for (values, expected) in [
        (vec![], Coding::Identity),
        (vec!["identity"], Coding::Identity),
        (vec![" GZip\t"], Coding::Gzip),
        (vec!["x-gzip"], Coding::Gzip),
        (vec!["ZSTD"], Coding::Zstd),
        (vec![""], Coding::Unsupported),
        (vec!["gzip", "gzip"], Coding::Unsupported),
        (vec!["identity", "gzip"], Coding::Unsupported),
        (vec!["gzip, identity"], Coding::Unsupported),
        (vec!["br"], Coding::Unsupported),
        (vec!["\u{a0}gzip"], Coding::Unsupported),
    ] {
        assert_eq!(transport(&values, None).coding, expected);
    }
    assert_eq!(
        transport(&[&"z".repeat(MAX_HEADER_BYTES + 1)], None).coding,
        Coding::Unsupported
    );
    let repeated = BodyTransport::from_headers(
        [].into_iter(),
        [
            b"application/json".as_slice(),
            b"application/json".as_slice(),
        ]
        .into_iter(),
    );
    assert!(matches!(repeated.media, Media::Unsupported));
}

#[test]
fn compression_decodes_exactly_one_complete_bounded_body() {
    let original = serde_json::to_vec(&json!({"text":"x".repeat(2048)})).unwrap();
    for (coding, encode) in [
        (Coding::Gzip, gzip as fn(&[u8]) -> Vec<u8>),
        (Coding::Zstd, zstd),
    ] {
        let bytes = encode(&original);
        assert_eq!(
            decode(&bytes, coding, original.len()).unwrap().as_ref(),
            original
        );
        assert!(decode(&bytes, coding, original.len() - 1).is_err());
        assert!(decode(&bytes, coding, bytes.len() - 1).is_err());
        assert!(decode(&bytes[..bytes.len() - 1], coding, original.len()).is_err());
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode(&trailing, coding, original.len()).is_err());
        let mut concatenated = bytes.clone();
        concatenated.extend_from_slice(&bytes);
        assert!(decode(&concatenated, coding, original.len()).is_err());
        assert_eq!(
            decode(&bytes, coding, original.len() + 1).unwrap().as_ref(),
            original
        );
    }
    let identity = decode(&original, Coding::Identity, original.len()).unwrap();
    assert!(matches!(identity, Cow::Borrowed(_)));
    assert!(decode(&original, Coding::Identity, 0).is_err());
    assert!(decode(&[], Coding::Identity, MAX_CAPTURE_BYTES + 1).is_err());
}

#[test]
fn compression_validates_checksums_and_zstd_window_before_acceptance() {
    let original = vec![b'a'; 2048];
    let mut bad = gzip(&original);
    let size = bad.len();
    bad[size - 8] ^= 1;
    assert!(decode(&bad, Coding::Gzip, 4096).is_err());
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 1).unwrap();
    encoder.include_checksum(true).unwrap();
    encoder.write_all(&original).unwrap();
    let mut bad = encoder.finish().unwrap();
    let size = bad.len();
    bad[size - 1] ^= 1;
    assert!(decode(&bad, Coding::Zstd, 4096).is_err());
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 1).unwrap();
    encoder.window_log(24).unwrap();
    encoder.write_all(&original).unwrap();
    let large_window = encoder.finish().unwrap();
    assert!(decode(&large_window, Coding::Zstd, 4096).is_err());
    assert!(decode(&[0x50, 0x2a, 0x4d, 0x18, 0, 0, 0, 0], Coding::Zstd, 4096).is_err());
}

#[test]
fn multipart_metadata_accepts_quoted_boundaries_and_rejects_ambiguity() {
    for value in [
        "multipart/form-data; boundary=fixture",
        "Multipart/Form-Data; boundary=\"fixture\"",
    ] {
        assert!(matches!(transport(&[],Some(value)).media,Media::Multipart(ref b) if b=="fixture"));
    }
    for value in [
        "multipart/form-data",
        "multipart/mixed; boundary=fixture",
        "multipart/form-data; boundary=fixture; boundary=other",
        "multipart/form-data; boundary=\"bad\\\"boundary\"",
        "multipart/form-data; boundary=\"trailing \"",
    ] {
        assert!(matches!(
            transport(&[], Some(value)).media,
            Media::Unsupported
        ));
    }
    assert!(matches!(
        transport(
            &[],
            Some(&format!("multipart/form-data; boundary={}", "a".repeat(70)))
        )
        .media,
        Media::Multipart(_)
    ));
    assert!(matches!(
        transport(
            &[],
            Some(&format!("multipart/form-data; boundary={}", "a".repeat(71)))
        )
        .media,
        Media::Unsupported
    ));
}

#[test]
fn multipart_inspection_retains_original_metadata_and_limits_base64() {
    let image = b"\x89PNG\r\n\x1a\nimage\r\n--fixtureXinside";
    let request = fixture_request();
    let bytes = body(&request, &["asset-0"], image);
    let omitted = multipart::request(&bytes, "fixture", 4096, false).unwrap();
    assert_eq!(
        omitted, request,
        "all original JSON remains available for secret discovery"
    );
    let retained = multipart::request(&bytes, "fixture", 4096, true).unwrap();
    assert_eq!(retained["assets"]["photo"]["part"], "asset-0");
    assert_eq!(retained["assets"]["photo"]["format"], "png");
    assert_eq!(
        retained["assets"]["photo"]["data_base64"],
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, image)
    );
    let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
    big.extend(vec![b'x'; 4096]);
    let bytes = body(&request, &["asset-0"], &big);
    assert!(multipart::request(&bytes, "fixture", bytes.len(), false).is_ok());
    assert!(multipart::request(&bytes, "fixture", bytes.len(), true).is_err());
}

#[test]
fn multipart_rejects_incomplete_ambiguous_or_unclaimed_parts() {
    let image = b"\x89PNG\r\n\x1a\nfixture";
    let request = fixture_request();
    let valid = body(&request, &["asset-0"], image);
    assert!(multipart::request(&valid[..valid.len() - 4], "fixture", 4096, false).is_err());
    assert!(
        multipart::request(
            &body(&request, &["asset-0", "asset-0"], image),
            "fixture",
            4096,
            false
        )
        .is_err()
    );
    assert!(
        multipart::request(&body(&request, &["asset-1"], image), "fixture", 4096, false).is_err()
    );
    assert!(
        multipart::request(
            &body(&request, &["asset-0", "asset-1"], image),
            "fixture",
            4096,
            false
        )
        .is_err()
    );
    let mut shared = request.clone();
    shared["assets"]["other"] = shared["assets"]["photo"].clone();
    assert!(
        multipart::request(&body(&shared, &["asset-0"], image), "fixture", 4096, false).is_err()
    );
    for (key, value) in [
        ("data_base64", json!("hidden")),
        ("format", json!("jpg")),
        ("type", json!("audio")),
    ] {
        let mut changed = request.clone();
        changed["assets"]["photo"][key] = value;
        assert!(
            multipart::request(&body(&changed, &["asset-0"], image), "fixture", 4096, true)
                .is_err()
        );
    }
    let replace = |old: &[u8], new: &[u8]| {
        let at = memchr::memmem::find(&valid, old).unwrap();
        [
            valid[..at].to_vec(),
            new.to_vec(),
            valid[at + old.len()..].to_vec(),
        ]
        .concat()
    };
    let unknown = replace(
        b"Content-Type: application/json",
        b"Content-Encoding: gzip\r\nContent-Type: application/json",
    );
    assert!(multipart::request(&unknown, "fixture", 4096, false).is_err());
    let wrong = replace(b"name=\"request\"", b"name=\"other\"");
    assert!(multipart::request(&wrong, "fixture", 4096, false).is_err());
}

#[test]
fn multipart_name_part_count_and_header_limits_are_checked() {
    let image = b"\x89PNG\r\n\x1a\nfixture";
    let names: Vec<_> = (0..128).map(|i| format!("part-{i}")).collect();
    let mut request = fixture_request();
    request["assets"] = json!({});
    for name in &names {
        request["assets"][name] = json!({"part":name});
    }
    let many = body(
        &request,
        &names.iter().map(String::as_str).collect::<Vec<_>>(),
        image,
    );
    assert!(multipart::request(&many, "fixture", 128 * 1024, false).is_err());
    let long = "n".repeat(1025);
    let mut request = fixture_request();
    request["assets"]["photo"]["part"] = json!(long);
    assert!(
        multipart::request(
            &body(&request, &[&long], image),
            "fixture",
            128 * 1024,
            false
        )
        .is_err()
    );
    let request = fixture_request();
    let valid = body(&request, &["asset-0"], image);
    let prefix = b"Content-Disposition: form-data; name=\"request\"";
    let pos = memchr::memmem::find(&valid, prefix).unwrap() + prefix.len();
    let mut large = valid[..pos].to_vec();
    large.extend_from_slice(format!("; filename=\"{}\"", "a".repeat(16 * 1024)).as_bytes());
    large.extend_from_slice(&valid[pos..]);
    assert!(multipart::request(&large, "fixture", 128 * 1024, false).is_err());
}

#[test]
fn multipart_exact_part_name_and_header_budgets_accept_then_reject() {
    let image = b"\x89PNG\r\n\x1a\nfixture";
    let names: Vec<_> = (0..127).map(|i| format!("part-{i}")).collect();
    let mut request = fixture_request();
    request["assets"] = json!({});
    for name in &names {
        request["assets"][name] = json!({"part":name});
    }
    let bytes = body(
        &request,
        &names.iter().map(String::as_str).collect::<Vec<_>>(),
        image,
    );
    assert!(multipart::request(&bytes, "fixture", 128 * 1024, false).is_ok());
    let name = "n".repeat(1024);
    let mut request = fixture_request();
    request["assets"]["photo"]["part"] = json!(name);
    assert!(
        multipart::request(
            &body(&request, &[&name], image),
            "fixture",
            128 * 1024,
            false
        )
        .is_ok()
    );

    // Total header accounting is independent of the per-part bound. Padding
    // is a quoted, ignored filename, so otherwise valid JSON/images stay fixed.
    fn padded(sizes: &[usize]) -> Vec<u8> {
        let names: Vec<_> = (1..sizes.len()).map(|i| format!("p{i}")).collect();
        let mut assets = json!({});
        for name in &names {
            assets[name] = json!({"part":name});
        }
        let request = serde_json::to_vec(&json!({"assets":assets})).unwrap();
        let mut bytes = Vec::new();
        for (i, &size) in sizes.iter().enumerate() {
            let name = if i == 0 { "request" } else { &names[i - 1] };
            let media = if i == 0 {
                "application/json"
            } else {
                "image/png"
            };
            let header = format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"\"\r\nContent-Type: {media}"
            );
            let padding = "x".repeat(size - header.len());
            let header = header.replacen("filename=\"\"", &format!("filename=\"{padding}\""), 1);
            assert_eq!(header.len(), size);
            bytes.extend_from_slice(format!("--fixture\r\n{header}\r\n\r\n").as_bytes());
            bytes.extend_from_slice(if i == 0 {
                &request
            } else {
                b"\x89PNG\r\n\x1a\n"
            });
            bytes.extend_from_slice(b"\r\n");
        }
        bytes.extend_from_slice(b"--fixture--\r\n");
        bytes
    }
    assert!(multipart::request(&padded(&[16_384; 4]), "fixture", 128 * 1024, false).is_ok());
    assert!(
        multipart::request(
            &padded(&[16_384, 16_384, 16_384, 16_284, 101]),
            "fixture",
            128 * 1024,
            false
        )
        .is_err()
    );
    assert!(multipart::request(&padded(&[16_385]), "fixture", 128 * 1024, false).is_err());
}

#[test]
fn compressed_multipart_and_response_pass_original_secret_discovery() {
    let mut request = fixture_request();
    request["questions"]["q"]["instructions"] = json!("asset-0");
    let raw = body(&request, &["asset-0"], b"\x89PNG\r\n\x1a\nfixture");
    let response =
        serde_json::to_vec(&json!({"answers":{"q":{"type":"noul","noul":0.75}},"echo":"asset-0"}))
            .unwrap();
    for (coding, encode) in [("gzip", gzip as fn(&[u8]) -> Vec<u8>), ("zstd", zstd)] {
        let capture = capture(
            encode(&raw),
            encode(&response),
            transport(&[coding], Some("multipart/form-data; boundary=fixture")),
            transport(&[coding], None),
            4096,
        );
        let record = normalize(
            &capture,
            &NormalizeOptions {
                capture_state: true,
                redact_keys: vec!["part".into()],
                ..Default::default()
            },
        );
        assert_eq!(record["capture_complete"], true);
        assert_eq!(record["response_extra"]["echo"], "[REDACTED]");
        assert!(!record.to_string().contains("asset-0"));
        assert_eq!(
            capture.request,
            encode(&raw),
            "captured wire bytes are untouched"
        );
    }
}

#[test]
fn decode_failures_use_static_minimal_fallback_and_legacy_json_stays_compatible() {
    let capture = capture(
        gzip(&vec![b'x'; 4096]),
        b"{}".to_vec(),
        transport(&["gzip"], None),
        transport(&[], None),
        1024,
    );
    let record = normalize(&capture, &NormalizeOptions::default());
    assert_eq!(record["capture_complete"], false);
    assert_eq!(record["normalization_error"], DECODE_ERROR_MESSAGE);
    assert!(!record.to_string().contains("decode-test"));
    let mut legacy = Capture {
        request: b"not JSON".to_vec(),
        response: b"{}".to_vec(),
        ..Default::default()
    };
    assert_eq!(
        decoded_values(&legacy, false).unwrap(),
        (Value::Null, json!({}))
    );
    legacy.transport = Some(CaptureTransport::new(transport(&[], None), 1024));
    assert_eq!(
        decoded_values(&legacy, false).unwrap(),
        (Value::Null, json!({}))
    );
}
