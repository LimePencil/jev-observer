use std::{
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    body::Body,
    extract::Request,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::IntoResponse,
    routing::post,
};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};

use super::{
    lifecycle::{drain_after_http_grace, spawn_deadline_watchdog},
    local_api::{MAX_IMPORT_BODY_BYTES, export_database, invalid_filter},
    proxy::{
        CaptureGuard, Capturing, ForwardResponseBody, content_length, strip_hop_headers,
        supported_transfer_encoding,
    },
};
use crate::{
    access::Access,
    collector::Collector,
    config::{Config, UpstreamAuth},
    credentials::LOCAL_TOKEN_PREFIX,
    model::{self, Capture},
    store::{Filter, Store},
};

use super::*;
use axum::body::to_bytes;
use clap::Parser;
use tower::ServiceExt;

mod response_deadline_tests {
    include!("response_deadline_tests.rs");
}

mod capture_decoding_tests {
    include!("server/capture_decoding_tests.rs");
}

struct Fixture {
    _directory: tempfile::TempDir,
    state: AppState,
}

fn fixture(upstream: &str, limit: usize, demo: bool) -> Fixture {
    fixture_with_config(upstream, limit, demo, |_| {})
}

fn fixture_with_config(
    upstream: &str,
    limit: usize,
    demo: bool,
    configure: impl FnOnce(&mut Config),
) -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let mut config = Config::try_parse_from(["observer"]).unwrap();
    config.upstream = upstream.into();
    config.capture_limit = limit;
    config.demo = demo;
    configure(&mut config);
    config.validate().unwrap();
    let store = Store::open(&directory.path().join("test.sqlite"), 7, 1000).unwrap();
    let collector = Collector::start(store.clone(), config.normalize_options(), limit, 4, 4);
    let state = AppState::new(config, store, collector).unwrap();
    Fixture {
        _directory: directory,
        state,
    }
}

async fn mock(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}/v1/systemone"), task)
}

async fn raw_mock(
    response: Vec<u8>,
    finish: Option<Arc<tokio::sync::Notify>>,
) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            assert!(request.len() < 8192);
            assert!(socket.read_buf(&mut request).await.unwrap() > 0);
        }
        socket.write_all(&response).await.unwrap();
        if let Some(finish) = finish {
            finish.notified().await;
        }
    });
    (format!("http://{address}/v1/systemone"), task)
}

async fn serve_fixture(fixture: &mut Fixture) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    Arc::make_mut(&mut fixture.state.config).port = address.port();
    let app = router(fixture.state.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}/v1/systemone"), task)
}

fn native(body: impl Into<Body>) -> Request {
    Request::builder()
        .method(Method::POST)
        .uri("/v1/systemone")
        .header(header::HOST, "127.0.0.1:8765")
        .header(header::AUTHORIZATION, "Bearer test-secret")
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.into())
        .unwrap()
}

fn capturing(limit: usize) -> Capturing {
    Capturing {
        capture: Capture {
            capture_complete: true,
            ..Capture::default()
        },
        started: Instant::now(),
        limit,
        request_bytes: 0,
        response_bytes: 0,
        request_length: Some(limit as u64),
        response_length: Some(limit as u64),
        request_done: false,
        response_done: false,
        finalized: false,
    }
}

#[test]
fn capture_growth_is_amortized_for_tiny_chunks() {
    let limit = 256 * 1024;
    let mut capture = capturing(limit);
    assert_eq!(capture.capture.request.capacity(), 0);
    assert_eq!(capture.capture.response.capacity(), 0);
    let mut growths = [0; 2];
    let mut capacities = [0; 2];
    let payload: Vec<u8> = (0..limit).map(|i| (i % 251) as u8).collect();
    for byte in &payload {
        capture.append(&[*byte], false);
        capture.append(&[255 - *byte], true);
        for (index, body) in [&capture.capture.request, &capture.capture.response]
            .into_iter()
            .enumerate()
        {
            assert!(body.capacity() <= limit);
            if body.capacity() != capacities[index] {
                growths[index] += 1;
                capacities[index] = body.capacity();
            }
        }
    }
    assert!(growths.into_iter().all(|growths| growths <= 20));
    assert_eq!(capture.capture.request, payload);
    assert_eq!(
        capture.capture.response,
        payload.iter().map(|byte| 255 - byte).collect::<Vec<_>>()
    );
    assert!(capture.capture.capture_complete);
    assert!(capture.request_done && capture.response_done);
    capture.append(b"excess", false);
    capture.append(b"excess", true);
    assert!(!capture.capture.capture_complete);
    assert_eq!(capture.capture.request.len(), limit);
    assert_eq!(capture.capture.response.len(), limit);
    assert_eq!(capture.capture.request.capacity(), capacities[0]);
    assert_eq!(capture.capture.response.capacity(), capacities[1]);
    assert_eq!(capture.request_bytes, limit as u64 + 6);
    assert_eq!(capture.response_bytes, limit as u64 + 6);
}

#[test]
fn capture_growth_respects_small_and_irregular_limits() {
    for limit in [0, 1, 3, 31, 127, 4093, 262_143] {
        let payload: Vec<u8> = (0..limit + 17).map(|i| (i % 251) as u8).collect();
        let mut capture = capturing(limit);
        for response in [false, true] {
            let mut offset = 0;
            for size in [0, 1, 7, 4096, 5, 123, payload.len()].into_iter().cycle() {
                let end = (offset + size).min(payload.len());
                capture.append(&payload[offset..end], response);
                let body = if response {
                    &capture.capture.response
                } else {
                    &capture.capture.request
                };
                assert!(body.capacity() <= limit);
                assert_eq!(body.as_slice(), &payload[..end.min(limit)]);
                offset = end;
                if offset == payload.len() {
                    break;
                }
            }
        }
        assert!(!capture.capture.capture_complete);
        assert_eq!(capture.request_bytes, payload.len() as u64);
        assert_eq!(capture.response_bytes, payload.len() as u64);
        assert!(capture.request_done && capture.response_done);
    }
}

#[test]
fn filters_reject_invalid_dates_and_cursors_but_accept_timestamp_ties() {
    for case in [
        json!({"from":2,"to":1}),
        json!({"from":8_640_000_000_000_001_i64}),
        json!({"to":-8_640_000_000_000_001_i64}),
        json!({"request_cursor":"invalid:3"}),
        json!({"request_cursor":"123:0"}),
        json!({"request_cursor":"123:2:extra"}),
        json!({"group_cursor":"123:"}),
        json!({"group_cursor":"invalid:group"}),
        json!({"group_cursor":"group"}),
    ] {
        let filter: Filter = serde_json::from_value(case.clone()).unwrap();
        assert!(invalid_filter(&filter).is_some(), "accepted {case}");
    }
    for case in [
        json!({}),
        json!({"from":123,"to":123,"request_cursor":"123:2"}),
        json!({"from":123,"to":123,"group_cursor":"123:group-a"}),
        json!({"from":-8_640_000_000_000_000_i64,"to":8_640_000_000_000_000_i64}),
    ] {
        let filter: Filter = serde_json::from_value(case.clone()).unwrap();
        assert!(invalid_filter(&filter).is_none(), "rejected {case}");
    }
}

async fn saved(fixture: &Fixture) -> Value {
    for _ in 0..100 {
        let dashboard = fixture.state.store.dashboard(&Filter::default()).unwrap();
        if let Some(id) = dashboard["requests"][0]["id"].as_str() {
            return fixture.state.store.request(id).unwrap().unwrap();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "capture not persisted: {}",
        fixture.state.collector.health()
    );
}

#[tokio::test]
async fn input_opt_out_key_redaction_preserves_exact_forwarded_bytes() {
    const REQUEST: &str = "{ \"state\":\"PRIVATE_REQUEST_QZX\", \"questions\":{}, \"extension\":{\"state\":\"ORDINARY_REQUEST_QZX\"} }\n";
    const RESPONSE: &str = "{ \"state\":\"PRIVATE_RESPONSE_QZX\", \"original_metadata\":{\"state\":\"PRIVATE_LEGACY_QZX\"}, \"extension\":{\"state\":\"ORDINARY_RESPONSE_QZX\"} }\n";
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(|headers: HeaderMap, bytes: axum::body::Bytes| async move {
            assert_eq!(headers[header::AUTHORIZATION], "Bearer state");
            assert_eq!(bytes.as_ref(), REQUEST.as_bytes());
            RESPONSE
        }),
    ))
    .await;
    for capture_state in [false, true] {
        let fixture = fixture_with_config(&url, 8192, false, |config| {
            config.capture_state = capture_state
        });
        let mut request = native(REQUEST);
        request.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer state"),
        );
        let response = router(fixture.state.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), 8192).await.unwrap().as_ref(),
            RESPONSE.as_bytes()
        );
        let record = saved(&fixture).await;
        let text = record.to_string();
        for marker in [
            "PRIVATE_REQUEST_QZX",
            "PRIVATE_RESPONSE_QZX",
            "PRIVATE_LEGACY_QZX",
        ] {
            assert_eq!(
                text.contains(marker),
                capture_state,
                "input capture preference bypassed"
            );
        }
        assert!(text.contains("ORDINARY_REQUEST_QZX"));
        assert!(text.contains("ORDINARY_RESPONSE_QZX"));
        assert_eq!(record["capture_complete"], true);
        fixture.state.collector.shutdown().await;
    }
    task.abort();
    let _ = task.await;
}

#[test]
fn transfer_coding_support_requires_absent_or_single_chunked() {
    for (values, supported) in [
        (vec![], true),
        (vec!["chunked"], true),
        (vec![" ChUnKeD "], true),
        (vec![""], false),
        (vec!["identity"], false),
        (vec!["gzip"], false),
        (vec!["gzip, chunked"], false),
        (vec!["chunked, gzip"], false),
        (vec!["chunked, chunked"], false),
        (vec!["chunked", "chunked"], false),
        (vec!["gzip", "chunked"], false),
        (vec!["chunked,"], false),
        (vec![",chunked"], false),
    ] {
        let mut headers = HeaderMap::new();
        for value in &values {
            headers.append(
                header::TRANSFER_ENCODING,
                HeaderValue::from_str(value).unwrap(),
            );
        }
        assert_eq!(
            supported_transfer_encoding(&headers),
            supported,
            "{values:?}"
        );
    }
}

