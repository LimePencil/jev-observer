use std::{
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll, Waker},
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::Response,
};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use tokio::sync::OwnedSemaphorePermit;
use tokio_util::sync::CancellationToken;

use super::{AppState, error};
use crate::{
    collector::Collector,
    config::UpstreamAuth,
    credentials::LOCAL_TOKEN_PREFIX,
    model::{self, Capture},
};

fn metadata(headers: &HeaderMap, key: &'static str) -> Option<String> {
    headers
        .get(key)
        .and_then(|h| h.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
        .map(ToOwned::to_owned)
}

pub(super) fn strip_hop_headers(headers: &mut HeaderMap) {
    // Transfer framing overrides Content-Length. Once that framing is decoded
    // and removed, forwarding its stale length can truncate the streamed body.
    if headers.contains_key(header::TRANSFER_ENCODING) {
        headers.remove(header::CONTENT_LENGTH);
    } else {
        let mut lengths = headers.get_all(header::CONTENT_LENGTH).iter();
        let combined = lengths
            .next()
            .is_some_and(|value| value.as_bytes().contains(&b',') || lengths.next().is_some());
        if combined {
            // The decoder accepts identical comma-separated or repeated lengths,
            // but the outgoing stream encoder requires one decimal field value.
            if let Some(length) = content_length(headers) {
                headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
            }
        }
    }
    let named: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect();
    for name in named {
        headers.remove(name);
    }
    for name in [
        "connection",
        "proxy-connection",
        "keep-alive",
        "transfer-encoding",
        "te",
        "trailer",
        "upgrade",
        "proxy-authenticate",
        "proxy-authorization",
    ] {
        headers.remove(name);
    }
    let observer: Vec<_> = headers
        .keys()
        .filter(|k| k.as_str().starts_with("x-observer-"))
        .cloned()
        .collect();
    for name in observer {
        headers.remove(name);
    }
}

fn strip_local_request_headers(headers: &mut HeaderMap) {
    // These describe the local browser or proxy hop, not the provider call.
    // In particular, Referer can include private dashboard paths or queries.
    for name in ["origin", "referer", "forwarded", "x-real-ip"] {
        headers.remove(name);
    }
    let local: Vec<_> = headers
        .keys()
        .filter(|name| {
            let name = name.as_str();
            name.starts_with("sec-fetch-") || name.starts_with("x-forwarded-")
        })
        .cloned()
        .collect();
    for name in local {
        headers.remove(name);
    }
}

pub(super) fn content_length(headers: &HeaderMap) -> Option<u64> {
    if headers.contains_key(header::TRANSFER_ENCODING) {
        return None;
    }
    let mut length = None;
    for value in headers.get_all(header::CONTENT_LENGTH) {
        for part in value.to_str().ok()?.split(',') {
            let part = part.trim();
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let parsed = part.parse::<u64>().ok()?;
            if length.is_some_and(|length| length != parsed) {
                return None;
            }
            length = Some(parsed);
        }
    }
    length
}

pub(super) fn supported_transfer_encoding(headers: &HeaderMap) -> bool {
    let mut chunked = false;
    for value in headers.get_all(header::TRANSFER_ENCODING) {
        let Ok(value) = value.to_str() else {
            return false;
        };
        for coding in value.split(',') {
            if chunked || !coding.trim().eq_ignore_ascii_case("chunked") {
                return false;
            }
            chunked = true;
        }
    }
    true
}

fn encoded(headers: &HeaderMap) -> bool {
    headers.get_all(header::CONTENT_ENCODING).iter().any(|v| {
        v.to_str().map_or(true, |s| {
            s.split(',')
                .any(|p| !p.trim().eq_ignore_ascii_case("identity"))
        })
    })
}

pub(super) struct Capturing {
    pub(super) capture: Capture,
    pub(super) started: Instant,
    pub(super) limit: usize,
    pub(super) request_bytes: u64,
    pub(super) response_bytes: u64,
    pub(super) request_length: Option<u64>,
    pub(super) response_length: Option<u64>,
    pub(super) request_done: bool,
    pub(super) response_done: bool,
    pub(super) finalized: bool,
}

impl Capturing {
    pub(super) fn append(&mut self, data: &[u8], response: bool) {
        let body = if response {
            &mut self.capture.response
        } else {
            &mut self.capture.request
        };
        let available = self.limit.saturating_sub(body.len());
        let append = data.len().min(available);
        if body.len() + append > body.capacity() {
            // Amortize streamed chunks without letting Vec's implicit growth
            // exceed the configured per-body capture memory budget.
            let capacity = body
                .capacity()
                .saturating_mul(2)
                .max(body.len() + append)
                .min(self.limit);
            body.reserve_exact(capacity - body.len());
        }
        body.extend_from_slice(&data[..append]);
        if data.len() > available {
            self.capture.capture_complete = false;
        }
        if response {
            self.response_bytes += data.len() as u64;
            if self
                .response_length
                .is_some_and(|length| self.response_bytes >= length)
            {
                self.response_done = true;
            }
        } else {
            self.request_bytes += data.len() as u64;
            if self
                .request_length
                .is_some_and(|length| self.request_bytes >= length)
            {
                self.request_done = true;
            }
        }
    }
}

pub(super) type SharedCapture = Arc<Mutex<Capturing>>;

fn with_capture(capture: &Option<SharedCapture>, f: impl FnOnce(&mut Capturing)) {
    if let Some(capture) = capture {
        let mut capture = capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !capture.finalized {
            f(&mut capture);
        }
    }
}

/// Drop also handles downstream cancellation, including a body never polled.
pub(super) struct CaptureGuard {
    pub(super) shared: SharedCapture,
    pub(super) collector: Collector,
    pub(super) permit: Option<OwnedSemaphorePermit>,
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        let Some(permit) = self.permit.take() else {
            return;
        };
        let mut state = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.finalized = true;
        let complete = state.request_done && state.response_done;
        state.capture.capture_complete &= complete && state.capture.transport_error.is_none();
        if !complete && state.capture.transport_error.is_none() {
            state.capture.transport_error =
                Some("Transfer ended before the complete exchange was observed".into());
        }
        state.capture.duration_ms = state.started.elapsed().as_secs_f64() * 1000.0;
        // Move buffers into the queue; do not duplicate the capture memory budget.
        let capture = Capture {
            id: std::mem::take(&mut state.capture.id),
            timestamp: state.capture.timestamp,
            source: std::mem::take(&mut state.capture.source),
            task_version: state.capture.task_version.take(),
            adapter: state.capture.adapter.take(),
            status: state.capture.status,
            duration_ms: state.capture.duration_ms,
            request: std::mem::take(&mut state.capture.request),
            response: std::mem::take(&mut state.capture.response),
            capture_complete: state.capture.capture_complete,
            transport_error: state.capture.transport_error.take(),
            secret: state.capture.secret.take(),
            local_token: state.capture.local_token.take(),
            access_token: state.capture.access_token.take(),
        };
        drop(state);
        self.collector.submit(capture, permit);
    }
}

