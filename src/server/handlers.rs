//! Route handlers. Each reads or updates the session through AppState.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};

use super::{pages, url, AppError, AppState};
use crate::session::SessionError;

pub async fn index(State(st): State<AppState>) -> Redirect {
    let s = st.snapshot();
    match s.unruled().first() {
        Some(i) => Redirect::to(&url(&s.token, &format!("f/{i}"))),
        None => Redirect::to(&url(&s.token, "submit")),
    }
}

pub async fn finding_page(
    State(st): State<AppState>,
    Path((_, idx)): Path<(String, usize)>,
) -> Result<Html<String>, AppError> {
    let s = st.snapshot();
    if idx >= s.entries.len() {
        return Err(SessionError::NoSuchFinding(idx).into());
    }
    Ok(Html(pages::finding(&s, idx, None).into_string()))
}

pub async fn asset(Path((_, name)): Path<(String, String)>) -> Response {
    let (body, mime): (&'static str, &'static str) = match name.as_str() {
        "app.css" => (include_str!("../../assets/app.css"), "text/css; charset=utf-8"),
        "app.js" => (include_str!("../../assets/app.js"), "text/javascript; charset=utf-8"),
        "mermaid.min.js" => (include_str!("../../assets/vendor/mermaid.min.js"), "text/javascript; charset=utf-8"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    ([(header::CONTENT_TYPE, mime)], body).into_response()
}