#[tokio::test]
async fn unsupported_response_transfer_coding_is_a_visible_gateway_failure() {
    // A valid gzip stream containing `{}`; Hyper removes only chunk framing.
    let gzip = [
        0x1f_u8, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3, 171, 174, 5, 0, 67, 191, 166, 163, 2, 0, 0, 0,
    ];
    let mut raw = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\nConnection: close\r\n\r\n{:x}\r\n",
        gzip.len()
    )
    .into_bytes();
    raw.extend_from_slice(&gzip);
    raw.extend_from_slice(b"\r\n0\r\n\r\n");
    let (upstream, upstream_task) = raw_mock(raw, None).await;
    let mut fixture = fixture(&upstream, 1024, false);
    let (observer, observer_task) = serve_fixture(&mut fixture).await;
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        fixture
            .state
            .client
            .post(observer)
            .bearer_auth("test-secret")
            .header(header::CONTENT_LENGTH, "0")
            .body("")
            .send(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = tokio::time::timeout(Duration::from_secs(3), response.text())
        .await
        .unwrap()
        .unwrap();
    assert!(body.contains("Unsupported upstream transfer encoding"));
    let record = saved(&fixture).await;
    assert_eq!(record["status"], 200);
    assert_eq!(record["capture_complete"], false);
    assert_eq!(
        record["transport_error"],
        "Unsupported upstream transfer encoding"
    );
    fixture.state.collector.shutdown().await;
    observer_task.abort();
    upstream_task.await.unwrap();
}

#[tokio::test]
async fn unsupported_request_transfer_coding_does_not_contact_upstream() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut fixture = fixture(
        &format!("http://{}/v1/systemone", upstream.local_addr().unwrap()),
        1024,
        false,
    );
    let (observer, observer_task) = serve_fixture(&mut fixture).await;
    let url = reqwest::Url::parse(&observer).unwrap();
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", url.port().unwrap()))
        .await
        .unwrap();
    let gzip = [
        0x1f_u8, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3, 171, 174, 5, 0, 67, 191, 166, 163, 2, 0, 0, 0,
    ];
    let mut request = format!(
        "POST /v1/systemone HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer test-secret\r\nTransfer-Encoding: gzip, chunked\r\nConnection: close\r\n\r\n{:x}\r\n",
        url.port().unwrap(), gzip.len(),
    ).into_bytes();
    request.extend_from_slice(&gzip);
    request.extend_from_slice(b"\r\n0\r\n\r\n");
    let response = tokio::time::timeout(Duration::from_secs(3), async {
        socket.write_all(&request).await.unwrap();
        let mut response = Vec::new();
        socket.read_to_end(&mut response).await.unwrap();
        response
    })
    .await
    .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 501 Not Implemented\r\n"));
    assert_eq!(fixture.state.collector.health()["forwarded"], 0);
    assert_eq!(fixture.state.collector.health()["captured"], 0);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), upstream.accept())
            .await
            .is_err()
    );
    fixture.state.collector.shutdown().await;
    observer_task.abort();
}

#[test]
fn content_length_accepts_only_identical_decimal_values() {
    for (values, expected) in [
        (vec![], None),
        (vec!["0"], Some(0)),
        (vec!["5, 5"], Some(5)),
        (vec![" 05 ", "5, 005"], Some(5)),
        (vec!["18446744073709551615"], Some(u64::MAX)),
        (vec!["5, 6"], None),
        (vec!["5", "6"], None),
        (vec!["5", ""], None),
        (vec![",5"], None),
        (vec!["5,"], None),
        (vec!["+5"], None),
        (vec!["-5"], None),
        (vec!["5.0"], None),
        (vec!["5 5"], None),
        (vec!["18446744073709551616"], None),
    ] {
        let mut headers = HeaderMap::new();
        for value in &values {
            headers.append(
                header::CONTENT_LENGTH,
                HeaderValue::from_str(value).unwrap(),
            );
        }
        assert_eq!(content_length(&headers), expected, "{values:?}");
        if values.len() == 1 && !values[0].contains(',') {
            strip_hop_headers(&mut headers);
            assert_eq!(headers[header::CONTENT_LENGTH], values[0]);
        }
        headers.insert(
            header::TRANSFER_ENCODING,
            HeaderValue::from_static("chunked"),
        );
        assert_eq!(
            content_length(&headers),
            None,
            "{values:?} with transfer coding"
        );
    }
}

#[tokio::test]
async fn repeated_identical_lengths_are_forwarded_as_one_decimal_value() {
    let payload = r#"{"message":"complete fixed-length response"}"#;
    let length = payload.len();
    for lengths in [
        format!("Content-Length: {length}\r\n"),
        format!("Content-Length: {length}, {length}\r\n"),
        format!("Content-Length: {length}\r\nContent-Length: {length}\r\n"),
        format!("Content-Length: 0{length}, {length}\r\nContent-Length: {length}\r\n"),
    ] {
        let response = format!("HTTP/1.1 200 OK\r\n{lengths}Connection: close\r\n\r\n{payload}");
        let (upstream, upstream_task) = raw_mock(response.into_bytes(), None).await;
        let mut fixture = fixture(&upstream, 1024, false);
        let (observer, observer_task) = serve_fixture(&mut fixture).await;
        let response = tokio::time::timeout(
            Duration::from_secs(3),
            fixture
                .state
                .client
                .post(observer)
                .bearer_auth("test-secret")
                .header(header::CONTENT_LENGTH, "0")
                .body("")
                .send(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_LENGTH],
            length.to_string()
        );
        assert_eq!(
            response
                .headers()
                .get_all(header::CONTENT_LENGTH)
                .iter()
                .count(),
            1
        );
        let body = tokio::time::timeout(Duration::from_secs(3), response.bytes())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(body.as_ref(), payload.as_bytes());
        let record = saved(&fixture).await;
        assert_eq!(record["capture_complete"], true);
        assert!(record["transport_error"].is_null());
        assert_eq!(
            record["response_extra"]["message"],
            "complete fixed-length response"
        );
        fixture.state.collector.shutdown().await;
        observer_task.abort();
        upstream_task.await.unwrap();
    }
}

#[tokio::test]
async fn chunked_responses_discard_conflicting_lengths_before_forwarding() {
    let payload = r#"{"message":"complete chunked response"}"#;
    for length in [0, 2, 999] {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{payload}\r\n0\r\n\r\n",
            payload.len()
        );
        let (upstream, upstream_task) = raw_mock(response.into_bytes(), None).await;
        let mut fixture = fixture(&upstream, 1024, false);
        let (observer, observer_task) = serve_fixture(&mut fixture).await;
        let response = tokio::time::timeout(
            Duration::from_secs(3),
            fixture
                .state
                .client
                .post(observer)
                .bearer_auth("test-secret")
                .header(header::CONTENT_LENGTH, "0")
                .body("")
                .send(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key(header::CONTENT_LENGTH));
        let body = tokio::time::timeout(Duration::from_secs(3), response.bytes())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(body.as_ref(), payload.as_bytes());
        let record = saved(&fixture).await;
        assert_eq!(record["capture_complete"], true);
        assert!(record["transport_error"].is_null());
        assert_eq!(
            record["response_extra"]["message"],
            "complete chunked response"
        );
        fixture.state.collector.shutdown().await;
        observer_task.abort();
        upstream_task.await.unwrap();
    }
}

#[tokio::test]
async fn chunked_response_cancellation_cannot_complete_at_a_conflicting_length() {
    let first = "{\"message\":";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{first}\r\n",
        first.len()
    );
    let finish = Arc::new(tokio::sync::Notify::new());
    let (upstream, upstream_task) = raw_mock(response.into_bytes(), Some(finish.clone())).await;
    let mut fixture = fixture(&upstream, 1024, false);
    let (observer, observer_task) = serve_fixture(&mut fixture).await;
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        fixture
            .state
            .client
            .post(observer)
            .bearer_auth("test-secret")
            .header(header::CONTENT_LENGTH, "0")
            .body("")
            .send(),
    )
    .await
    .unwrap()
    .unwrap();
    let mut body = response.bytes_stream();
    let observed = tokio::time::timeout(Duration::from_secs(3), async {
        let mut observed = Vec::new();
        while observed.len() < first.len() {
            let Some(chunk) = body.next().await else {
                break;
            };
            observed.extend_from_slice(&chunk.unwrap());
        }
        observed
    })
    .await
    .unwrap();
    assert_eq!(observed, first.as_bytes());
    drop(body);
    let record = saved(&fixture).await;
    assert_eq!(record["capture_complete"], false);
    assert!(record["transport_error"].is_string());
    fixture.state.collector.shutdown().await;
    finish.notify_one();
    observer_task.abort();
    upstream_task.await.unwrap();
}

#[tokio::test]
async fn configured_upstream_deadline_controls_delayed_headers() {
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(|| async {
            tokio::time::sleep(Duration::from_millis(1200)).await;
            Json(json!({"answers": {}}))
        }),
    ))
    .await;

    for (seconds, expected) in [(1, StatusCode::GATEWAY_TIMEOUT), (2, StatusCode::OK)] {
        let fixture = fixture_with_config(&url, 1024, false, |config| {
            config.upstream_timeout_secs = seconds;
        });
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            router(fixture.state.clone()).oneshot(native("{}")),
        )
        .await
        .expect("the configured upstream deadline was not enforced")
        .unwrap();
        assert_eq!(response.status(), expected);
        let _ = to_bytes(response.into_body(), 1024).await.unwrap();
        let record = saved(&fixture).await;
        assert_eq!(record["status"], expected.as_u16());
        if expected == StatusCode::GATEWAY_TIMEOUT {
            assert_eq!(record["transport_error"], "Upstream request timed out");
            assert_eq!(record["capture_complete"], false);
        } else {
            assert!(record["transport_error"].is_null());
            assert_eq!(record["capture_complete"], true);
        }
        fixture.state.collector.shutdown().await;
    }
    task.abort();
}

