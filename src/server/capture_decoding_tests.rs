use super::*;
use std::io::Write;

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

fn zstd(bytes: &[u8]) -> Vec<u8> {
    zstd::stream::encode_all(bytes, 1).unwrap()
}

#[tokio::test]
async fn bounded_decoding_preserves_exact_request_response_bytes_and_headers() {
    let plain_request=br#"{ "state":{"password":"discarded-state-secret"},"questions":{"q":{"type":"noul","instructions":"Evaluate"}},"echo":"discarded-state-secret","ordinary":"retained" }"#;
    let plain_response=br#"{ "answers":{"q":{"type":"noul","noul":0.75}},"echo":"test-secret","ordinary":"retained" }"#;
    for (request_encoding, request_encoder, response_encoding, response_encoder) in [
        (
            "gzip",
            gzip as fn(&[u8]) -> Vec<u8>,
            "zstd",
            zstd as fn(&[u8]) -> Vec<u8>,
        ),
        ("zstd", zstd, "gzip", gzip),
    ] {
        let raw_request = request_encoder(plain_request);
        let raw_response = response_encoder(plain_response);
        let expected_request = raw_request.clone();
        let expected_response = raw_response.clone();
        let (url, task) = mock(Router::new().route(
            "/v1/systemone",
            post(move |headers: HeaderMap, body: Bytes| {
                let expected = expected_request.clone();
                let response = raw_response.clone();
                async move {
                    assert_eq!(headers[header::CONTENT_ENCODING], request_encoding);
                    assert_eq!(headers[header::CONTENT_TYPE], "application/json");
                    assert_eq!(headers[header::AUTHORIZATION], "Bearer test-secret");
                    assert_eq!(body.as_ref(), expected);
                    (
                        [
                            (header::CONTENT_ENCODING, response_encoding),
                            (header::CONTENT_TYPE, "application/json"),
                        ],
                        response,
                    )
                }
            }),
        ))
        .await;
        let fixture = fixture(&url, 4096, false);
        let mut request = native(raw_request);
        request.headers_mut().insert(
            header::CONTENT_ENCODING,
            HeaderValue::from_static(request_encoding),
        );
        let response = router(fixture.state.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_ENCODING],
            response_encoding
        );
        assert_eq!(
            to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
            expected_response
        );
        let record = saved(&fixture).await;
        assert_eq!(record["capture_complete"], true);
        assert_eq!(record["answers"][0]["valid"], true);
        assert_eq!(record["answers"][0]["value"], 0.75);
        assert_eq!(record["request_extra"]["echo"], "[REDACTED]");
        assert_eq!(record["response_extra"]["echo"], "[REDACTED]");
        assert_eq!(record["request_extra"]["ordinary"], "retained");
        assert_eq!(record["response_extra"]["ordinary"], "retained");
        assert!(!record.to_string().contains("discarded-state-secret"));
        assert!(!record.to_string().contains("test-secret"));
        assert_eq!(fixture.state.collector.health()["truncated"], 0);
        fixture.state.collector.shutdown().await;
        task.abort();
    }
}

#[tokio::test]
async fn multipart_compressed_capture_keeps_wire_and_original_asset_metadata() {
    let request_json = json!({"state":"photo","assets":{"photo":{"type":"image","part":"image-part"}},
        "questions":{"q":{"type":"noul","instructions":"Inspect"}}});
    let mut multipart=format!("--wire\r\nContent-Disposition: form-data; name=\"request\"\r\nContent-Type: application/json\r\n\r\n{request_json}\r\n--wire\r\nContent-Disposition: form-data; name=\"image-part\"; filename=\"not-a-path.png\"\r\nContent-Type: image/png\r\n\r\n").into_bytes();
    let image = b"\x89PNG\r\n\x1a\nsynthetic";
    multipart.extend_from_slice(image);
    multipart.extend_from_slice(b"\r\n--wire--\r\n");
    let raw = zstd(&multipart);
    let expected = raw.clone();
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(move |headers: HeaderMap, body: Bytes| {
            let expected = expected.clone();
            async move {
                assert_eq!(headers[header::CONTENT_ENCODING], "zstd");
                assert_eq!(
                    headers[header::CONTENT_TYPE],
                    "multipart/form-data; boundary=wire"
                );
                assert_eq!(body.as_ref(), expected);
                Json(json!({"answers":{"q":{"type":"noul","noul":0.5}}}))
            }
        }),
    ))
    .await;
    let fixture = fixture_with_config(&url, 4096, false, |config| config.capture_state = true);
    let mut request = native(raw);
    request
        .headers_mut()
        .insert(header::CONTENT_ENCODING, HeaderValue::from_static("zstd"));
    request.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("multipart/form-data; boundary=wire"),
    );
    let response = router(fixture.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    to_bytes(response.into_body(), 4096).await.unwrap();
    let record = saved(&fixture).await;
    assert_eq!(record["capture_complete"], true);
    assert_eq!(record["answers"][0]["valid"], true);
    assert_eq!(
        record["request_extra"]["assets"]["photo"]["part"],
        "image-part"
    );
    assert_eq!(record["request_extra"]["assets"]["photo"]["format"], "png");
    assert_eq!(
        record["request_extra"]["assets"]["photo"]["data_base64"],
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, image)
    );
    assert!(!record.to_string().contains("not-a-path.png"));
    fixture.state.collector.shutdown().await;
    task.abort();
}

#[tokio::test]
async fn decoding_failure_counts_one_gap_and_releases_admission_for_recovery() {
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(|_body: Bytes| async { Json(json!({"answers":{}})) }),
    ))
    .await;
    let fixture = fixture(&url, 1024, false);
    let mut request = native(gzip(&vec![b'x'; 8192]));
    request
        .headers_mut()
        .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    let response = router(fixture.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    to_bytes(response.into_body(), 1024).await.unwrap();
    let record = saved(&fixture).await;
    assert_eq!(record["capture_complete"], false);
    assert_eq!(
        record["normalization_error"],
        model::capture_transport::DECODE_ERROR_MESSAGE
    );
    for _ in 0..100 {
        if fixture.state.collector.health()["active_captures"] == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let health = fixture.state.collector.health();
    assert_eq!(health["truncated"], 1);
    assert_eq!(health["active_captures"], 0);
    let response = router(fixture.state.clone())
        .oneshot(native(r#"{"questions":{}}"#))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    to_bytes(response.into_body(), 1024).await.unwrap();
    for _ in 0..100 {
        let health = fixture.state.collector.health();
        if health["persisted"] == 2 && health["active_captures"] == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let health = fixture.state.collector.health();
    for (field, expected) in [
        ("forwarded", 2),
        ("captured", 2),
        ("persisted", 2),
        ("truncated", 1),
        ("dropped", 0),
        ("active_captures", 0),
        ("queue_depth", 0),
    ] {
        assert_eq!(health[field], expected, "{field}");
    }
    fixture.state.collector.shutdown().await;
    task.abort();
}
