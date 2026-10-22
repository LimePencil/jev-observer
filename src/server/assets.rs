use axum::{
    body::Body,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use include_dir::{Dir, include_dir};

use super::error;

static UI: Dir<'_> = include_dir!("$OUT_DIR/ui");

pub(super) async fn ui(uri: Uri) -> Response {
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
