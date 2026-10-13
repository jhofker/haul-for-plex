//! Request checks applied to every route: optional password and CSRF protection.

use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::state::AppState;

pub async fn guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if let Some(password) = &state.settings().password
        && !password_ok(req.headers(), password)
    {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"Haul for Plex\"")],
            "Password required",
        )
            .into_response();
    }
    if !is_safe_method(req.method()) && !same_origin(req.headers()) {
        return (StatusCode::FORBIDDEN, "Cross-site requests are not allowed").into_response();
    }
    next.run(req).await
}

fn is_safe_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Browsers label cross-site requests with `Sec-Fetch-Site`, and send `Origin`
/// on form posts; a mismatch means another site is driving the request.
fn same_origin(headers: &HeaderMap) -> bool {
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        return matches!(site, "same-origin" | "none");
    }
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        // Not a browser form post (e.g. curl); nothing to forge.
        return true;
    };
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let origin_host = origin.split_once("://").map(|(_, rest)| rest);
    matches!((origin_host, host), (Some(o), Some(h)) if o == h)
}

/// Accepts any username with the right password.
fn password_ok(headers: &HeaderMap, password: &str) -> bool {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
    else {
        return false;
    };
    let Ok(decoded) = STANDARD.decode(value.trim()) else {
        return false;
    };
    let Ok(decoded) = String::from_utf8(decoded) else {
        return false;
    };
    let given = decoded.split_once(':').map(|(_, p)| p).unwrap_or("");
    constant_time_eq(given.as_bytes(), password.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}