type UpstreamBody = Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>;
type ResponseFailure = (std::io::ErrorKind, &'static str);

enum ResponseEnd {
    Complete,
    Failed,
    Deadline,
    Shutdown,
    ClientDrop,
}

pub(super) struct ResponseState {
    upstream: Option<UpstreamBody>,
    guard: Option<CaptureGuard>,
    pub(super) capture: Option<SharedCapture>,
    length: Option<u64>,
    observed: u64,
    complete: bool,
    failure: Option<ResponseFailure>,
    waker: Option<Waker>,
}

// Taking these resources under the state lock makes terminalization exactly
// once. Drop the upstream and finalize capture outside it: their destructors
// and the downstream waker must not reenter a locked response state.
struct FinishedResponse {
    upstream: UpstreamBody,
    guard: Option<CaptureGuard>,
    capture: Option<SharedCapture>,
    failure: Option<ResponseFailure>,
    waker: Option<Waker>,
}

impl FinishedResponse {
    fn finish(self) {
        if let Some((_, message)) = self.failure {
            with_capture(&self.capture, |state| {
                state.capture.transport_error = Some(message.into());
            });
        }
        drop(self.upstream);
        drop(self.guard);
        if let Some(waker) = self.waker {
            waker.wake();
        }
    }
}

impl ResponseState {
    fn end(&mut self, reason: ResponseEnd) -> Option<FinishedResponse> {
        let upstream = self.upstream.take()?;
        let failure = match reason {
            ResponseEnd::Failed => Some((
                std::io::ErrorKind::UnexpectedEof,
                "Upstream response transfer failed",
            )),
            ResponseEnd::Deadline if !self.complete => {
                Some((std::io::ErrorKind::TimedOut, "Upstream response timed out"))
            }
            ResponseEnd::Shutdown if !self.complete => Some((
                std::io::ErrorKind::ConnectionAborted,
                "Observer shut down during response transfer",
            )),
            _ => None,
        };
        self.failure = failure;
        Some(FinishedResponse {
            upstream,
            guard: self.guard.take(),
            capture: self.capture.take(),
            failure,
            waker: self.waker.take(),
        })
    }

    fn terminal_poll(&mut self) -> Poll<Option<std::io::Result<Bytes>>> {
        Poll::Ready(
            self.failure
                .take()
                .map(|(kind, message)| Err(std::io::Error::new(kind, message))),
        )
    }
}

/// Reads remain demand-driven, with no additional frame queue or byte copies.
/// The independent watcher can drop unfinished upstream work and the capture
/// permit even while a slow client prevents Hyper from polling this body.
pub(super) struct ForwardResponseBody {
    pub(super) state: Arc<Mutex<ResponseState>>,
    watcher: Option<tokio::task::JoinHandle<()>>,
}

impl ForwardResponseBody {
    pub(super) fn new(
        upstream: impl Stream<Item = reqwest::Result<Bytes>> + Send + 'static,
        guard: Option<CaptureGuard>,
        capture: Option<SharedCapture>,
        length: Option<u64>,
        complete: bool,
        deadline: tokio::time::Instant,
        shutdown: CancellationToken,
    ) -> Self {
        let state = Arc::new(Mutex::new(ResponseState {
            upstream: Some(Box::pin(upstream)),
            guard,
            capture,
            length,
            observed: 0,
            complete,
            failure: None,
            waker: None,
        }));
        let watched = state.clone();
        let watcher = tokio::spawn(async move {
            let reason = tokio::select! {
                _ = tokio::time::sleep_until(deadline) => ResponseEnd::Deadline,
                _ = shutdown.cancelled() => ResponseEnd::Shutdown,
            };
            let finished = watched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .end(reason);
            if let Some(finished) = finished {
                finished.finish();
            }
        });
        Self {
            state,
            watcher: Some(watcher),
        }
    }

    fn abort_watcher(&mut self) {
        if let Some(watcher) = self.watcher.take() {
            watcher.abort();
        }
    }
}

impl Stream for ForwardResponseBody {
    type Item = std::io::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let (result, finished, terminal) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.upstream.is_none() {
                (state.terminal_poll(), None, true)
            } else {
                state.waker = Some(cx.waker().clone());
                match state.upstream.as_mut().unwrap().as_mut().poll_next(cx) {
                    Poll::Pending => (Poll::Pending, None, false),
                    Poll::Ready(Some(Ok(bytes))) => {
                        state.observed += bytes.len() as u64;
                        state.complete |=
                            state.length.is_some_and(|length| state.observed >= length);
                        with_capture(&state.capture, |capture| capture.append(&bytes, true));
                        (Poll::Ready(Some(Ok(bytes))), None, false)
                    }
                    Poll::Ready(Some(Err(error))) => {
                        let finished = state.end(if error.is_timeout() {
                            ResponseEnd::Deadline
                        } else {
                            ResponseEnd::Failed
                        });
                        (state.terminal_poll(), finished, true)
                    }
                    Poll::Ready(None) => {
                        with_capture(&state.capture, |capture| capture.response_done = true);
                        let finished = state.end(ResponseEnd::Complete);
                        (state.terminal_poll(), finished, true)
                    }
                }
            }
        };
        if terminal {
            self.abort_watcher();
        }
        if let Some(finished) = finished {
            finished.finish();
        }
        result
    }
}

