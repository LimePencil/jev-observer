use std::{
    future::{Future, IntoFuture},
    net::{Ipv4Addr, SocketAddr},
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use bytes::Bytes;
use futures_util::StreamExt;
use include_dir::{Dir, include_dir};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::{
    access::Access,
    collector::Collector,
    config::{Config, UpstreamAuth},
    credentials::{Credentials, LOCAL_TOKEN_PREFIX, validate_key},
    model::{self, Capture},
    store::{Filter, Store},
};

static UI: Dir<'_> = include_dir!("$OUT_DIR/ui");
const MAX_IMPORT_BYTES: usize = 8 * 1024 * 1024;
// The API wraps file contents in a JSON string. An escaped byte can occupy six
// transport bytes (for example, `a` sent as `\u0061`); enforce the file limit
// after decoding and keep the upload itself bounded as well.
const MAX_IMPORT_BODY_BYTES: usize = 6 * MAX_IMPORT_BYTES + 1024;
const HTTP_SHUTDOWN_GRACE: Duration = Duration::from_secs(10);
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Store,
    pub collector: Collector,
    pub client: reqwest::Client,
    pub shutdown: CancellationToken,
    pub credentials: Credentials,
    pub access: Option<Access>,
    database_slots: Arc<Semaphore>,
    import_slots: Arc<Semaphore>,
    export_slots: Arc<Semaphore>,
}

impl AppState {
    pub fn new(config: Config, store: Store, collector: Collector) -> Result<Self> {
        #[cfg(not(test))]
        let credentials = Credentials::open(&config.database_path(), store.clone())?;
        #[cfg(test)]
        let credentials = Credentials::empty();
        #[cfg(not(test))]
        let (access, access_path) = Access::open(&config.database_path())?;
        #[cfg(not(test))]
        let access = Some(access);
        #[cfg(test)]
        let access: Option<Access> = None;
        #[cfg(not(test))]
        eprintln!("Dashboard access token: {}", access_path.display());
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(120))
            .timeout(Duration::from_secs(300))
            .tcp_nodelay(true)
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(64)
            .build()
            .context("Create upstream HTTP client")?;
        Ok(Self {
            config: Arc::new(config),
            store,
            collector,
            client,
            shutdown: CancellationToken::new(),
            credentials,
            access,
            database_slots: Arc::new(Semaphore::new(8)),
            import_slots: Arc::new(Semaphore::new(1)),
            export_slots: Arc::new(Semaphore::new(1)),
        })
    }
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/dashboard", get(dashboard))
        .route("/requests/{id}", get(request_detail))
        .route("/groups/{id}", get(group_detail))
        .route(
            "/requests/{id}/label",
            post(label).layer(DefaultBodyLimit::max(8192)),
        )
        .route("/import", post(import))
        .route("/export", get(export))
        .route("/data", delete(delete_data))
        .route("/settings", get(settings))
        .route(
            "/credentials",
            get(credentials_status)
                .put(set_credentials)
                .delete(clear_credentials)
                .layer(DefaultBodyLimit::max(8192)),
        )
        .route("/health", get(health))
        .layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES));
    Router::new()
        .route("/v1/systemone", post(proxy))
        .nest("/api", api)
        .fallback(get(ui))
        .layer(middleware::from_fn_with_state(state.clone(), local_guard))
        .with_state(state)
}

pub async fn run(config: Config, store: Store, collector: Collector) -> Result<()> {
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, config.port)))
            .await
            .context("Bind local HTTP listener")?;
    let address = listener.local_addr()?;
    let mut config = config;
    config.port = address.port();
    let state = AppState::new(config, store, collector.clone())?;
    let shutdown = state.shutdown.clone();
    eprintln!(
        "Observer: http://{address}{}",
        if state.config.demo {
            " (synthetic demo; forwarding disabled)"
        } else {
            ""
        }
    );
    let server = axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown.clone().cancelled_owned())
        .into_future();
    tokio::pin!(server);
    let finished = tokio::select! {
        result = server.as_mut() => Some(result),
        _ = shutdown_signal() => None,
    };
    shutdown.cancel();
    arm_shutdown_watchdog();
    if let Some(result) = finished {
        collector.shutdown().await;
        return result.context("Run local HTTP server");
    }
    match drain_after_http_grace(server.as_mut(), collector.shutdown(), HTTP_SHUTDOWN_GRACE).await {
        Some(result) => result.context("Run local HTTP server"),
        None => anyhow::bail!(
            "HTTP shutdown grace expired; queued records were drained, but in-flight captures may be lost"
        ),
    }
}