#[tokio::test]
async fn configured_upstream_deadline_also_bounds_response_body() {
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(|| async {
            Body::from_stream(async_stream::stream! {
                yield Ok::<_, std::convert::Infallible>(Bytes::from_static(b"{\"answers\":"));
                tokio::time::sleep(Duration::from_secs(10)).await;
                yield Ok(Bytes::from_static(b"{}}"));
            })
        }),
    ))
    .await;
    let fixture = fixture_with_config(&url, 1024, false, |config| {
        config.upstream_timeout_secs = 1;
    });
    let response = router(fixture.state.clone())
        .oneshot(native("{}"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result = tokio::time::timeout(Duration::from_secs(3), to_bytes(response.into_body(), 1024))
        .await
        .expect("the upstream deadline stopped after response headers");
    assert!(result.is_err());
    let record = saved(&fixture).await;
    assert_eq!(record["status"], 200);
    assert_eq!(record["capture_complete"], false);
    assert_eq!(record["transport_error"], "Upstream response timed out");
    fixture.state.collector.shutdown().await;
    task.abort();
}

#[tokio::test]
async fn local_model_auth_stays_local_and_does_not_need_a_provider_key() {
    let access = Access::test();
    let echoed = access.token().to_owned();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let received = calls.clone();
    let (upstream, task) = mock(Router::new().route("/v1/systemone", post(move |headers: HeaderMap| {
        let echoed = echoed.clone();
        let received = received.clone();
        async move {
            received.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            assert!(!headers.contains_key(header::AUTHORIZATION));
            assert!(!headers.contains_key("x-observer-access"));
            assert!(!headers.contains_key("x-observer-source"));
            Json(json!({"model":"laya-rl-agent","answers":{"q":{"type":"noul","noul":0.8}},"usage":{"input_tokens":8,"output_tokens":0},"echo":echoed}))
        }
    }))).await;
    let mut fixture = fixture(&upstream, 4096, false);
    Arc::make_mut(&mut fixture.state.config).upstream_auth = UpstreamAuth::None;
    fixture.state.access = Some(access.clone());
    let app = router(fixture.state.clone());
    let body = r#"{"model":"english","state":"Synthetic","questions":{"q":{"type":"noul","instructions":"Is this synthetic?"}}}"#;
    let rejected = app.clone().oneshot(native(body)).await.unwrap();
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    for token_as_bearer in [true, false] {
        let mut request = native(body);
        if token_as_bearer {
            request.headers_mut().insert(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", access.token())).unwrap(),
            );
        } else {
            request.headers_mut().insert(
                "x-observer-access",
                HeaderValue::from_str(access.token()).unwrap(),
            );
        }
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(value["answers"]["q"]["noul"], 0.8);
    }
    let saved = saved(&fixture).await;
    assert_eq!(saved["answers"][0]["valid"], true);
    assert_eq!(saved["output_tokens"], 0);
    assert!(!saved.to_string().contains(access.token()));
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    fixture.state.collector.shutdown().await;
    task.abort();
}

#[tokio::test]
async fn preserves_unknown_wire_data_status_and_strips_private_headers() {
    let expected = br#"{"unknown_extension":{"opaque":[1,2,3]},"questions":{}}"#.to_vec();
    let expected_for_mock = expected.clone();
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(move |request: Request| async move {
            assert_eq!(
                request.headers()[header::AUTHORIZATION],
                "Bearer test-secret"
            );
            assert!(request.headers().get("x-observer-source").is_none());
            assert!(request.headers().get("x-private-hop").is_none());
            assert!(request.headers().get(header::COOKIE).is_none());
            for name in [
                "origin",
                "referer",
                "forwarded",
                "x-forwarded-for",
                "x-real-ip",
                "sec-fetch-site",
            ] {
                assert!(
                    request.headers().get(name).is_none(),
                    "{name} leaked upstream"
                );
            }
            assert_eq!(
                to_bytes(request.into_body(), 1024).await.unwrap().as_ref(),
                expected_for_mock.as_slice()
            );
            let mut response = (
                StatusCode::TOO_MANY_REQUESTS,
                "{\"provider_extension\":{\"unchanged\":true}}",
            )
                .into_response();
            response
                .headers_mut()
                .insert("retry-after", HeaderValue::from_static("7"));
            response
                .headers_mut()
                .insert("connection", HeaderValue::from_static("x-upstream-hop"));
            response
                .headers_mut()
                .insert("x-upstream-hop", HeaderValue::from_static("hidden"));
            response.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_static("session=provider-cookie"),
            );
            response
        }),
    ))
    .await;
    let fixture = fixture(&url, 4096, false);
    let mut request = native(expected);
    request
        .headers_mut()
        .insert("x-observer-source", HeaderValue::from_static("fixture"));
    request.headers_mut().insert(
        header::COOKIE,
        HeaderValue::from_static("session=local-cookie"),
    );
    request.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_static("http://127.0.0.1:8765"),
    );
    request.headers_mut().insert(
        header::REFERER,
        HeaderValue::from_static("http://127.0.0.1:8765/private?search=confidential"),
    );
    request
        .headers_mut()
        .insert("forwarded", HeaderValue::from_static("for=192.0.2.1"));
    request
        .headers_mut()
        .insert("x-forwarded-for", HeaderValue::from_static("192.0.2.1"));
    request
        .headers_mut()
        .insert("x-real-ip", HeaderValue::from_static("192.0.2.1"));
    request
        .headers_mut()
        .insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
    request
        .headers_mut()
        .insert("connection", HeaderValue::from_static("x-private-hop"));
    request
        .headers_mut()
        .insert("x-private-hop", HeaderValue::from_static("hidden"));
    let response = router(fixture.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()["retry-after"], "7");
    assert!(response.headers().get("x-upstream-hop").is_none());
    assert!(response.headers().get(header::SET_COOKIE).is_none());
    assert_eq!(
        to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
        br#"{"provider_extension":{"unchanged":true}}"#
    );
    let record = saved(&fixture).await;
    assert_eq!(record["status"], 429);
    assert_eq!(record["source"], "fixture");
    assert!(!record.to_string().contains("test-secret"));
    fixture.state.collector.shutdown().await;
    task.abort();
}

#[tokio::test]
async fn generated_live_ids_keep_hex_secret_captures_distinct() {
    let body = serde_json::to_vec(&json!({
        "questions":{},
        "credentials":"0123456789abcdef".chars().map(|value| value.to_string()).collect::<Vec<_>>()
    }))
    .unwrap();
    let expected = body.clone();
    let response_body = br#"{"answers":{},"fixture":"unchanged"}"#;
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(move |request: Request| {
            let expected = expected.clone();
            async move {
                assert_eq!(
                    request.headers()[header::AUTHORIZATION],
                    "Bearer test-secret"
                );
                assert_eq!(
                    to_bytes(request.into_body(), 4096).await.unwrap().as_ref(),
                    expected
                );
                response_body.as_slice()
            }
        }),
    ))
    .await;
    let fixture = fixture(&url, 4096, false);
    for _ in 0..2 {
        let response = router(fixture.state.clone())
            .oneshot(native(body.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
            response_body
        );
    }
    fixture.state.collector.shutdown().await;
    task.abort();
    let health = fixture.state.collector.health();
    for field in ["forwarded", "captured", "persisted"] {
        assert_eq!(health[field], 2, "{field}");
    }
    for field in [
        "dropped",
        "write_failures",
        "queue_depth",
        "active_captures",
    ] {
        assert_eq!(health[field], 0, "{field}");
    }
    let output = fixture
        .state
        .store
        .export(
            &Filter {
                window: Some("all".into()),
                ..Default::default()
            },
            "jsonl",
        )
        .unwrap();
    let records: Vec<Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 2);
    assert_ne!(records[0]["id"], records[1]["id"]);
    for record in records {
        let id = record["id"].as_str().unwrap();
        assert_eq!(uuid::Uuid::parse_str(id).unwrap().get_version_num(), 7);
        assert!(!id.contains("[REDACTED]"));
        // Supplied body keys still follow ordinary privacy rules.
        assert_eq!(
            record["normalization_error"],
            "Request questions is not an object"
        );
    }
}

#[tokio::test]
async fn credential_matching_generated_envelope_key_preserves_forwarding_and_capture() {
    let request_body =
        br#"{"questions":{"q":{"type":"noul","instructions":"Eligible?"}},"echo":"id"}"#;
    let response_body = br#"{"answers":{"q":{"type":"noul","noul":0.8}},"echo":"id"}"#;
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(move |request: Request| async move {
            assert_eq!(request.headers()[header::AUTHORIZATION], "Bearer id");
            assert_eq!(
                to_bytes(request.into_body(), 4096).await.unwrap().as_ref(),
                request_body
            );
            response_body.as_slice()
        }),
    ))
    .await;
    let fixture = fixture(&url, 4096, false);
    let mut request = native(request_body.as_slice());
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer id"));
    request
        .headers_mut()
        .insert("x-observer-source", HeaderValue::from_static("source-id"));
    let response = router(fixture.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
        response_body
    );
    fixture.state.collector.shutdown().await;
    task.abort();
    let health = fixture.state.collector.health();
    assert_eq!(health["persisted"], 1);
    assert_eq!(health["dropped"], 0);
    assert_eq!(health["write_failures"], 0);
    let record = saved(&fixture).await;
    assert_eq!(record["source"], "source-[REDACTED]");
    assert_eq!(record["request_extra"]["echo"], "[REDACTED]");
    assert_eq!(record["response_extra"]["echo"], "[REDACTED]");
    assert_eq!(record["answers"][0]["valid"], true);
    assert_eq!(record["answers"][0]["definition_redacted"], true);
    assert_eq!(record["capture_complete"], true);
}

#[tokio::test]
async fn basic_credential_echoes_are_redacted_in_both_local_auth_modes() {
    for upstream_auth in [UpstreamAuth::Bearer, UpstreamAuth::None] {
        for prefix in [
            "Basic ", "basic ", "BASIC ", "bAsIc ", "Basic  ", "bAsIc   ",
        ] {
            let (url, task) = mock(Router::new().route(
                "/v1/systemone",
                post(move |request: Request| async move {
                    let expected = (upstream_auth == UpstreamAuth::Bearer)
                        .then_some("Bearer synthetic-fallback-provider-key");
                    assert_eq!(
                        request
                            .headers()
                            .get(header::AUTHORIZATION)
                            .map(|value| value.to_str().unwrap()),
                        expected
                    );
                    assert!(request.headers().get("x-observer-source").is_none());
                    assert!(request.headers().get("x-observer-task-version").is_none());
                    let body: Value =
                        serde_json::from_slice(&to_bytes(request.into_body(), 4096).await.unwrap())
                            .unwrap();
                    Json(json!({"answers":{"q":{"type":"noul","noul":0.8}},
                    "echo":body["echo"], "ordinary_extension":body["ordinary_extension"]}))
                }),
            ))
            .await;
            let mut fixture = fixture_with_config(&url, 4096, false, |config| {
                config.upstream_auth = upstream_auth;
                config.api_key = Some("synthetic-fallback-provider-key".into());
            });
            let access = Access::test();
            fixture.state.access = Some(access.clone());
            let component = access.basic_header().strip_prefix("Basic ").unwrap();
            let basic = format!("{prefix}{component}");
            let basic = basic.as_str();
            let mut echoes = vec![basic, component, access.token()];
            if upstream_auth == UpstreamAuth::Bearer {
                echoes.push("synthetic-fallback-provider-key");
            }
            let body = json!({"questions":{"q":{"type":"noul","instructions":"Synthetic question"}},
            "echo":echoes, "ordinary_extension":{"fixture":"ordinary retained text"}});
            let mut invalid = native(body.to_string());
            invalid.headers_mut().insert(
                header::AUTHORIZATION,
                HeaderValue::from_static("Basic invalid"),
            );
            let rejected = router(fixture.state.clone())
                .oneshot(invalid)
                .await
                .unwrap();
            assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
            let rejection = to_bytes(rejected.into_body(), 1024).await.unwrap();
            assert!(!String::from_utf8_lossy(&rejection).contains("Basic invalid"));
            assert_eq!(fixture.state.collector.health()["forwarded"], 0);

            let mut request = native(body.to_string());
            request
                .headers_mut()
                .insert(header::AUTHORIZATION, HeaderValue::from_str(basic).unwrap());
            request
                .headers_mut()
                .insert("x-observer-source", HeaderValue::from_str(basic).unwrap());
            request.headers_mut().insert(
                "x-observer-task-version",
                HeaderValue::from_str(component).unwrap(),
            );
            let response = router(fixture.state.clone())
                .oneshot(request)
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let response: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap())
                    .unwrap();
            assert_eq!(response["echo"], body["echo"]);
            assert_eq!(response["ordinary_extension"], body["ordinary_extension"]);
            let record = saved(&fixture).await;
            let retained = record.to_string();
            for secret in echoes {
                assert!(
                    !retained.contains(secret),
                    "accepted local credential survived capture"
                );
            }
            assert_eq!(record["source"], "[REDACTED]");
            assert_eq!(record["answers"][0]["task_version"], "[REDACTED]");
            assert_eq!(
                record["request_extra"]["ordinary_extension"],
                body["ordinary_extension"]
            );
            assert_eq!(
                record["response_extra"]["ordinary_extension"],
                body["ordinary_extension"]
            );
            fixture.state.collector.shutdown().await;
            task.abort();
        }
    }
}