impl Drop for ForwardResponseBody {
    fn drop(&mut self) {
        self.abort_watcher();
        let finished = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .end(ResponseEnd::ClientDrop);
        if let Some(finished) = finished {
            finished.finish();
        }
    }
}

pub(super) async fn proxy(State(state): State<AppState>, request: Request) -> Response {
    if state.config.demo {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Forwarding is disabled in demo mode",
        );
    }
    if request.uri().query().is_some() {
        return error(
            StatusCode::BAD_REQUEST,
            "The native endpoint does not accept query parameters",
        );
    }
    let (parts, body) = request.into_parts();
    let mut headers = parts.headers;
    let basic_fallback = state.access.as_ref().is_some_and(|access| {
        access.allows_basic(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
        )
    });
    let (authorization, local_token) = if state.config.upstream_auth == UpstreamAuth::None {
        let bearer = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
            .map(|(_, token)| token.trim());
        if state.access.as_ref().is_some_and(|access| {
            !basic_fallback
                && !access.allows_token(bearer)
                && !access.allows_token(
                    headers
                        .get("x-observer-access")
                        .and_then(|value| value.to_str().ok()),
                )
        }) {
            return error(
                StatusCode::UNAUTHORIZED,
                "Observer access token required for a local model",
            );
        }
        if !headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
        {
            return error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Local model requests require application/json",
            );
        }
        (None, None)
    } else {
        match headers
            .get(header::AUTHORIZATION)
            .filter(|_| !basic_fallback)
        {
            Some(value) => {
                let local_token = value
                    .to_str()
                    .ok()
                    .and_then(|text| text.split_once(' '))
                    .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
                    .map(|(_, token)| token.trim());
                if let Some(token) =
                    local_token.filter(|token| token.starts_with(LOCAL_TOKEN_PREFIX))
                {
                    let Some(key) = state.credentials.provider_for_token(token) else {
                        return error(StatusCode::UNAUTHORIZED, "Local client token is invalid");
                    };
                    match HeaderValue::from_str(&format!("Bearer {key}")) {
                        Ok(value) => (Some(value), Some(token.to_owned())),
                        Err(_) => {
                            return error(
                                StatusCode::SERVICE_UNAVAILABLE,
                                "Saved provider key is invalid",
                            );
                        }
                    }
                } else {
                    if state.access.as_ref().is_some_and(|access| {
                        !access.allows_token(
                            headers
                                .get("x-observer-access")
                                .and_then(|value| value.to_str().ok()),
                        )
                    }) {
                        return error(
                            StatusCode::UNAUTHORIZED,
                            "Local access token required with a provider key",
                        );
                    }
                    (Some(value.clone()), None)
                }
            }
            None => {
                if state.access.as_ref().is_some_and(|access| {
                    !basic_fallback
                        && !access.allows_token(
                            headers
                                .get("x-observer-access")
                                .and_then(|value| value.to_str().ok()),
                        )
                }) {
                    return error(
                        StatusCode::UNAUTHORIZED,
                        "Local access token required for fallback credentials",
                    );
                }
                let Some(value) = state
                    .config
                    .api_key
                    .as_ref()
                    .and_then(|key| HeaderValue::from_str(&format!("Bearer {key}")).ok())
                else {
                    return error(
                        StatusCode::UNAUTHORIZED,
                        "Provide a Bearer credential or set TYPESAFE_API_KEY",
                    );
                };
                // Browser forms and no-CORS fetches can send a simple POST to
                // loopback without setting Authorization. Do not let one spend
                // the Observer process's fallback provider credential.
                if !headers
                    .get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.split(';').next())
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
                {
                    return error(
                        StatusCode::UNSUPPORTED_MEDIA_TYPE,
                        "Fallback credentials require application/json",
                    );
                }
                (Some(value), None)
            }
        }
    };
    // Basic dashboard authentication is replaced before forwarding, just like
    // a local client token. Retain the exact accepted header for capture-only
    // redaction; its encoded component can also authenticate the owner.
    let local_token = local_token.or_else(|| {
        headers
            .get(header::AUTHORIZATION)
            .filter(|_| basic_fallback)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned)
    });
    let secret = authorization
        .as_ref()
        .or_else(|| headers.get(header::AUTHORIZATION))
        .and_then(|authorization| authorization.to_str().ok())
        .and_then(|s| s.split_once(' '))
        .filter(|(scheme, token)| scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty())
        .map(|(_, token)| token.trim().to_owned());
    if authorization.is_some() && secret.is_none() {
        return error(
            StatusCode::UNAUTHORIZED,
            "Authorization must contain a Bearer credential",
        );
    }
    // Hyper decodes chunk framing, but leaves other transfer codings intact.
    // Removing their header would silently change the meaning of the body.
    if !supported_transfer_encoding(&headers) {
        return error(
            StatusCode::NOT_IMPLEMENTED,
            "Request transfer encoding is not supported",
        );
    }
    let guard = state.collector.reserve_capture().map(|permit| {
        let request_length = content_length(&headers);
        CaptureGuard {
            shared: Arc::new(Mutex::new(Capturing {
                capture: Capture {
                    id: model::CaptureId::new_live(),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                    source: metadata(&headers, "x-observer-source")
                        .unwrap_or_else(|| "default".into()),
                    task_version: metadata(&headers, "x-observer-task-version"),
                    adapter: metadata(&headers, "x-observer-adapter").filter(|a| a == "jgrep-v1"),
                    status: 502,
                    duration_ms: 0.0,
                    request: Vec::new(),
                    response: Vec::new(),
                    capture_complete: !encoded(&headers),
                    transport_error: None,
                    secret,
                    local_token,
                    access_token: state
                        .access
                        .as_ref()
                        .map(|access| access.token().to_owned()),
                },
                started: Instant::now(),
                limit: state.config.capture_limit,
                request_bytes: 0,
                response_bytes: 0,
                request_length,
                response_length: None,
                request_done: request_length == Some(0),
                response_done: false,
                finalized: false,
            })),
            collector: state.collector.clone(),
            permit: Some(permit),
        }
    });
    if guard.is_none() {
        state.collector.record_skipped();
    }
    let capture = guard.as_ref().map(|g| g.shared.clone());
    let request_capture = capture.clone();
    strip_hop_headers(&mut headers);
    strip_local_request_headers(&mut headers);
    // Loopback cookies belong to local applications, not to the provider.
    headers.remove(header::COOKIE);
    headers.remove(header::HOST);
    headers.remove(header::AUTHORIZATION);
    if let Some(authorization) = authorization {
        headers.insert(header::AUTHORIZATION, authorization);
    }
    let mut body_stream = body.into_data_stream();
    let request_stream = async_stream::stream! {
        while let Some(chunk) = body_stream.next().await {
            match &chunk {
                Ok(bytes) => with_capture(&request_capture, |c| c.append(bytes, false)),
                Err(_) => with_capture(&request_capture, |c| c.capture.transport_error = Some("Request body transfer failed".into())),
            }
            yield chunk;
        }
        with_capture(&request_capture, |c| c.request_done = true);
    };
    let upstream_deadline =
        tokio::time::Instant::now() + Duration::from_secs(state.config.upstream_timeout_secs);
    state.collector.record_forwarded();
    let outgoing = state
        .client
        .post(&state.config.upstream)
        .headers(headers)
        .body(reqwest::Body::wrap_stream(request_stream));
    let upstream = tokio::select! {
        response = outgoing.send() => response,
        _ = state.shutdown.cancelled() => {
            with_capture(&capture, |c| c.capture.transport_error = Some("Observer shut down during transfer".into()));
            drop(guard);
            return error(StatusCode::SERVICE_UNAVAILABLE, "Observer is shutting down");
        }
    };
    let response = match upstream {
        Ok(response) => response,
        Err(failure) => {
            let timeout = failure.is_timeout();
            with_capture(&capture, |c| {
                c.capture.status = if timeout { 504 } else { 502 };
                c.capture.transport_error = Some(
                    if timeout {
                        "Upstream request timed out"
                    } else {
                        "Upstream connection or request transfer failed"
                    }
                    .into(),
                );
            });
            drop(guard);
            return error(
                if timeout {
                    StatusCode::GATEWAY_TIMEOUT
                } else {
                    StatusCode::BAD_GATEWAY
                },
                if timeout {
                    "Upstream request timed out"
                } else {
                    "Upstream connection or request transfer failed"
                },
            );
        }
    };
    let status = response.status();
    let mut response_headers = response.headers().clone();
    let response_length = content_length(&response_headers);
    let response_complete = response_length == Some(0)
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED;
    with_capture(&capture, |c| {
        c.capture.status = status.as_u16();
        c.response_length = response_length;
        c.response_done = response_complete;
        c.capture.capture_complete &= !encoded(&response_headers);
    });
    if !supported_transfer_encoding(&response_headers) {
        with_capture(&capture, |c| {
            c.capture.transport_error = Some("Unsupported upstream transfer encoding".into());
        });
        drop(guard);
        return error(
            StatusCode::BAD_GATEWAY,
            "Unsupported upstream transfer encoding",
        );
    }
    strip_hop_headers(&mut response_headers);
    // A provider must not set cookies for the local dashboard origin.
    response_headers.remove(header::SET_COOKIE);
    let stream = ForwardResponseBody::new(
        response.bytes_stream(),
        guard,
        capture,
        response_length,
        response_complete,
        upstream_deadline,
        state.shutdown.clone(),
    );
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    *response.headers_mut() = response_headers;
    response
}
