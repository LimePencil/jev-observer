use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct TrackedUpstream {
    chunks: VecDeque<Bytes>,
    pending: bool,
    dropped: Arc<AtomicBool>,
    polls: Arc<AtomicUsize>,
}

impl Stream for TrackedUpstream {
    type Item = reqwest::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        if let Some(chunk) = self.chunks.pop_front() {
            Poll::Ready(Some(Ok(chunk)))
        } else if self.pending {
            Poll::Pending
        } else {
            Poll::Ready(None)
        }
    }
}

impl Drop for TrackedUpstream {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

struct BodyProbe {
    body: ForwardResponseBody,
    dropped: Arc<AtomicBool>,
    polls: Arc<AtomicUsize>,
}

fn probe(
    fixture: &Fixture,
    chunks: &[&'static [u8]],
    length: Option<u64>,
    complete: bool,
    recording: bool,
    pending: bool,
) -> BodyProbe {
    let dropped = Arc::new(AtomicBool::new(false));
    let polls = Arc::new(AtomicUsize::new(0));
    let upstream = TrackedUpstream {
        chunks: chunks
            .iter()
            .map(|bytes| Bytes::from_static(bytes))
            .collect(),
        pending,
        dropped: dropped.clone(),
        polls: polls.clone(),
    };
    let capture = recording.then(|| {
        let mut state = capturing(1024);
        state.capture.id = uuid::Uuid::new_v4().to_string();
        state.capture.timestamp = chrono::Utc::now().timestamp_millis();
        state.capture.source = "deadline-fixture".into();
        state.capture.status = 200;
        state.capture.request = br#"{"questions":{}}"#.to_vec();
        state.request_done = true;
        state.response_length = length;
        state.response_done = complete;
        Arc::new(Mutex::new(state))
    });
    let guard = capture.as_ref().map(|shared| CaptureGuard {
        shared: shared.clone(),
        collector: fixture.state.collector.clone(),
        permit: Some(fixture.state.collector.reserve_capture().unwrap()),
    });
    fixture.state.collector.record_forwarded();
    let body = ForwardResponseBody::new(
        upstream,
        guard,
        capture,
        length,
        complete,
        tokio::time::Instant::now() + Duration::from_millis(50),
        fixture.state.shutdown.clone(),
    );
    BodyProbe {
        body,
        dropped,
        polls,
    }
}

async fn wait_dropped(probe: &BodyProbe) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !probe.dropped.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("upstream was retained until the downstream body was polled");
}

#[tokio::test]
async fn response_deadline_drops_unpolled_upstream_with_and_without_recording() {
    for recording in [false, true] {
        let fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, false);
        let mut probe = probe(&fixture, &[], Some(20), false, recording, true);
        wait_dropped(&probe).await;
        assert_eq!(probe.polls.load(Ordering::SeqCst), 0, "no read-ahead");
        assert_eq!(
            probe.body.next().await.unwrap().unwrap_err().kind(),
            std::io::ErrorKind::TimedOut
        );
        assert!(probe.body.next().await.is_none());
        drop(probe.body);
        fixture.state.collector.shutdown().await;
        let health = fixture.state.collector.health();
        assert_eq!(health["active_captures"], 0);
        assert_eq!(health["captured"], usize::from(recording));
        assert_eq!(health["persisted"], usize::from(recording));
        assert_eq!(health["truncated"], usize::from(recording));
        if recording {
            let record = saved(&fixture).await;
            assert_eq!(record["status"], 200);
            assert_eq!(record["transport_error"], "Upstream response timed out");
        }
    }
}

#[tokio::test]
async fn response_deadline_preserves_partial_bytes_and_does_not_read_ahead() {
    let fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, false);
    let mut probe = probe(
        &fixture,
        &[b"first", b"second", b"unread"],
        None,
        false,
        true,
        true,
    );
    assert_eq!(probe.body.next().await.unwrap().unwrap(), "first");
    assert_eq!(probe.body.next().await.unwrap().unwrap(), "second");
    {
        let state = probe.body.state.lock().unwrap();
        assert_eq!(
            state
                .capture
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .capture
                .response,
            b"firstsecond"
        );
    }
    wait_dropped(&probe).await;
    assert_eq!(probe.polls.load(Ordering::SeqCst), 2);
    assert_eq!(
        probe.body.next().await.unwrap().unwrap_err().kind(),
        std::io::ErrorKind::TimedOut
    );
    assert!(probe.body.next().await.is_none());
    fixture.state.collector.shutdown().await;
    let record = saved(&fixture).await;
    assert_eq!(record["capture_complete"], false);
    assert_eq!(record["status"], 200);
    assert_eq!(fixture.state.collector.health()["captured"], 1);
}

