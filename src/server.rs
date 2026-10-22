//! HTTP composition; transport, access guards and local data handlers stay separate.

mod assets;
mod guard;
mod lifecycle;
mod local_api;
mod proxy;
mod state;

use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde_json::json;

use assets::ui;
use guard::local_guard;
pub use lifecycle::run;
use local_api::{
    MAX_IMPORT_BYTES, clear_credentials, credentials_status, dashboard, delete_data, export,
    group_detail, health, import, label, request_detail, set_credentials, settings,
};
use proxy::proxy;
pub use state::AppState;

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
        .route(state.config.protocol.local_path(), post(proxy))
        .nest("/api", api)
        .fallback(get(ui))
        .layer(middleware::from_fn_with_state(state.clone(), local_guard))
        .with_state(state)
}

fn error(status: StatusCode, message: &'static str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