#[tokio::test]
async fn bearer_whitespace_keeps_normalized_echoes_out_of_history() {
    let expected = br#"{"answers":{},"echo":"spaced-secret"}"#;
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(move |request: Request| async move {
            assert_eq!(
                request.headers()[header::AUTHORIZATION],
                "Bearer    spaced-secret"
            );
            let _ = to_bytes(request.into_body(), 1024).await.unwrap();
            expected.as_slice()
        }),
    ))
    .await;
    let fixture = fixture(&url, 4096, false);
    let mut request = native(r#"{"questions":{},"echo":"spaced-secret"}"#);
    request.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer    spaced-secret"),
    );
    let response = router(fixture.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
        expected
    );
    let record = saved(&fixture).await;
    assert!(!record.to_string().contains("spaced-secret"));
    assert!(record.to_string().contains("[REDACTED]"));
    fixture.state.collector.shutdown().await;
    task.abort();
}

#[tokio::test]
async fn forwards_bodies_beyond_capture_limit_and_marks_gap() {
    let response_bytes = vec![b'x'; 512 * 1024];
    let expected = response_bytes.clone();
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(move |request: Request| async move {
            assert_eq!(
                to_bytes(request.into_body(), 1024 * 1024)
                    .await
                    .unwrap()
                    .len(),
                384 * 1024
            );
            response_bytes
        }),
    ))
    .await;
    let fixture = fixture(&url, 1024, false);
    let response = router(fixture.state.clone())
        .oneshot(native(vec![b'y'; 384 * 1024]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .as_ref(),
        expected.as_slice()
    );
    let record = saved(&fixture).await;
    assert_eq!(record["capture_complete"], false);
    assert_eq!(fixture.state.collector.health()["truncated"], 1);
    fixture.state.collector.shutdown().await;
    task.abort();
}

#[tokio::test]
async fn returns_redirect_without_contacting_location() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_clone = calls.clone();
    let (url, task) = mock(
        Router::new()
            .route(
                "/v1/systemone",
                post(|| async { (StatusCode::TEMPORARY_REDIRECT, [("location", "/other")]) }),
            )
            .route(
                "/other",
                post(move || async move {
                    calls_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    "wrong"
                }),
            ),
    )
    .await;
    let fixture = fixture(&url, 1024, false);
    let response = router(fixture.state.clone())
        .oneshot(native("{}"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(response.headers()[header::LOCATION], "/other");
    let _ = to_bytes(response.into_body(), 1024).await.unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    fixture.state.collector.shutdown().await;
    task.abort();
}

#[tokio::test]
async fn local_api_rejects_rebinding_cross_origin_and_unmarked_mutations() {
    let fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, true);
    for request in [
        Request::builder()
            .uri("/api/settings")
            .header("host", "attacker.example:8765")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .uri("/api/settings")
            .header("host", "127.0.0.1:8765")
            .header("origin", "https://attacker.example")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .uri("/api/export")
            .header("host", "127.0.0.1:8765")
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .method("POST")
            .uri("/v1/systemone")
            .header("host", "127.0.0.1:8765")
            .header("sec-fetch-site", "cross-site")
            .body(Body::from("{}"))
            .unwrap(),
        Request::builder()
            .method("DELETE")
            .uri("/api/data")
            .header("host", "127.0.0.1:8765")
            .body(Body::empty())
            .unwrap(),
    ] {
        assert_eq!(
            router(fixture.state.clone())
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let request = Request::builder()
        .uri("/api/settings")
        .header("host", "127.0.0.1:8765")
        .header("origin", "http://127.0.0.1:8765")
        .body(Body::empty())
        .unwrap();
    let response = router(fixture.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
    fixture.state.collector.shutdown().await;
}

#[tokio::test]
async fn localhost_host_case_accepts_authenticated_equivalent_origins() {
    let mut fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, true);
    let access = Access::test();
    fixture.state.access = Some(access.clone());
    for (host, origin) in [
        ("localhost:8765", None),
        ("LOCALHOST:8765", None),
        ("LocalHost:8765", None),
        ("LOCALHOST:8765", Some("http://localhost:8765")),
        ("localhost:8765", Some("http://LOCALHOST:8765")),
        ("LoCaLhOsT:8765", Some("http://lOcAlHoSt:8765")),
        ("127.0.0.1:8765", Some("http://127.0.0.1:8765")),
        ("[::1]:8765", Some("http://[::1]:8765")),
    ] {
        let mut request = Request::builder()
            .uri("/api/health")
            .header(header::HOST, host)
            .header(header::AUTHORIZATION, access.basic_header())
            .header("sec-fetch-site", "same-origin");
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        let response = router(fixture.state.clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{host}, {origin:?}");
        assert!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
        let health: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 16 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(health["forwarded"], 0);
    }
    fixture.state.collector.shutdown().await;
}

#[tokio::test]
async fn localhost_host_case_still_requires_authentication() {
    let mut fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, true);
    fixture.state.access = Some(Access::test());
    for host in ["localhost:8765", "LOCALHOST:8765", "LocalHost:8765"] {
        for authorization in [None, Some("Basic b2JzZXJ2ZXI6d3Jvbmc=")] {
            let mut request = Request::builder()
                .uri("/api/health")
                .header(header::HOST, host);
            if let Some(value) = authorization {
                request = request.header(header::AUTHORIZATION, value);
            }
            let response = router(fixture.state.clone())
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{host}");
            assert!(response.headers().contains_key(header::WWW_AUTHENTICATE));
        }
    }
    fixture.state.collector.shutdown().await;
}

#[tokio::test]
async fn localhost_host_case_keeps_authority_allowlist_and_port_checks() {
    let mut fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, true);
    let access = Access::test();
    fixture.state.access = Some(access.clone());
    for host in [
        "localhost:8766",
        "LOCALHOST:8766",
        "LOCALHOST",
        "LOCALHOST.invalid:8765",
        "evil-LOCALHOST:8765",
        "LOCALHOST.:8765",
        "observer@LOCALHOST:8765",
        "LOCALHOST@attacker.invalid:8765",
        "127.0.0.2:8765",
        "127.1:8765",
        "127.0.0.01:8765",
        "[0:0:0:0:0:0:0:1]:8765",
        "[::ffff:127.0.0.1]:8765",
    ] {
        let response = router(fixture.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .header(header::HOST, host)
                    .header(header::AUTHORIZATION, access.basic_header())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{host}");
    }
    fixture.state.collector.shutdown().await;
}

#[tokio::test]
async fn localhost_host_case_keeps_origin_fetch_and_mutation_guards() {
    let mut fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, true);
    let access = Access::test();
    fixture.state.access = Some(access.clone());
    for host in ["localhost:8765", "LOCALHOST:8765"] {
        for (origin, fetch_site, mutation) in [
            (Some("http://127.0.0.1:8765"), None, false),
            (Some("http://localhost:8766"), None, false),
            (Some("https://localhost:8765"), None, false),
            (Some("http://observer@localhost:8765"), None, false),
            (Some("http://localhost.invalid:8765"), None, false),
            (None, Some("cross-site"), false),
            (None, Some("same-site"), false),
            (None, None, true),
        ] {
            let mut request = Request::builder()
                .method(if mutation { "DELETE" } else { "GET" })
                .uri(if mutation { "/api/data" } else { "/api/health" })
                .header(header::HOST, host)
                .header(header::AUTHORIZATION, access.basic_header());
            if let Some(origin) = origin {
                request = request.header(header::ORIGIN, origin);
            }
            if let Some(site) = fetch_site {
                request = request.header("sec-fetch-site", site);
            }
            let response = router(fixture.state.clone())
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "{host}, {origin:?}, {fetch_site:?}, mutation={mutation}"
            );
        }
    }
    fixture.state.collector.shutdown().await;
}

#[tokio::test]
async fn dashboard_requires_owner_access_token() {
    let mut fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, true);
    let access = Access::test();
    fixture.state.access = Some(access.clone());
    let blocked = Request::builder()
        .uri("/api/dashboard")
        .header(header::HOST, "127.0.0.1:8765")
        .body(Body::empty())
        .unwrap();
    let response = router(fixture.state.clone())
        .oneshot(blocked)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().contains_key(header::WWW_AUTHENTICATE));
    let allowed = Request::builder()
        .uri("/api/dashboard")
        .header(header::HOST, "127.0.0.1:8765")
        .header(header::AUTHORIZATION, access.basic_header())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        router(fixture.state.clone())
            .oneshot(allowed)
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    fixture.state.collector.shutdown().await;
}

