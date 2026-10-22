use std::str::FromStr;

use axum::{
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::Response,
};

use super::{AppState, error};

fn allowed_host(host: &str, port: u16) -> bool {
    let Ok(authority) = axum::http::uri::Authority::from_str(host) else {
        return false;
    };
    let name = authority.host();
    (matches!(name, "127.0.0.1" | "[::1]") || name.eq_ignore_ascii_case("localhost"))
        && authority.port_u16().unwrap_or(80) == port
        && !host.contains('@')
}

pub(super) async fn local_guard(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
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