/// This thread remains armed through runtime teardown. A clean process exit
/// ends it; an executor or blocking database task cannot disable the deadline.
fn arm_shutdown_watchdog() {
    if let Err(failure) = spawn_deadline_watchdog(SHUTDOWN_DEADLINE, || {
        eprintln!(
            "Observer shutdown exceeded 15 seconds; forcing exit. Pending captures may be lost."
        );
        std::process::exit(2);
    }) {
        eprintln!(
            "Cannot start the shutdown watchdog ({failure}); forcing exit. Pending captures may be lost."
        );
        std::process::exit(2);
    }
}

fn spawn_deadline_watchdog(
    deadline: Duration,
    on_deadline: impl FnOnce() + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("observer-shutdown".into())
        .spawn(move || {
            std::thread::sleep(deadline);
            on_deadline();
        })
}

async fn drain_after_http_grace<S, D>(server: S, drain: D, grace: Duration) -> Option<S::Output>
where
    S: Future,
    D: Future<Output = ()>,
{
    let finished = tokio::time::timeout(grace, server).await.ok();
    if finished.is_none() {
        eprintln!(
            "HTTP shutdown grace ({:.1}s) expired; draining queued records. In-flight captures may be lost.",
            grace.as_secs_f64()
        );
    }
    drain.await;
    finished
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = terminate.recv() => {},
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(windows)]
    {
        match tokio::signal::windows::ctrl_break() {
            Ok(mut signal) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = signal.recv() => {},
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn error(status: StatusCode, message: &'static str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

fn allowed_host(host: &str, port: u16) -> bool {
    let Ok(authority) = axum::http::uri::Authority::from_str(host) else {
        return false;
    };
    let name = authority.host();
    matches!(name, "127.0.0.1" | "localhost" | "[::1]")
        && authority.port_u16().unwrap_or(80) == port
        && !host.contains('@')
}

async fn local_guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    if headers.get_all(header::HOST).iter().count() != 1
        || !host.is_some_and(|h| allowed_host(h, state.config.port))
    {
        return error(StatusCode::FORBIDDEN, "Use the local Observer address");
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let expected = reqwest::Url::parse(&format!("http://{}", host.unwrap_or_default())).ok();
        let valid = origin
            .to_str()
            .ok()
            .and_then(|o| reqwest::Url::parse(o).ok())
            .is_some_and(|o| {
                o.scheme() == "http"
                    && o.username().is_empty()
                    && o.password().is_none()
                    && o.query().is_none()
                    && o.fragment().is_none()
                    && o.path() == "/"
                    && expected
                        .as_ref()
                        .is_some_and(|expected| o.origin() == expected.origin())
            });
        if !valid {
            return error(
                StatusCode::FORBIDDEN,
                "Cross-origin requests are not allowed",
            );
        }
    }
    let local_api = request.uri().path().starts_with("/api/");
    let is_proxy = request.uri().path() == "/v1/systemone";
    // Cross-site image/navigation requests can omit Origin while still
    // reaching local GET endpoints. Browsers mark these with Fetch Metadata.
    if (local_api || is_proxy)
        && headers
            .get("sec-fetch-site")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|site| !matches!(site, "same-origin" | "none"))
    {
        return error(
            StatusCode::FORBIDDEN,
            "Cross-site Observer requests are not allowed",
        );
    }
    if local_api
        && !matches!(
            *request.method(),
            Method::GET | Method::HEAD | Method::OPTIONS
        )
        && headers
            .get("x-observer-request")
            .and_then(|v| v.to_str().ok())
            != Some("1")
    {
        return error(
            StatusCode::FORBIDDEN,
            "Local mutations require X-Observer-Request: 1",
        );
    }
    if !is_proxy
        && state.access.as_ref().is_some_and(|access| {
            !access.allows_basic(
                headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok()),
            )
        })
    {
        let mut response = error(StatusCode::UNAUTHORIZED, "Dashboard access token required");
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"Jev Observer\""),
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return response;
    }
    let mut response = next.run(request).await;
    if !is_proxy {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response.headers_mut().insert(
            "x-content-type-options",
            HeaderValue::from_static("nosniff"),
        );
        response.headers_mut().insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"
        ));
        response.headers_mut().insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        );
    }
    response
}

