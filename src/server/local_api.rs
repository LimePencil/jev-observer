use std::{sync::Arc, time::Duration};

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::{AppState, error};
use crate::{
    config::UpstreamAuth,
    credentials::validate_key,
    model,
    store::{ExportSession, Filter, LabelOutcome},
};

pub(super) const MAX_IMPORT_BYTES: usize = 8 * 1024 * 1024;
// The API wraps file contents in a JSON string. An escaped byte can occupy six
// transport bytes (for example, `a` sent as `\u0061`); enforce the file limit
// after decoding and keep the upload itself bounded as well.
pub(super) const MAX_IMPORT_BODY_BYTES: usize = 6 * MAX_IMPORT_BYTES + 1024;

pub(super) struct LocalError(StatusCode, &'static str);

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

pub(super) fn invalid_filter(filter: &Filter) -> Option<&'static str> {
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
        .any(|time| !model::valid_timestamp(time))
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

pub(super) async fn dashboard(
    State(state): State<AppState>,
    Query(filter): Query<Filter>,
) -> Response {
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

pub(super) async fn request_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
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

pub(super) async fn group_detail(
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

fn import_failure(status: StatusCode) -> LocalError {
    LocalError(
        status,
        match status {
            StatusCode::BAD_REQUEST => {
                "Import could not be parsed; check the format and record structure"
            }
            StatusCode::PAYLOAD_TOO_LARGE => {
                "Import exceeds a size or normalization limit. Use smaller records or batches."
            }
            _ => "The local import could not be saved",
        },
    )
}

pub(super) async fn import(State(state): State<AppState>, request: Request) -> Response {
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
        tokio::task::spawn_blocking(move || -> std::result::Result<(usize, usize), LocalError> {
            let _permit = permit;
            let body: ImportBody = serde_json::from_slice(&bytes)
                .map_err(|_| import_failure(StatusCode::BAD_REQUEST))?;
            drop(bytes);
            if body.text.len() > MAX_IMPORT_BYTES {
                return Err(import_failure(StatusCode::PAYLOAD_TOO_LARGE));
            }
            if !matches!(
                body.format.as_str(),
                "observer-jsonl" | "jevrouter-receipt" | "systemone-capture"
            ) {
                return Err(import_failure(StatusCode::BAD_REQUEST));
            }
            let records =
                model::import_records(&body.text, &body.format, &options).map_err(|error| {
                    if error.is::<model::ImportLimitError>() {
                        import_failure(StatusCode::PAYLOAD_TOO_LARGE)
                    } else if error.is::<model::RedactionCollisionError>() {
                        LocalError(
                            StatusCode::BAD_REQUEST,
                            model::IMPORT_REDACTION_COLLISION_MESSAGE,
                        )
                    } else if error.is::<model::ImportIdentityError>() {
                        LocalError(
                            StatusCode::BAD_REQUEST,
                            model::IMPORT_IDENTITY_ERROR_MESSAGE,
                        )
                    } else if error.is::<model::RedactionValidationError>() {
                        LocalError(
                            StatusCode::BAD_REQUEST,
                            model::IMPORT_REDACTION_VALIDATION_MESSAGE,
                        )
                    } else {
                        import_failure(StatusCode::BAD_REQUEST)
                    }
                })?;
            if records.len() > 10_000 {
                return Err(import_failure(StatusCode::PAYLOAD_TOO_LARGE));
            }
            let total = records.len();
            let mut connection = state
                .store
                .writer_connection()
                .map_err(|_| import_failure(StatusCode::SERVICE_UNAVAILABLE))?;
            let imported = state
                .store
                .write_batch(&mut connection, &records)
                .map_err(|_| import_failure(StatusCode::SERVICE_UNAVAILABLE))?;
            Ok((imported, total.saturating_sub(imported)))
        })
        .await;
    match result {
        Ok(Ok((imported, duplicates))) => {
            Json(json!({"imported": imported, "duplicates": duplicates})).into_response()
        }
        Ok(Err(failure)) => failure.into_response(),
        Err(_) => import_failure(StatusCode::SERVICE_UNAVAILABLE).into_response(),
    }
}

#[derive(Deserialize)]
pub(super) struct ExportQuery {
    #[serde(default = "jsonl_format")]
    format: String,
    #[serde(flatten)]
    filter: Filter,
}

fn jsonl_format() -> String {
    "jsonl".into()
}

pub(super) async fn export(
    State(state): State<AppState>,
    Query(mut query): Query<ExportQuery>,
) -> Response {
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
    let store = state.store.clone();
    let (mut session, (first, mut after, _, mut has_more)) =
        match export_database(&state, lease.clone(), move || {
            let mut session = store.start_export(&query.filter, &query.format)?;
            let page = session.next_page()?;
            Ok((session, page))
        })
        .await
        {
            Ok(page) => page,
            Err(failure) => return failure.into_response(),
        };
    let stream = async_stream::stream! {
        let _lease = lease;
        yield Ok::<Bytes, std::io::Error>(Bytes::from(first));
        while has_more {
            let page = tokio::select! {
                page = export_page(&state, session, _lease.clone()) => page,
                _ = state.shutdown.cancelled() => {
                    yield Err(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "Export interrupted by shutdown"));
                    break;
                }
            };
            let (next_session, (text, next, _, more)) = match page {
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
            session = next_session;
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
    mut session: ExportSession,
    lease: Arc<OwnedSemaphorePermit>,
) -> std::result::Result<(ExportSession, (String, i64, i64, bool)), LocalError> {
    export_database(state, lease, move || {
        let page = session.next_page()?;
        Ok((session, page))
    })
    .await
}

pub(super) async fn export_database<T: Send + 'static>(
    state: &AppState,
    lease: Arc<OwnedSemaphorePermit>,
    operation: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> std::result::Result<T, LocalError> {
    database(state.database_slots.clone(), move || {
        // A blocking page may outlive its canceled HTTP future. Keep the
        // export slot until its selection/body work has actually completed.
        let _lease = lease;
        operation()
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LabelBody {
    key: String,
    label: String,
}

pub(super) async fn label(
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
        state.store.add_label(&id, &body.key, &body.label)
    })
    .await
    {
        Ok(LabelOutcome::Saved) => Json(json!({"ok": true})).into_response(),
        Ok(LabelOutcome::AnswerNotFound) => error(StatusCode::NOT_FOUND, "Answer key not found"),
        Ok(LabelOutcome::RequestNotFound) => error(StatusCode::NOT_FOUND, "Request not found"),
        Err(response) => response.into_response(),
    }
}

pub(super) async fn delete_data(State(state): State<AppState>) -> Response {
    match database(state.database_slots.clone(), move || {
        state.store.delete_all()
    })
    .await
    {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(response) => response.into_response(),
    }
}

pub(super) async fn settings(State(state): State<AppState>) -> Json<Value> {
    Json(state.config.public_settings())
}

pub(super) async fn credentials_status(State(state): State<AppState>) -> Json<Value> {
    Json(json!(state.credentials.status()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SetCredentialsBody {
    api_key: String,
    persist: bool,
}

pub(super) async fn set_credentials(
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

pub(super) async fn clear_credentials(State(state): State<AppState>) -> Response {
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

pub(super) async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(state.collector.health())
}