#[tokio::test]
async fn fallback_key_rejects_browser_simple_posts() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = calls.clone();
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(move |request: Request| async move {
            assert_eq!(
                request.headers()[header::AUTHORIZATION],
                "Bearer fallback-key"
            );
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Json(json!({"answers": {}}))
        }),
    ))
    .await;
    let mut fixture = fixture(&url, 1024, false);
    Arc::make_mut(&mut fixture.state.config).api_key = Some("fallback-key".into());
    for content_type in [
        None,
        Some("text/plain"),
        Some("application/x-www-form-urlencoded"),
        Some("multipart/form-data; boundary=x"),
    ] {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/v1/systemone")
            .header(header::HOST, "127.0.0.1:8765");
        if let Some(content_type) = content_type {
            builder = builder.header(header::CONTENT_TYPE, content_type);
        }
        let response = router(fixture.state.clone())
            .oneshot(builder.body(Body::from("{}")).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    let access = Access::test();
    fixture.state.access = Some(access.clone());
    let request = Request::builder()
        .method(Method::POST)
        .uri("/v1/systemone")
        .header(header::HOST, "127.0.0.1:8765")
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(
        router(fixture.state.clone())
            .oneshot(request)
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let request = Request::builder()
        .method(Method::POST)
        .uri("/v1/systemone")
        .header(header::HOST, "127.0.0.1:8765")
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header("x-observer-access", access.token())
        .body(Body::from("{}"))
        .unwrap();
    let response = router(fixture.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let mut direct = native("{}");
    direct.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer fallback-key"),
    );
    assert_eq!(
        router(fixture.state.clone())
            .oneshot(direct)
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let mut direct = native("{}");
    direct.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer fallback-key"),
    );
    direct.headers_mut().insert(
        "x-observer-access",
        HeaderValue::from_str(access.token()).unwrap(),
    );
    assert_eq!(
        router(fixture.state.clone())
            .oneshot(direct)
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    fixture.state.collector.shutdown().await;
    task.abort();
}

#[tokio::test]
async fn registered_key_requires_local_token_and_never_enters_history() {
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(|request: Request| async move {
            assert_eq!(
                request.headers()[header::AUTHORIZATION],
                "Bearer registered-provider-key"
            );
            assert!(request.headers().get("x-observer-client-token").is_none());
            assert!(request.headers().get("x-observer-access").is_none());
            let body: Value = serde_json::from_slice(&to_bytes(request.into_body(), 4096).await.unwrap()).unwrap();
            Json(json!({"answers": {}, "echo": body["echo"], "provider_echo": "registered-provider-key"}))
        }),
    ))
    .await;
    let mut fixture = fixture(&url, 4096, false);
    let put = Request::builder()
        .method(Method::PUT)
        .uri("/api/credentials")
        .header(header::HOST, "127.0.0.1:8765")
        .header("x-observer-request", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"api_key":"registered-provider-key","persist":false}"#,
        ))
        .unwrap();
    let response = router(fixture.state.clone()).oneshot(put).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let token = result["client_token"].as_str().unwrap();
    assert!(token.starts_with(LOCAL_TOKEN_PREFIX));
    assert!(!result.to_string().contains("registered-provider-key"));
    let status = Request::builder()
        .uri("/api/credentials")
        .header(header::HOST, "127.0.0.1:8765")
        .body(Body::empty())
        .unwrap();
    let response = router(fixture.state.clone()).oneshot(status).await.unwrap();
    let text = to_bytes(response.into_body(), 4096).await.unwrap();
    assert!(!String::from_utf8_lossy(&text).contains(token));
    assert!(!String::from_utf8_lossy(&text).contains("registered-provider-key"));

    fixture.state.access = Some(Access::test());

    let unauthenticated = Request::builder()
        .method(Method::POST)
        .uri("/v1/systemone")
        .header(header::HOST, "127.0.0.1:8765")
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(
        router(fixture.state.clone())
            .oneshot(unauthenticated)
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let mut bad = native("{}");
    bad.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer jo_local_invalid"),
    );
    assert_eq!(
        router(fixture.state.clone())
            .oneshot(bad)
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let workspace_token = fixture.state.access.as_ref().unwrap().token().to_owned();
    let mut request = native(
        json!({"questions":{},"echo":["registered-provider-key",token,workspace_token]})
            .to_string(),
    );
    request.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    request.headers_mut().insert(
        "x-observer-access",
        HeaderValue::from_str(&workspace_token).unwrap(),
    );
    let response = router(fixture.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("registered-provider-key"));
    let record = saved(&fixture).await;
    assert_eq!(
        record["request_extra"]["echo"],
        json!(["[REDACTED]", "[REDACTED]", "[REDACTED]"])
    );
    assert_eq!(
        record["response_extra"]["echo"],
        json!(["[REDACTED]", "[REDACTED]", "[REDACTED]"])
    );
    let stored = record.to_string();
    assert!(!stored.contains("registered-provider-key"));
    assert!(!stored.contains(token));
    assert!(!stored.contains(&workspace_token));

    fixture.state.access = None;

    let remove = Request::builder()
        .method(Method::DELETE)
        .uri("/api/credentials")
        .header(header::HOST, "127.0.0.1:8765")
        .header("x-observer-request", "1")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        router(fixture.state.clone())
            .oneshot(remove)
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let mut expired = native("{}");
    expired.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    assert_eq!(
        router(fixture.state.clone())
            .oneshot(expired)
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.state.collector.shutdown().await;
    task.abort();
}

#[tokio::test]
async fn demo_never_forwards_and_imports_are_bounded() {
    let fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, true);
    let response = router(fixture.state.clone())
        .oneshot(native("{}"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(fixture.state.collector.health()["forwarded"], 0);
    let request = Request::builder()
        .method("POST")
        .uri("/api/import")
        .header("host", "127.0.0.1:8765")
        .header("x-observer-request", "1")
        .header("content-type", "application/json")
        .body(Body::from(vec![b' '; MAX_IMPORT_BODY_BYTES + 1]))
        .unwrap();
    assert_eq!(
        router(fixture.state.clone())
            .oneshot(request)
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    fixture.state.collector.shutdown().await;
}

#[tokio::test]
async fn import_limit_applies_to_text_before_json_transport_escaping() {
    let fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, true);
    let mut record = model::sample_records().remove(0);
    record["state"] = json!({"quoted": "\"".repeat(MAX_IMPORT_BYTES / 4)});
    record["state_retained"] = json!(true);
    let mut text = record.to_string();
    assert!(text.len() < MAX_IMPORT_BYTES);
    text.extend(std::iter::repeat_n(' ', MAX_IMPORT_BYTES - text.len()));
    for (extra, status) in [("", StatusCode::OK), (" ", StatusCode::PAYLOAD_TOO_LARGE)] {
        let body = json!({"format":"observer-jsonl", "text":format!("{text}{extra}")}).to_string();
        assert!(body.len() > MAX_IMPORT_BYTES);
        let request = Request::builder()
            .method("POST")
            .uri("/api/import")
            .header("host", "127.0.0.1:8765")
            .header("x-observer-request", "1")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let response = router(fixture.state.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        if status == StatusCode::OK {
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap())
                    .unwrap();
            assert_eq!(body["imported"], 1);
        }
    }
    fixture.state.collector.shutdown().await;
}

#[tokio::test]
async fn text_shape_redaction_keeps_wire_bytes_and_records_a_minimal_live_gap() {
    for (field, request_body) in [
        (
            "instructions",
            br#"{"questions":{"q":{"type":"noul","instructions":12}}}"#.as_slice(),
        ),
        (
            "true",
            br#"{"questions":{"q":{"type":"noul","instructions":"Rule","criteria":{"true":12}}}}"#
                .as_slice(),
        ),
    ] {
        let response_body = br#"{"answers":{"q":{"type":"noul","noul":0.2}}}"#;
        let (url, task) = mock(Router::new().route(
            "/v1/systemone",
            post(move |request: Request| async move {
                assert_eq!(
                    request.headers()[header::AUTHORIZATION],
                    "Bearer test-secret"
                );
                assert_eq!(
                    to_bytes(request.into_body(), 4096).await.unwrap().as_ref(),
                    request_body
                );
                response_body.as_slice()
            }),
        ))
        .await;
        let fixture = fixture_with_config(&url, 4096, false, |config| {
            config.provider = Some("typesafe".into());
            config.redact_keys.push(field.into());
        });
        let response = router(fixture.state.clone())
            .oneshot(native(request_body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
            response_body
        );
        fixture.state.collector.shutdown().await;
        task.abort();
        let record = saved(&fixture).await;
        assert_eq!(record["capture_complete"], false);
        assert_eq!(record["answers"], json!([]));
        assert_eq!(
            record["normalization_error"],
            "Credential redaction changed answer validation information; request and response content were not retained"
        );
        assert_eq!(record["request_extra"], json!({}));
        assert_eq!(record["response_extra"], json!({}));
        let health = fixture.state.collector.health();
        for field in ["forwarded", "captured", "persisted", "truncated"] {
            assert_eq!(health[field], 1, "{field}");
        }
        for field in [
            "dropped",
            "write_failures",
            "queue_depth",
            "active_captures",
        ] {
            assert_eq!(health[field], 0, "{field}");
        }
        assert!(health["last_gap_at"].as_i64().unwrap() > 0);
    }
}

#[tokio::test]
async fn text_shape_redaction_imports_are_atomic_and_recoverable() {
    let raw = json!({"id":"good-event", "source":"fixture", "provider":"typesafe",
        "timestamp":1800000000000_i64, "status":200, "capture_complete":true,
        "request":{"questions":{"q":{"type":"noul","instructions":"Rule","criteria":{"true":"Allowed"}}}},
        "response":{"answers":{"q":{"type":"noul","noul":0.2}}}});
    let observer =
        model::import_records(&raw.to_string(), "systemone-capture", &Default::default())
            .unwrap()
            .remove(0);
    for field in ["instructions", "true"] {
        for (format, good) in [("systemone-capture", &raw), ("observer-jsonl", &observer)] {
            let fixture =
                fixture_with_config("http://127.0.0.1:1/v1/systemone", 4096, true, |config| {
                    config.redact_keys.push(field.into());
                });
            let mut bad = good.clone();
            let definition = if format == "systemone-capture" {
                &mut bad["request"]["questions"]["q"]
            } else {
                &mut bad["answers"][0]["definition"]
            };
            if field == "instructions" {
                definition[field] = json!(12);
            } else {
                definition["criteria"][field] = json!(12);
            }
            for (text, expected) in [
                (format!("{good}\n{bad}"), None),
                (good.to_string(), Some(json!({"imported":1,"duplicates":0}))),
                (good.to_string(), Some(json!({"imported":0,"duplicates":1}))),
            ] {
                let request = Request::builder()
                    .method("POST")
                    .uri("/api/import")
                    .header("host", "127.0.0.1:8765")
                    .header("x-observer-request", "1")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"format":format,"text":text}).to_string()))
                    .unwrap();
                let response = router(fixture.state.clone())
                    .oneshot(request)
                    .await
                    .unwrap();
                assert_eq!(
                    response.status(),
                    if expected.is_some() {
                        StatusCode::OK
                    } else {
                        StatusCode::BAD_REQUEST
                    }
                );
                let body: Value =
                    serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap())
                        .unwrap();
                if let Some(expected) = expected {
                    assert_eq!(body, expected);
                } else {
                    assert_eq!(
                        body,
                        json!({"error":model::IMPORT_REDACTION_VALIDATION_MESSAGE})
                    );
                    assert!(fixture.state.store.is_empty().unwrap());
                }
                assert_eq!(fixture.state.import_slots.available_permits(), 1);
            }
            fixture.state.collector.shutdown().await;
        }
    }
}