fn metadata(headers: &HeaderMap, key: &'static str) -> Option<String> {
    headers
        .get(key)
        .and_then(|h| h.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
        .map(ToOwned::to_owned)
}

fn strip_hop_headers(headers: &mut HeaderMap) {
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

fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse().ok())
}

fn encoded(headers: &HeaderMap) -> bool {
    headers.get_all(header::CONTENT_ENCODING).iter().any(|v| {
        v.to_str().map_or(true, |s| {
            s.split(',')
                .any(|p| !p.trim().eq_ignore_ascii_case("identity"))
        })
    })
}

struct Capturing {
    capture: Capture,
    started: Instant,
    limit: usize,
    request_bytes: u64,
    response_bytes: u64,
    request_length: Option<u64>,
    response_length: Option<u64>,
    request_done: bool,
    response_done: bool,
    finalized: bool,
}

impl Capturing {
    fn append(&mut self, data: &[u8], response: bool) {
        let body = if response {
            &mut self.capture.response
        } else {
            &mut self.capture.request
        };
        let available = self.limit.saturating_sub(body.len());
        let append = data.len().min(available);
        if body.len() + append > body.capacity() {
            body.reserve_exact(append);
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

type SharedCapture = Arc<Mutex<Capturing>>;

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
struct CaptureGuard {
    shared: SharedCapture,
    collector: Collector,
    permit: Option<OwnedSemaphorePermit>,
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

async fn proxy(State(state): State<AppState>, request: Request) -> Response {
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
    let guard = state.collector.reserve_capture().map(|permit| {
        let request_length = content_length(&headers);
        CaptureGuard {
            shared: Arc::new(Mutex::new(Capturing {
                capture: Capture {
                    id: uuid::Uuid::now_v7().to_string(),
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
    with_capture(&capture, |c| {
        c.capture.status = status.as_u16();
        c.response_length = content_length(&response_headers);
        c.response_done = c.response_length == Some(0)
            || status == StatusCode::NO_CONTENT
            || status == StatusCode::NOT_MODIFIED;
        c.capture.capture_complete &= !encoded(&response_headers);
    });
    strip_hop_headers(&mut response_headers);
    // A provider must not set cookies for the local dashboard origin.
    response_headers.remove(header::SET_COOKIE);
    let mut upstream_stream = response.bytes_stream();
    let shutdown = state.shutdown.clone();
    let stream = async_stream::stream! {
        let _guard = guard;
        loop {
            let chunk = tokio::select! {
                chunk = upstream_stream.next() => chunk,
                _ = shutdown.cancelled() => {
                    with_capture(&capture, |c| c.capture.transport_error = Some("Observer shut down during response transfer".into()));
                    yield Err::<Bytes, std::io::Error>(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "Observer is shutting down"));
                    break;
                }
            };
            match chunk {
                Some(Ok(bytes)) => {
                    with_capture(&capture, |c| c.append(&bytes, true));
                    yield Ok::<Bytes, std::io::Error>(bytes);
                }
                Some(Err(_)) => {
                    with_capture(&capture, |c| c.capture.transport_error = Some("Upstream response transfer failed".into()));
                    yield Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Upstream response transfer failed"));
                    break;
                }
                None => {
                    with_capture(&capture, |c| c.response_done = true);
                    break;
                }
            }
        }
    };
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    *response.headers_mut() = response_headers;
    response
}

struct LocalError(StatusCode, &'static str);

impl IntoResponse for LocalError {
    fn into_response(self) -> Response {
        error(self.0, self.1)
    }
}

async fn database<T: Send + 'static>(
    slots: Arc<Semaphore>,
    operation: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> std::result::Result<T, LocalError> {
    let Ok(permit) = slots.try_acquire_owned() else {
        return Err(LocalError(
            StatusCode::TOO_MANY_REQUESTS,
            "Local queries are busy; retry shortly",
        ));
    };
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        operation()
    })
    .await
    {
        Ok(Ok(value)) => Ok(value),
        _ => Err(LocalError(
            StatusCode::SERVICE_UNAVAILABLE,
            "The local database operation failed",
        )),
    }
}

fn invalid_filter(filter: &Filter) -> Option<&'static str> {
    if filter
        .window
        .as_deref()
        .is_some_and(|value| !matches!(value, "1h" | "24h" | "7d" | "all"))
    {
        return Some("Window must be 1h, 24h, 7d, or all");
    }
    if filter
        .status
        .as_deref()
        .is_some_and(|value| !matches!(value, "" | "all" | "error"))
    {
        return Some("Status must be all or error");
    }
    if [
        &filter.source,
        &filter.model,
        &filter.group,
        &filter.search,
        &filter.group_search,
        &filter.group_cursor,
        &filter.request_cursor,
    ]
    .into_iter()
    .flatten()
    .any(|value| value.len() > 4096)
    {
        return Some("Filter values must not exceed 4096 bytes");
    }
    if filter
        .from
        .zip(filter.to)
        .is_some_and(|(from, to)| from > to)
    {
        return Some("The start of a date range must not exceed its end");
    }
    if [filter.from, filter.to]
        .into_iter()
        .flatten()
        .any(|time| !(-8_640_000_000_000_000..=8_640_000_000_000_000).contains(&time))
    {
        return Some("Date bounds must be valid epoch milliseconds");
    }
    if filter.request_cursor.as_deref().is_some_and(|cursor| {
        !cursor.split_once(':').is_some_and(|(timestamp, seq)| {
            timestamp.parse::<i64>().is_ok() && seq.parse::<i64>().is_ok_and(|seq| seq > 0)
        })
    }) {
        return Some("Invalid request cursor");
    }
    if filter.group_cursor.as_deref().is_some_and(|cursor| {
        !cursor
            .split_once(':')
            .is_some_and(|(timestamp, id)| timestamp.parse::<i64>().is_ok() && !id.is_empty())
    }) {
        return Some("Invalid group cursor");
    }
    None
}

async fn dashboard(State(state): State<AppState>, Query(filter): Query<Filter>) -> Response {
    if let Some(message) = invalid_filter(&filter) {
        return error(StatusCode::BAD_REQUEST, message);
    }
    let store = state.store.clone();
    match database(state.database_slots.clone(), move || {
        store.dashboard(&filter)
    })
    .await
    {
        Ok(mut value) => {
            value["health"] = state.collector.health();
            Json(value).into_response()
        }
        Err(response) => response.into_response(),
    }
}

async fn request_detail(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match database(state.database_slots.clone(), move || {
        state.store.request(&id)
    })
    .await
    {
        Ok(Some(value)) => Json(value).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "Request not found"),
        Err(response) => response.into_response(),
    }
}

async fn group_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(filter): Query<Filter>,
) -> Response {
    if let Some(message) = invalid_filter(&filter) {
        return error(StatusCode::BAD_REQUEST, message);
    }
    match database(state.database_slots.clone(), move || {
        state.store.group(&id, &filter)
    })
    .await
    {
        Ok(Some(value)) => Json(value).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "Question group not found"),
        Err(response) => response.into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportBody {
    text: String,
    format: String,
}

async fn import(State(state): State<AppState>, request: Request) -> Response {
    let Ok(permit) = state.import_slots.clone().try_acquire_owned() else {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "An import is already running; retry shortly",
        );
    };
    if !request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
        })
    {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Import expects application/json",
        );
    }
    let bytes = match tokio::time::timeout(
        Duration::from_secs(30),
        axum::body::to_bytes(request.into_body(), MAX_IMPORT_BODY_BYTES),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Encoded import body is too large; import text must not exceed 8 MiB",
            );
        }
        Err(_) => return error(StatusCode::REQUEST_TIMEOUT, "Import upload timed out"),
    };
    let options = state.config.normalize_options();
    // Hold the permit inside the blocking task, even if its HTTP caller cancels.
    let result =
        tokio::task::spawn_blocking(move || -> std::result::Result<(usize, usize), StatusCode> {
            let _permit = permit;
            let body: ImportBody =
                serde_json::from_slice(&bytes).map_err(|_| StatusCode::BAD_REQUEST)?;
            drop(bytes);
            if body.text.len() > MAX_IMPORT_BYTES {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            if !matches!(
                body.format.as_str(),
                "observer-jsonl" | "jevrouter-receipt" | "systemone-capture"
            ) {
                return Err(StatusCode::BAD_REQUEST);
            }
            let records = model::import_records(&body.text, &body.format, &options)
                .map_err(|_| StatusCode::BAD_REQUEST)?;
            if records.len() > 10_000 {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            let total = records.len();
            let mut connection = state
                .store
                .writer_connection()
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            let imported = state
                .store
                .write_batch(&mut connection, &records)
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            Ok((imported, total.saturating_sub(imported)))
        })
        .await;
    match result {
        Ok(Ok((imported, duplicates))) => {
            Json(json!({"imported": imported, "duplicates": duplicates})).into_response()
        }
        Ok(Err(StatusCode::BAD_REQUEST)) => error(
            StatusCode::BAD_REQUEST,
            "Import could not be parsed; check the format and record structure",
        ),
        Ok(Err(StatusCode::PAYLOAD_TOO_LARGE)) => error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Import text must not exceed 8 MiB or 10,000 records",
        ),
        _ => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "The local import could not be saved",
        ),
    }
}

#[derive(Deserialize)]
struct ExportQuery {
    #[serde(default = "jsonl_format")]
    format: String,
    #[serde(flatten)]
    filter: Filter,
}

fn jsonl_format() -> String {
    "jsonl".into()
}

async fn export(State(state): State<AppState>, Query(mut query): Query<ExportQuery>) -> Response {
    if !matches!(query.format.as_str(), "jsonl" | "csv") {
        return error(
            StatusCode::BAD_REQUEST,
            "Export format must be jsonl or csv",
        );
    }
    if let Some(message) = invalid_filter(&query.filter) {
        return error(StatusCode::BAD_REQUEST, message);
    }
    query.filter.as_of = Some(chrono::Utc::now().timestamp_millis());
    let Ok(permit) = state.export_slots.clone().try_acquire_owned() else {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "An export is already running; retry shortly",
        );
    };
    // An in-progress page holds a clone even after the downloading client leaves.
    let lease = Arc::new(permit);
    let csv = query.format == "csv";
    let (first, mut after, through, mut has_more) =
        match export_page(&state, &query, 0, None, lease.clone()).await {
            Ok(page) => page,
            Err(failure) => return failure.into_response(),
        };
    let stream = async_stream::stream! {
        let _lease = lease;
        yield Ok::<Bytes, std::io::Error>(Bytes::from(first));
        while has_more {
            let page = tokio::select! {
                page = export_page(&state, &query, after, Some(through), _lease.clone()) => page,
                _ = state.shutdown.cancelled() => {
                    yield Err(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "Export interrupted by shutdown"));
                    break;
                }
            };
            let (text, next, _, more) = match page {
                Ok(page) => page,
                Err(_) => {
                    yield Err(std::io::Error::other("Export interrupted by a local database failure"));
                    break;
                }
            };
            if more && next <= after {
                yield Err(std::io::Error::other("Export cursor did not advance"));
                break;
            }
            after = next;
            has_more = more;
            yield Ok(Bytes::from(text));
        }
    };
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(if csv {
            "text/csv; charset=utf-8"
        } else {
            "application/x-ndjson; charset=utf-8"
        }),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static(if csv {
            "attachment; filename=observer.csv"
        } else {
            "attachment; filename=observer.jsonl"
        }),
    );
    response
}