#[tokio::test]
async fn response_deadline_keeps_observed_length_empty_and_real_eof_successful() {
    for (length, empty, pending) in [
        (Some(14), false, true),
        (Some(0), true, true),
        (None, false, false),
    ] {
        let fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, false);
        let chunks: &[&[u8]] = if empty { &[] } else { &[br#"{"answers":{}}"#] };
        let mut probe = probe(&fixture, chunks, length, empty, true, pending);
        if !empty {
            assert_eq!(
                probe.body.next().await.unwrap().unwrap(),
                br#"{"answers":{}}"#.as_slice()
            );
        }
        if !pending {
            assert!(probe.body.next().await.is_none());
        }
        wait_dropped(&probe).await;
        assert!(probe.body.next().await.is_none());
        fixture.state.collector.shutdown().await;
        let record = saved(&fixture).await;
        assert_eq!(record["capture_complete"], true);
        assert!(record["transport_error"].is_null());
        assert_eq!(fixture.state.collector.health()["truncated"], 0);
    }
}

#[tokio::test]
async fn response_deadline_body_drop_releases_resources_before_first_poll() {
    let fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, false);
    let probe = probe(&fixture, &[], None, false, true, true);
    let weak = Arc::downgrade(&probe.body.state);
    drop(probe.body);
    assert!(probe.dropped.load(Ordering::SeqCst));
    assert_eq!(probe.polls.load(Ordering::SeqCst), 0);
    tokio::time::timeout(Duration::from_secs(2), async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the aborted watcher retained response state");
    fixture.state.collector.shutdown().await;
    assert_eq!(fixture.state.collector.health()["captured"], 1);
    assert_eq!(fixture.state.collector.health()["active_captures"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn response_deadline_shutdown_and_body_drop_finalize_once() {
    for drop_first in [false, true] {
        let fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, false);
        let mut probe = probe(&fixture, &[b"prefix"], None, false, true, true);
        assert_eq!(probe.body.next().await.unwrap().unwrap(), "prefix");
        fixture.state.shutdown.cancel();
        if !drop_first {
            wait_dropped(&probe).await;
            assert_eq!(
                probe.body.next().await.unwrap().unwrap_err().kind(),
                std::io::ErrorKind::ConnectionAborted
            );
            assert!(probe.body.next().await.is_none());
        }
        drop(probe.body);
        assert!(probe.dropped.load(Ordering::SeqCst));
        tokio::time::timeout(Duration::from_secs(2), async {
            while fixture.state.collector.health()["captured"] != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the winning terminal transition did not finalize capture");
        fixture.state.collector.shutdown().await;
        let health = fixture.state.collector.health();
        for field in ["captured", "persisted", "truncated"] {
            assert_eq!(health[field], 1, "{field}");
        }
        for field in ["dropped", "queue_depth", "active_captures"] {
            assert_eq!(health[field], 0, "{field}");
        }
    }
}

#[tokio::test]
async fn response_deadline_unpolled_http_body_releases_capture_slot_for_normal_requests() {
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(|request: Request| async move {
            let bytes = to_bytes(request.into_body(), 1024).await.unwrap();
            if bytes == br#"{"normal":true}"#.as_slice() {
                Body::from(br#"{"answers":{}}"#.as_slice())
            } else {
                Body::from_stream(futures_util::stream::pending::<std::io::Result<Bytes>>())
            }
        }),
    ))
    .await;
    let fixture = fixture_with_config(&url, 1024, false, |config| config.upstream_timeout_secs = 1);
    let held: Vec<_> = (0..3)
        .map(|_| fixture.state.collector.reserve_capture().unwrap())
        .collect();
    let response = router(fixture.state.clone())
        .oneshot(native("{}"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.state.collector.health()["persisted"] != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("unpolled body kept the only available capture slot past its deadline");
    let normal = router(fixture.state.clone())
        .oneshot(native(br#"{"normal":true}"#.as_slice()))
        .await
        .unwrap();
    assert_eq!(normal.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(normal.into_body(), 1024).await.unwrap(),
        br#"{"answers":{}}"#.as_slice()
    );
    let mut body = response.into_body().into_data_stream();
    assert!(body.next().await.unwrap().is_err());
    assert!(body.next().await.is_none());
    drop(held);
    fixture.state.collector.shutdown().await;
    let health = fixture.state.collector.health();
    assert_eq!(health["forwarded"], 2);
    assert_eq!(health["captured"], 2);
    assert_eq!(health["persisted"], 2);
    assert_eq!(health["truncated"], 1);
    assert_eq!(health["dropped"], 0);
    assert_eq!(health["active_captures"], 0);
    task.abort();
}

#[tokio::test]
async fn response_deadline_uses_remaining_time_after_headers() {
    let (url, task) = mock(Router::new().route(
        "/v1/systemone",
        post(|| async {
            tokio::time::sleep(Duration::from_millis(1200)).await;
            Body::from_stream(futures_util::stream::pending::<std::io::Result<Bytes>>())
        }),
    ))
    .await;
    let fixture = fixture_with_config(&url, 1024, false, |config| config.upstream_timeout_secs = 2);
    let response = router(fixture.state.clone())
        .oneshot(native("{}"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // Only about 0.8s of the original deadline remains. Allow writer/CI
    // scheduling headroom while rejecting a new 2s timeout after the headers.
    tokio::time::timeout(Duration::from_millis(1400), async {
        while fixture.state.collector.health()["persisted"] != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("response headers reset the total upstream deadline");
    assert!(to_bytes(response.into_body(), 1024).await.is_err());
    assert_eq!(
        saved(&fixture).await["transport_error"],
        "Upstream response timed out"
    );
    fixture.state.collector.shutdown().await;
    task.abort();
}