#[tokio::test]
async fn semantic_redaction_keeps_wire_bytes_but_records_an_incomplete_capture() {
    let request_body = br#"{"questions":{"q":{"type":"noul","instructions":"Rule","criteria":{"invalid":"Wrong"}}},"credentials":["criteria"]}"#;
    let response_body = br#"{"answers":{"q":{"type":"noul","noul":0.2}}}"#;
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(move |request: Request| async move {
            assert_eq!(
                to_bytes(request.into_body(), 4096).await.unwrap().as_ref(),
                request_body
            );
            response_body.as_slice()
        }),
    ))
    .await;
    let fixture = fixture(&url, 4096, false);
    let response = router(fixture.state.clone())
        .oneshot(native(request_body.as_slice()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
        response_body
    );
    fixture.state.collector.shutdown().await;
    task.abort();
    let record = saved(&fixture).await;
    assert_eq!(record["capture_complete"], false);
    assert_eq!(record["answers"], json!([]));
    assert_eq!(
        record["normalization_error"],
        "Credential redaction changed answer validation information; request and response content were not retained"
    );
    assert!(!record.to_string().contains("Wrong"));
    let health = fixture.state.collector.health();
    for field in ["forwarded", "captured", "persisted", "truncated"] {
        assert_eq!(health[field], 1, "{field}");
    }
    for field in [
        "dropped",
        "write_failures",
        "queue_depth",
        "active_captures",
    ] {
        assert_eq!(health[field], 0, "{field}");
    }
    assert!(health["last_gap_at"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn semantic_redaction_import_rejection_is_specific_atomic_and_recoverable() {
    let good = json!({"id":"fixture-event", "source":"fixture", "provider":"typesafe",
        "timestamp":1800000000000_i64, "status":200, "capture_complete":true,
        "request":{"questions":{"q":{"type":"noul","instructions":"Rule","criteria":{"invalid":"Wrong"}}}},
        "response":{"answers":{"q":{"type":"noul","noul":0.2}}}});
    let observer =
        model::import_records(&good.to_string(), "systemone-capture", &Default::default())
            .unwrap()
            .remove(0);
    for (format, good) in [("systemone-capture", good), ("observer-jsonl", observer)] {
        let fixture = fixture("http://127.0.0.1:1/v1/systemone", 4096, true);
        let mut bad = good.clone();
        bad[if format == "systemone-capture" {
            "request"
        } else {
            "request_extra"
        }]["credentials"] = json!(["criteria"]);
        for (text, success) in [(format!("{good}\n{bad}"), false), (good.to_string(), true)] {
            let request = Request::builder()
                .method("POST")
                .uri("/api/import")
                .header("host", "127.0.0.1:8765")
                .header("x-observer-request", "1")
                .header("content-type", "application/json")
                .body(Body::from(json!({"format":format,"text":text}).to_string()))
                .unwrap();
            let response = router(fixture.state.clone())
                .oneshot(request)
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if success {
                    StatusCode::OK
                } else {
                    StatusCode::BAD_REQUEST
                }
            );
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap())
                    .unwrap();
            if success {
                assert_eq!(body, json!({"imported":1,"duplicates":0}));
            } else {
                assert_eq!(
                    body,
                    json!({"error":model::IMPORT_REDACTION_VALIDATION_MESSAGE})
                );
                assert!(fixture.state.store.is_empty().unwrap());
            }
            assert_eq!(fixture.state.import_slots.available_permits(), 1);
        }
        fixture.state.collector.shutdown().await;
    }
}

#[tokio::test]
async fn redacted_key_collisions_preserve_wire_bytes_and_report_live_gaps() {
    let request_body = br#"{"questions":{"audit-X":{"type":"noul","instructions":"First"},"audit-Y":{"type":"noul","instructions":"Second"}},"credentials":["X","Y"]}"#;
    let response_body = br#"{"answers":{"audit-Y":{"type":"noul","noul":0.8},"audit-X":{"type":"noul","noul":0.2}}}"#;
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(move |request: Request| async move {
            assert_eq!(
                request.headers()[header::AUTHORIZATION],
                "Bearer test-secret"
            );
            assert_eq!(
                to_bytes(request.into_body(), 4096).await.unwrap().as_ref(),
                request_body
            );
            response_body.as_slice()
        }),
    ))
    .await;
    let fixture = fixture(&url, 4096, false);
    for _ in 0..2 {
        let response = router(fixture.state.clone())
            .oneshot(native(request_body.as_slice()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
            response_body
        );
    }
    fixture.state.collector.shutdown().await;
    task.abort();
    let health = fixture.state.collector.health();
    for field in ["forwarded", "captured", "persisted", "truncated"] {
        assert_eq!(health[field], 2, "{field}");
    }
    for field in [
        "dropped",
        "write_failures",
        "queue_depth",
        "active_captures",
    ] {
        assert_eq!(health[field], 0, "{field}");
    }
    assert!(health["last_gap_at"].as_i64().unwrap() > 0);
    let output = fixture
        .state
        .store
        .export(
            &Filter {
                window: Some("all".into()),
                ..Default::default()
            },
            "jsonl",
        )
        .unwrap();
    let records = output
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_ne!(records[0]["id"], records[1]["id"]);
    for record in records {
        assert_eq!(record["capture_complete"], false);
        assert_eq!(record["answers"], json!([]));
        assert_eq!(
            record["normalization_error"],
            "Credential redaction caused an identifier or object key collision; request and response content were not retained"
        );
        assert!(!record.to_string().contains("audit-"));
    }
}

#[tokio::test]
async fn semantic_identifier_collisions_preserve_wire_bytes_and_report_live_gaps() {
    let request_body = br#"{"questions":{"q":{"type":"choice","instructions":"Choose","criteria":{"option-X":null}}},"credentials":["X","Y"]}"#;
    let response_body = br#"{"answers":{"q":{"type":"choice","choice":"option-Y","probabilities":{"option-Y":1},"confidence":0.7}}}"#;
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(move |request: Request| async move {
            assert_eq!(
                request.headers()[header::AUTHORIZATION],
                "Bearer test-secret"
            );
            assert_eq!(
                to_bytes(request.into_body(), 4096).await.unwrap().as_ref(),
                request_body
            );
            response_body.as_slice()
        }),
    ))
    .await;
    let fixture = fixture(&url, 4096, false);
    for _ in 0..2 {
        let response = router(fixture.state.clone())
            .oneshot(native(request_body.as_slice()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
            response_body
        );
    }
    fixture.state.collector.shutdown().await;
    task.abort();
    let health = fixture.state.collector.health();
    for field in ["forwarded", "captured", "persisted", "truncated"] {
        assert_eq!(health[field], 2, "{field}");
    }
    for field in [
        "dropped",
        "write_failures",
        "queue_depth",
        "active_captures",
    ] {
        assert_eq!(health[field], 0, "{field}");
    }
    assert!(health["last_gap_at"].as_i64().unwrap() > 0);
    let output = fixture
        .state
        .store
        .export(
            &Filter {
                window: Some("all".into()),
                ..Default::default()
            },
            "jsonl",
        )
        .unwrap();
    let records = output
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_ne!(records[0]["id"], records[1]["id"]);
    for record in records {
        assert_eq!(record["capture_complete"], false);
        assert_eq!(record["answers"], json!([]));
        assert_eq!(
            record["normalization_error"],
            "Credential redaction caused an identifier or object key collision; request and response content were not retained"
        );
        assert!(!record.to_string().contains("option-"));
    }
}

#[tokio::test]
async fn redacted_key_collision_imports_are_atomic_and_release_the_slot() {
    let raw = json!({"id":"good-event", "source":"fixture", "provider":"typesafe",
        "timestamp":1800000000000_i64, "status":200, "capture_complete":true,
        "request":{"questions":{}}, "response":{"answers":{}}});
    let observer = model::import_records(
        &raw.to_string(),
        "systemone-capture",
        &model::NormalizeOptions::default(),
    )
    .unwrap()
    .remove(0);
    let receipt: Value =
        serde_json::from_str(include_str!("../fixtures/model/jevrouter-receipt.json")).unwrap();
    for (format, good) in [
        ("systemone-capture", raw),
        ("observer-jsonl", observer),
        ("jevrouter-receipt", receipt),
    ] {
        let fixture = fixture("http://127.0.0.1:1/v1/systemone", 4096, true);
        let mut bad = good.clone();
        let extra = match format {
            "systemone-capture" => &mut bad["request"],
            "observer-jsonl" => {
                bad["request_extra"] = json!({});
                &mut bad["request_extra"]
            }
            _ => &mut bad["raw_jev"],
        };
        extra["credentials"] = json!(["X", "Y"]);
        extra["extension"] = json!({"audit-X":1,"audit-Y":2});
        let valid_text = good.to_string();
        let text = format!("{good}\n{bad}\n{good}");
        for (text, expected, counts) in [
            (text, StatusCode::BAD_REQUEST, None),
            (valid_text.clone(), StatusCode::OK, Some((1, 0))),
            (valid_text, StatusCode::OK, Some((0, 1))),
        ] {
            let request = Request::builder()
                .method("POST")
                .uri("/api/import")
                .header("host", "127.0.0.1:8765")
                .header("x-observer-request", "1")
                .header("content-type", "application/json")
                .body(Body::from(json!({"format":format,"text":text}).to_string()))
                .unwrap();
            let response = router(fixture.state.clone())
                .oneshot(request)
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{format}");
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap())
                    .unwrap();
            if let Some((imported, duplicates)) = counts {
                assert_eq!(body, json!({"imported":imported,"duplicates":duplicates}));
            } else {
                assert!(fixture.state.store.is_empty().unwrap());
                assert_eq!(
                    body,
                    json!({"error":"Import rejected because credential redaction makes distinct identifiers or object keys identical."})
                );
            }
            assert_eq!(fixture.state.import_slots.available_permits(), 1);
        }
        fixture.state.collector.shutdown().await;
    }
}

#[tokio::test]
async fn semantic_identifier_imports_are_atomic_and_release_the_slot() {
    let raw = json!({"id":"good-event", "source":"fixture", "provider":"typesafe",
        "timestamp":1800000000000_i64, "status":200, "capture_complete":true,
        "request":{"questions":{"q":{"type":"choice","instructions":"Choose","criteria":{"option-X":null}}}},
        "response":{"answers":{"q":{"type":"choice","choice":"option-Y","probabilities":{"option-Y":1},"confidence":0.7}}}});
    let observer = model::import_records(
        &raw.to_string(),
        "systemone-capture",
        &model::NormalizeOptions::default(),
    )
    .unwrap()
    .remove(0);
    assert_eq!(observer["answers"][0]["valid"], false);
    for (format, good) in [("systemone-capture", raw), ("observer-jsonl", observer)] {
        let fixture = fixture("http://127.0.0.1:1/v1/systemone", 4096, true);
        let mut bad = good.clone();
        let extra = if format == "systemone-capture" {
            &mut bad["request"]
        } else {
            &mut bad["request_extra"]
        };
        extra["credentials"] = json!(["X", "Y"]);
        let valid_text = good.to_string();
        for (text, expected, counts) in [
            (
                format!("{good}\n{bad}\n{good}"),
                StatusCode::BAD_REQUEST,
                None,
            ),
            (valid_text.clone(), StatusCode::OK, Some((1, 0))),
            (valid_text, StatusCode::OK, Some((0, 1))),
        ] {
            let request = Request::builder()
                .method("POST")
                .uri("/api/import")
                .header("host", "127.0.0.1:8765")
                .header("x-observer-request", "1")
                .header("content-type", "application/json")
                .body(Body::from(json!({"format":format,"text":text}).to_string()))
                .unwrap();
            let response = router(fixture.state.clone())
                .oneshot(request)
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{format}");
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap())
                    .unwrap();
            if let Some((imported, duplicates)) = counts {
                assert_eq!(body, json!({"imported":imported,"duplicates":duplicates}));
            } else {
                assert!(fixture.state.store.is_empty().unwrap());
                assert_eq!(
                    body,
                    json!({"error":"Import rejected because credential redaction makes distinct identifiers or object keys identical."})
                );
            }
            assert_eq!(fixture.state.import_slots.available_permits(), 1);
        }
        fixture.state.collector.shutdown().await;
    }
}