async fn export_page(
    state: &AppState,
    query: &ExportQuery,
    after: i64,
    through: Option<i64>,
    lease: Arc<OwnedSemaphorePermit>,
) -> std::result::Result<(String, i64, i64, bool), LocalError> {
    let store = state.store.clone();
    let filter = query.filter.clone();
    let format = query.format.clone();
    database(state.database_slots.clone(), move || {
        let _lease = lease;
        store.export_page(&filter, &format, after, through)
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LabelBody {
    key: String,
    label: String,
}

async fn label(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<LabelBody>,
) -> Response {
    if !matches!(body.label.as_str(), "correct" | "incorrect" | "unknown") || body.key.len() > 4096
    {
        return error(
            StatusCode::BAD_REQUEST,
            "Choose correct, incorrect, or unknown for an answer key",
        );
    }
    match database(state.database_slots.clone(), move || {
        let Some(record) = state.store.request(&id)? else {
            return Ok(None);
        };
        if !record["answers"].as_array().is_some_and(|answers| {
            answers
                .iter()
                .any(|answer| answer["key"].as_str() == Some(body.key.as_str()))
        }) {
            return Ok(Some(false));
        }
        state.store.add_label(&id, &body.key, &body.label)?;
        Ok(Some(true))
    })
    .await
    {
        Ok(Some(true)) => Json(json!({"ok": true})).into_response(),
        Ok(Some(false)) => error(StatusCode::NOT_FOUND, "Answer key not found"),
        Ok(None) => error(StatusCode::NOT_FOUND, "Request not found"),
        Err(response) => response.into_response(),
    }
}

async fn delete_data(State(state): State<AppState>) -> Response {
    match database(state.database_slots.clone(), move || {
        state.store.delete_all()
    })
    .await
    {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(response) => response.into_response(),
    }
}

async fn settings(State(state): State<AppState>) -> Json<Value> {
    Json(state.config.public_settings())
}

async fn credentials_status(State(state): State<AppState>) -> Json<Value> {
    Json(json!(state.credentials.status()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetCredentialsBody {
    api_key: String,
    persist: bool,
}

async fn set_credentials(
    State(state): State<AppState>,
    Json(body): Json<SetCredentialsBody>,
) -> Response {
    if state.config.demo {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Provider keys cannot be set in demo mode",
        );
    }
    if state.config.upstream_auth == UpstreamAuth::None {
        return error(
            StatusCode::CONFLICT,
            "The local upstream is configured without provider credentials",
        );
    }
    if validate_key(&body.api_key).is_err() {
        return error(
            StatusCode::BAD_REQUEST,
            "Provider key must be 1–4096 printable ASCII characters without spaces",
        );
    }
    match state.credentials.set(body.api_key, body.persist).await {
        Ok(client_token) => Json(json!({"client_token": client_token, "storage": if body.persist { "system" } else { "session" }})).into_response(),
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "Provider connection could not be saved; check the credential store and workspace database"),
    }
}

async fn clear_credentials(State(state): State<AppState>) -> Response {
    if state.config.demo {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Provider keys cannot be changed in demo mode",
        );
    }
    match state.credentials.clear().await {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Saved provider key could not be removed from the system credential store",
        ),
    }
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(state.collector.health())
}

async fn ui(uri: Uri) -> Response {
    if uri.path().starts_with("/api/") || uri.path().starts_with("/v1/") {
        return error(StatusCode::NOT_FOUND, "Endpoint not found");
    }
    let path = uri.path().trim_start_matches('/');
    let file = if path.is_empty() {
        UI.get_file("index.html")
    } else {
        UI.get_file(path).or_else(|| {
            if !path.contains('.') {
                UI.get_file("index.html")
            } else {
                None
            }
        })
    };
    match file {
        Some(file) => {
            let mime = mime_guess::from_path(file.path()).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.to_string())],
                Body::from(file.contents()),
            )
                .into_response()
        }
        None => error(StatusCode::NOT_FOUND, "Asset not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use clap::Parser;
    use tower::ServiceExt;

    struct Fixture {
        _directory: tempfile::TempDir,
        state: AppState,
    }

    fn fixture(upstream: &str, limit: usize, demo: bool) -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let mut config = Config::try_parse_from(["observer"]).unwrap();
        config.upstream = upstream.into();
        config.capture_limit = limit;
        config.demo = demo;
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
                serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap())
                    .unwrap();
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
            let body =
                json!({"format":"observer-jsonl", "text":format!("{text}{extra}")}).to_string();
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
        assert_eq!(saved(&fixture).await["capture_complete"], false);
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
                if health["persisted"].as_u64().unwrap()
                    == before["persisted"].as_u64().unwrap() + 1
                {
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
    async fn export_streams_all_pages_with_one_header_and_excludes_later_arrivals() {
        let fixture = fixture("http://127.0.0.1:1/v1/systemone", 1024, true);
        let base = model::sample_records().into_iter().next().unwrap();
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
                .uri(format!("/api/export?window=all&format={format}"))
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
}