#[tokio::test]
async fn import_identity_redaction_is_atomic_and_releases_the_import_slot() {
    for format in ["observer-jsonl", "jevrouter-receipt"] {
        let fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, true);
        let first = if format == "observer-jsonl" {
            model::sample_records().remove(0)
        } else {
            serde_json::from_str(include_str!("../fixtures/model/jevrouter-receipt.json")).unwrap()
        };
        let mut affected = first.clone();
        let key = if format == "observer-jsonl" {
            "id"
        } else {
            "decision_id"
        };
        affected[key] = json!("private-identity-X");
        affected["credentials"] = json!("X");
        let batch = format!("{}\n{}", first, affected);
        for (text, counts) in [
            (batch.clone(), None),
            (batch, None),
            (first.to_string(), Some((1, 0))),
            (first.to_string(), Some((0, 1))),
        ] {
            let request = Request::builder()
                .method("POST")
                .uri("/api/import")
                .header("host", "127.0.0.1:8765")
                .header("x-observer-request", "1")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"format": format, "text": text}).to_string(),
                ))
                .unwrap();
            let response = router(fixture.state.clone())
                .oneshot(request)
                .await
                .unwrap();
            let expected_status = if counts.is_some() {
                StatusCode::OK
            } else {
                StatusCode::BAD_REQUEST
            };
            assert_eq!(response.status(), expected_status, "{format}");
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap())
                    .unwrap();
            if let Some((imported, duplicates)) = counts {
                assert_eq!(
                    body,
                    json!({"imported": imported, "duplicates": duplicates})
                );
            } else {
                assert_eq!(
                    body,
                    json!({"error": "Import rejected because privacy filtering changes an event identity. Use stable, non-sensitive IDs and source names."})
                );
                assert!(fixture.state.store.is_empty().unwrap());
            }
            assert_eq!(fixture.state.import_slots.available_permits(), 1);
        }
        fixture.state.collector.shutdown().await;
    }
}

#[tokio::test]
async fn invalid_import_timestamps_reject_the_whole_batch_and_release_the_slot() {
    let raw = json!({
        "id":"valid-clock", "source":"time-domain", "provider":"typesafe",
        "timestamp":chrono::Utc::now().timestamp_millis(), "status":200, "capture_complete":true,
        "request":{"questions":{"urgent":{"type":"noul","instructions":"Urgent?"}}},
        "response":{"answers":{"urgent":{"type":"noul","noul":0.8}}}
    });
    let normalized =
        model::import_records(&raw.to_string(), "systemone-capture", &Default::default())
            .unwrap()
            .remove(0);
    for (format, first) in [
        ("observer-jsonl", normalized.clone()),
        ("systemone-capture", raw),
    ] {
        let fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, true);
        let keys: &[&str] = if format == "observer-jsonl" {
            &["timestamp", "imported_at"]
        } else {
            &["timestamp"]
        };
        for &key in keys {
            let mut invalid = first.clone();
            invalid["id"] = json!("invalid-clock");
            invalid[key] = json!(i64::MAX);
            let response = router(fixture.state.clone())
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/import")
                        .header("host", "127.0.0.1:8765")
                        .header("x-observer-request", "1")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({"format":format, "text":format!("{first}\n{invalid}")})
                                .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{format} {key}");
            assert!(
                fixture.state.store.is_empty().unwrap(),
                "Part of invalid batch was saved"
            );
            assert_eq!(fixture.state.import_slots.available_permits(), 1);
        }
        let response = router(fixture.state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/import")
                    .header("host", "127.0.0.1:8765")
                    .header("x-observer-request", "1")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"format":format, "text":first.to_string()}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            fixture
                .state
                .store
                .request(normalized["id"].as_str().unwrap())
                .unwrap()
                .is_some()
        );
        fixture.state.collector.shutdown().await;
    }
}

#[tokio::test]
async fn import_normalization_limits_are_atomic_and_release_the_import_slot() {
    let fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, true);
    let first = model::sample_records().remove(0);
    let id = first["id"].as_str().unwrap();
    let mut oversized = first.clone();
    oversized["id"] = json!("oversized-synthetic-event");
    oversized["source_event_id"] = json!("event-id".repeat(1024));
    oversized["actions"] = json!(vec![json!({}); 2100]);
    let text = format!("{}\n{}", first, oversized);
    assert!(text.len() < 64 * 1024);
    let mut deep = first.clone();
    deep["id"] = json!("overdepth-synthetic-event");
    deep["request_extra"] = json!({"nested":
        (0..crate::normalized_json::MAX_DEPTH - 1).fold(json!(0), |v, _| json!([v]))
    });
    let deep_text = format!("{}\n{}", first, deep);
    assert!(deep_text.len() < 64 * 1024);
    let mut private = first.clone();
    private["id"] = json!("redaction-limited-event");
    private["credentials"] = json!("s".repeat(2 * 1024 * 1024));
    let private_text = format!("{}\n{}", first, private);
    for (text, status, counts) in [
        (text, StatusCode::PAYLOAD_TOO_LARGE, None),
        (private_text, StatusCode::PAYLOAD_TOO_LARGE, None),
        (deep_text, StatusCode::BAD_REQUEST, None),
        ("{}".into(), StatusCode::BAD_REQUEST, None),
        (first.to_string(), StatusCode::OK, Some((1, 0))),
        (first.to_string(), StatusCode::OK, Some((0, 1))),
    ] {
        let request = Request::builder()
            .method("POST")
            .uri("/api/import")
            .header("host", "127.0.0.1:8765")
            .header("x-observer-request", "1")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"format":"observer-jsonl", "text":text}).to_string(),
            ))
            .unwrap();
        let response = router(fixture.state.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap()).unwrap();
        if let Some((imported, duplicates)) = counts {
            assert_eq!(body, json!({"imported":imported, "duplicates":duplicates}));
        } else {
            assert!(fixture.state.store.request(id).unwrap().is_none());
            if status == StatusCode::PAYLOAD_TOO_LARGE {
                assert!(
                    body["error"]
                        .as_str()
                        .unwrap()
                        .contains("normalization limit")
                );
                assert!(!body.to_string().contains("oversized-synthetic-event"));
            }
        }
    }
    assert!(
        fixture
            .state
            .store
            .request("oversized-synthetic-event")
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .state
            .store
            .request("overdepth-synthetic-event")
            .unwrap()
            .is_none()
    );
    fixture.state.collector.shutdown().await;
}

#[tokio::test]
async fn client_cancellation_releases_capture_and_records_incomplete_transfer() {
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(|| async {
            Body::from_stream(async_stream::stream! {
                yield Ok::<_, std::convert::Infallible>(Bytes::from_static(b"{\"answers\":"));
                tokio::time::sleep(Duration::from_secs(10)).await;
                yield Ok(Bytes::from_static(b"{}}"));
            })
        }),
    ))
    .await;
    let fixture = fixture(&url, 1024, false);
    let response = router(fixture.state.clone())
        .oneshot(native("{}"))
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    assert!(stream.next().await.unwrap().is_ok());
    drop(stream);
    let record = saved(&fixture).await;
    assert_eq!(record["capture_complete"], false);
    assert!(record["transport_error"].is_string());
    fixture.state.collector.shutdown().await;
    task.abort();
}

#[tokio::test]
async fn forwards_compressed_bytes_without_mislabeling_encoding() {
    let bytes = vec![
        0x1f_u8, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3, 171, 174, 5, 0, 67, 191, 166, 163, 2, 0, 0, 0,
    ];
    let expected = bytes.clone();
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(move || async move { ([(header::CONTENT_ENCODING, "gzip")], bytes) }),
    ))
    .await;
    let fixture = fixture(&url, 1024, false);
    let response = router(fixture.state.clone())
        .oneshot(native("{}"))
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
        expected.as_slice()
    );
    assert_eq!(saved(&fixture).await["capture_complete"], true);
    fixture.state.collector.shutdown().await;
    task.abort();
}

#[tokio::test]
async fn streaming_first_chunk_is_forwarded_before_upstream_finishes() {
    let finish = Arc::new(tokio::sync::Notify::new());
    let upstream_finish = finish.clone();
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(move || {
            let finish = upstream_finish.clone();
            async move {
                Body::from_stream(async_stream::stream! {
                    yield Ok::<_, std::convert::Infallible>(Bytes::from_static(b"first"));
                    finish.notified().await;
                    yield Ok(Bytes::from_static(b"second"));
                })
            }
        }),
    ))
    .await;
    let fixture = fixture(&url, 1024, false);
    let response = router(fixture.state.clone())
        .oneshot(native("{}"))
        .await
        .unwrap();
    let mut body = response.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(1), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(first.as_ref(), b"first");
    finish.notify_one();
    assert_eq!(body.next().await.unwrap().unwrap().as_ref(), b"second");
    assert!(body.next().await.is_none());
    assert_eq!(saved(&fixture).await["capture_complete"], true);
    fixture.state.collector.shutdown().await;
    task.abort();
}

#[tokio::test]
async fn exhausted_recording_budget_keeps_forwarding_and_health_available() {
    let (url, task) =
        mock(Router::new().route("/v1/systemone", post(|| async { "unchanged" }))).await;
    let fixture = fixture(&url, 1024, false);
    let permits: Vec<_> = (0..4)
        .map(|_| fixture.state.collector.reserve_capture().unwrap())
        .collect();
    let response = router(fixture.state.clone())
        .oneshot(native("{}"))
        .await
        .unwrap();
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
        b"unchanged"
    );
    assert_eq!(fixture.state.collector.health()["dropped"], 1);
    let queries = fixture
        .state
        .database_slots
        .clone()
        .acquire_many_owned(8)
        .await
        .unwrap();
    let request = Request::builder()
        .uri("/api/health")
        .header("host", "127.0.0.1:8765")
        .body(Body::empty())
        .unwrap();
    let response = router(fixture.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let health: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(health["forwarded"], 1);
    assert_eq!(health["dropped"], 1);
    drop(queries);
    drop(permits);
    fixture.state.collector.shutdown().await;
    task.abort();
}

#[tokio::test]
async fn sqlite_write_lock_drops_visibly_without_blocking_proxy_then_recovers() {
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(|| async {
            (
                StatusCode::OK,
                "{\"answers\":{},\"provider_extension\":true}",
            )
        }),
    ))
    .await;
    let fixture = fixture(&url, 1024, false);
    let database_lock = fixture.state.store.writer_connection().unwrap();
    database_lock.execute_batch("BEGIN IMMEDIATE").unwrap();

    // An actual SQLite write transaction prevents the collector from saving.
    // All eight HTTP exchanges must still complete before its 2s busy timeout.
    tokio::time::timeout(Duration::from_secs(1), async {
        for _ in 0..8 {
            let response = router(fixture.state.clone())
                .oneshot(native("{\"questions\":{}}"))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
                br#"{"answers":{},"provider_extension":true}"#
            );
        }
    })
    .await
    .expect("forwarding waited for the locked SQLite writer");

    async fn read_health(fixture: &Fixture) -> Value {
        let request = Request::builder()
            .uri("/api/health")
            .header("host", "127.0.0.1:8765")
            .body(Body::empty())
            .unwrap();
        let response = tokio::time::timeout(
            Duration::from_secs(1),
            router(fixture.state.clone()).oneshot(request),
        )
        .await
        .expect("health waited for SQLite")
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap()
    }

    let blocked = read_health(&fixture).await;
    assert_eq!(blocked["forwarded"], 8);
    assert_eq!(blocked["captured"], 4);
    assert_eq!(blocked["persisted"], 0);
    assert_eq!(blocked["dropped"], 4);
    assert_eq!(blocked["queue_depth"], 4);
    assert_eq!(blocked["write_failures"], 0);
    assert!(blocked["last_gap_at"].as_i64().is_some());
    tokio::time::sleep(Duration::from_millis(75)).await;
    assert!(read_health(&fixture).await["lag_ms"].as_u64().unwrap() >= 50);

    // Keep the strict forwarding/health deadlines above, but allow the
    // separately scheduled SQLite writer headroom on shared CI runners.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let health = read_health(&fixture).await;
            if health["write_failures"].as_u64().unwrap() >= 1 {
                assert!(health["dropped"].as_u64().unwrap() > 4);
                assert_eq!(health["persisted"], 0);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a failed SQLite write was not exposed by health");
    database_lock.execute_batch("ROLLBACK").unwrap();

    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let health = read_health(&fixture).await;
            if health["queue_depth"] == 0 && health["active_captures"] == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("recording did not drain after the SQLite lock was released");
    let before = read_health(&fixture).await;
    assert_eq!(
        before["persisted"].as_u64().unwrap() + before["dropped"].as_u64().unwrap(),
        8
    );
    assert_eq!(before["queued_bytes"], 0);
    assert_eq!(before["lag_ms"], 0);

    let response = router(fixture.state.clone())
        .oneshot(native("{\"questions\":{}}"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = to_bytes(response.into_body(), 1024).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let health = read_health(&fixture).await;
            if health["persisted"].as_u64().unwrap() == before["persisted"].as_u64().unwrap() + 1 {
                assert_eq!(health["forwarded"], 9);
                assert_eq!(health["dropped"], before["dropped"]);
                assert_eq!(health["write_failures"], before["write_failures"]);
                assert!(health["last_persisted_at"].is_number());
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the collector did not recover for subsequent requests");
    fixture.state.collector.shutdown().await;
    task.abort();
}

#[tokio::test]
async fn canceled_export_work_holds_its_slot_until_the_blocking_page_finishes() {
    for shutdown in [false, true] {
        let fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, true);
        let state = fixture.state.clone();
        let lease = Arc::new(state.export_slots.clone().try_acquire_owned().unwrap());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let task = tokio::spawn(async move {
            tokio::select! {
                _ = export_database(&state, lease, move || {
                    started_tx.send(()).unwrap();
                    finish_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(())
                }) => {},
                _ = state.shutdown.cancelled() => {},
            }
        });
        tokio::time::timeout(Duration::from_secs(2), started_rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fixture.state.export_slots.available_permits(), 0);
        if shutdown {
            fixture.state.shutdown.cancel();
            task.await.unwrap();
        } else {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        }
        assert_eq!(
            fixture.state.export_slots.available_permits(),
            0,
            "Canceling the response must not admit a second export while its page still runs"
        );
        finish_tx.send(()).unwrap();
        let released = tokio::time::timeout(
            Duration::from_secs(2),
            fixture.state.export_slots.clone().acquire_owned(),
        )
        .await
        .unwrap()
        .unwrap();
        drop(released);
        assert_eq!(fixture.state.export_slots.available_permits(), 1);
        fixture.state.collector.shutdown().await;
    }
}

#[tokio::test]
async fn export_streams_all_pages_with_one_header_and_excludes_later_arrivals() {
    let fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, true);
    let mut base = model::sample_records().into_iter().next().unwrap();
    base["model"] = json!("export-fixture");
    let records: Vec<_> = (0..205)
        .map(|index| {
            let mut record = base.clone();
            record["id"] = json!(format!("export-{index}"));
            record["source_event_id"] = Value::Null;
            record
        })
        .collect();
    let mut connection = fixture.state.store.writer_connection().unwrap();
    fixture
        .state
        .store
        .write_batch(&mut connection, &records)
        .unwrap();
    fn request(format: &str) -> Request {
        Request::builder()
            .uri(format!(
                "/api/export?window=all&model=export-fixture&format={format}"
            ))
            .header("host", "127.0.0.1:8765")
            .body(Body::empty())
            .unwrap()
    }

    let response = router(fixture.state.clone())
        .oneshot(request("jsonl"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/x-ndjson; charset=utf-8"
    );
    let concurrent = router(fixture.state.clone())
        .oneshot(request("csv"))
        .await
        .unwrap();
    assert_eq!(concurrent.status(), StatusCode::TOO_MANY_REQUESTS);
    let mut late = base.clone();
    late["id"] = json!("arrived-after-export-start");
    late["source_event_id"] = Value::Null;
    fixture
        .state
        .store
        .write_batch(&mut connection, &[late])
        .unwrap();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    let exported: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(exported.len(), 205);
    assert!(
        exported
            .iter()
            .all(|record| record["id"] != "arrived-after-export-start")
    );
    let unique: std::collections::BTreeSet<_> =
        exported.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(unique.len(), 205);

    let response = router(fixture.state.clone())
        .oneshot(request("csv"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/csv; charset=utf-8"
    );
    let text = String::from_utf8(
        to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(
        text.lines()
            .filter(|line| line.starts_with("id,timestamp,"))
            .count(),
        1
    );
    assert_eq!(text.lines().count(), 207);

    let canceled = router(fixture.state.clone())
        .oneshot(request("jsonl"))
        .await
        .unwrap();
    assert_eq!(canceled.status(), StatusCode::OK);
    drop(canceled);
    assert_eq!(fixture.state.export_slots.available_permits(), 1);
    fixture.state.collector.shutdown().await;
}

#[tokio::test]
async fn label_outcomes_preserve_distinct_not_found_errors_and_replacement() {
    let fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, true);
    let record = model::sample_records().into_iter().next().unwrap();
    let id = record["id"].as_str().unwrap();
    let key = record["answers"][0]["key"].as_str().unwrap();
    fixture
        .state
        .store
        .write_batch(
            &mut fixture.state.store.writer_connection().unwrap(),
            std::slice::from_ref(&record),
        )
        .unwrap();
    for (request_id, answer, label, status, body) in [
        (
            "absent",
            key,
            "correct",
            StatusCode::NOT_FOUND,
            json!({"error":"Request not found"}),
        ),
        (
            id,
            "absent",
            "correct",
            StatusCode::NOT_FOUND,
            json!({"error":"Answer key not found"}),
        ),
        (
            id,
            key,
            "invalid",
            StatusCode::BAD_REQUEST,
            json!({"error":"Choose correct, incorrect, or unknown for an answer key"}),
        ),
        (id, key, "correct", StatusCode::OK, json!({"ok":true})),
        (id, key, "incorrect", StatusCode::OK, json!({"ok":true})),
    ] {
        let request = Request::builder()
            .method("POST")
            .uri(format!("/api/requests/{request_id}/label"))
            .header("host", "127.0.0.1:8765")
            .header("x-observer-request", "1")
            .header("content-type", "application/json")
            .body(Body::from(json!({"key":answer,"label":label}).to_string()))
            .unwrap();
        let response = router(fixture.state.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let actual: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap()).unwrap();
        assert_eq!(actual, body);
    }
    let reviewed = fixture.state.store.request(id).unwrap().unwrap();
    assert_eq!(reviewed["labels"].as_array().unwrap().len(), 1);
    assert_eq!(reviewed["labels"][0]["key"], key);
    assert_eq!(reviewed["labels"][0]["label"], "incorrect");
    fixture.state.collector.shutdown().await;
}

#[tokio::test]
async fn invalid_local_inputs_do_not_masquerade_as_database_failures() {
    let fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, true);
    for path in [
        "/api/dashboard?window=wrong",
        "/api/groups/unknown?window=wrong",
        "/api/export?window=wrong&format=csv",
    ] {
        let request = Request::builder()
            .uri(path)
            .header("host", "127.0.0.1:8765")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            router(fixture.state.clone())
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let record = model::sample_records().into_iter().next().unwrap();
    let mut connection = fixture.state.store.writer_connection().unwrap();
    fixture
        .state
        .store
        .write_batch(&mut connection, std::slice::from_ref(&record))
        .unwrap();
    let path = format!("/api/requests/{}/label", record["id"].as_str().unwrap());
    let request = Request::builder()
        .method("POST")
        .uri(&path)
        .header("host", "127.0.0.1:8765")
        .header("x-observer-request", "1")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"key":"absent-answer","label":"correct"}"#))
        .unwrap();
    assert_eq!(
        router(fixture.state.clone())
            .oneshot(request)
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let request = Request::builder()
        .method("POST")
        .uri(&path)
        .header("host", "127.0.0.1:8765")
        .header("x-observer-request", "1")
        .header("content-type", "application/json")
        .body(Body::from(vec![b' '; 8193]))
        .unwrap();
    assert_eq!(
        router(fixture.state.clone())
            .oneshot(request)
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    fixture.state.collector.shutdown().await;
}

#[tokio::test]
async fn shutdown_stops_waiting_for_http_and_still_drains_the_writer() {
    let drained = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let marker = drained.clone();
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        drain_after_http_grace(
            std::future::pending::<()>(),
            async move {
                marker.store(true, std::sync::atomic::Ordering::SeqCst);
            },
            Duration::from_millis(10),
        ),
    )
    .await
    .expect("HTTP grace did not stop waiting");
    assert!(result.is_none());
    assert!(drained.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn ordinary_shutdown_preserves_server_result_and_drains() {
    let mut drained = false;
    let result = drain_after_http_grace(
        async { 42 },
        async {
            drained = true;
        },
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(result, Some(42));
    assert!(drained);
}

#[tokio::test]
async fn watchdog_runs_when_writer_drain_and_async_executor_are_blocked() {
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let drain = async move {
        let _ = entered_tx.send(());
        std::future::pending::<()>().await;
    };
    let runner = tokio::spawn(drain_after_http_grace(
        async {},
        drain,
        Duration::from_millis(10),
    ));
    entered_rx.await.unwrap();
    let (deadline_tx, deadline_rx) = std::sync::mpsc::channel();
    let watchdog = spawn_deadline_watchdog(Duration::from_millis(20), move || {
        deadline_tx.send(()).unwrap();
    })
    .unwrap();
    // Deliberately block this single-threaded async executor. The production
    // callback exits the process; the injected callback safely signals us.
    deadline_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("watchdog depended on the blocked async executor");
    assert!(!runner.is_finished());
    runner.abort();
    let _ = runner.await;
    watchdog.join().unwrap();
}
