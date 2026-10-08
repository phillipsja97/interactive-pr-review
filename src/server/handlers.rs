//! Route handlers. Each reads or updates the session through AppState.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;

use super::{pages, url, AppError, AppState};
use crate::model::Location;
use crate::session::{Ruling, Session, SessionError, Verdict};
use crate::validate::{check_repo_path, resolve_lines};

use crate::compose::{cite_plain, compose, to_markdown, ComposedReview, SubmitChoice};
use crate::session::{Outcome, SessionStatus};

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
    Ok(Html(pages::finding(&s, idx, None, None).into_string()))
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

#[derive(Deserialize)]
pub struct RulingForm {
    #[serde(default)]
    pub verdict: String,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub escalate_to: String,
}

pub async fn save_ruling(
    State(st): State<AppState>,
    Path((_, idx)): Path<(String, usize)>,
    Form(form): Form<RulingForm>,
) -> Result<Response, AppError> {
    let token = st.token();
    let saved = st.update(|s| {
        let verdict = Verdict::from_form(&form.verdict, &form.reason, &form.escalate_to).map_err(SessionError::Invalid)?;
        s.rule(idx, Ruling { verdict, comment: form.comment.trim().to_string() })?;
        Ok(s.next_unruled_after(idx))
    });
    match saved {
        Ok(Some(next)) => Ok(Redirect::to(&url(&token, &format!("f/{next}"))).into_response()),
        Ok(None) => Ok(Redirect::to(&url(&token, "submit")).into_response()),
        Err(AppError::Session(SessionError::Invalid(msg))) => {
            let page = pages::finding(&st.snapshot(), idx, Some(&msg), Some(&form));
            Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(page.into_string())).into_response())
        }
        Err(e) => Err(e),
    }
}

#[derive(Deserialize, Default)]
pub struct NewFindingForm {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub start: String,
    #[serde(default)]
    pub end: String,
    #[serde(default)]
    pub comment: String,
}

pub async fn new_page(State(st): State<AppState>) -> Html<String> {
    Html(pages::new_finding(&st.snapshot(), &NewFindingForm::default(), None).into_string())
}

pub async fn create_reviewer_finding(
    State(st): State<AppState>,
    Form(form): Form<NewFindingForm>,
) -> Result<Response, AppError> {
    let s = st.snapshot();
    let created = locate(&st, &s, &form)
        .map_err(|msg| AppError::Session(SessionError::Invalid(msg)))
        .and_then(|(location, text)| st.update(|s| s.add_reviewer_finding(location, form.comment.clone(), text)));
    match created {
        Ok(idx) => Ok(Redirect::to(&url(&s.token, &format!("f/{idx}"))).into_response()),
        Err(AppError::Session(SessionError::Invalid(msg))) => {
            let page = pages::new_finding(&s, &form, Some(&msg));
            Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(page.into_string())).into_response())
        }
        Err(e) => Err(e),
    }
}

/// The Reviewer's location: PR-wide when no path is given, otherwise checked against the head commit.
fn locate(st: &AppState, s: &Session, form: &NewFindingForm) -> Result<(Location, Option<String>), String> {
    let path = form.path.trim();
    if path.is_empty() {
        return Ok((Location::PrWide, None));
    }
    check_repo_path(path)?;
    let start: u32 = form.start.trim().parse().map_err(|_| "Start line must be a number.".to_string())?;
    let end: u32 = match form.end.trim() {
        "" => start,
        e => e.parse().map_err(|_| "End line must be a number.".to_string())?,
    };
    let text = resolve_lines(st.repo(), &s.head_sha, path, start, end).map_err(|e| e.message)?;
    Ok((Location::Lines { path: path.to_string(), start, end }, Some(text)))
}

pub async fn submit_page(State(st): State<AppState>) -> Html<String> {
    let s = st.snapshot();
    Html(pages::submit(&s, preview(&st, &s).as_ref(), st.is_stale(), None, None).into_string())
}

/// The Review as it would be composed with no summary and no approval.
fn preview(st: &AppState, s: &Session) -> Option<ComposedReview> {
    if !s.unruled().is_empty() {
        return None;
    }
    let diff = st.repo().diff_map(&s.base_sha, &s.head_sha).ok()?;
    compose(&s.entries, &diff, &SubmitChoice::default(), &cite_plain).ok()
}

#[derive(Deserialize)]
pub struct SubmitForm {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub approve: Option<String>,
    #[serde(default)]
    pub ack_stale: Option<String>,
}

pub async fn submit(State(st): State<AppState>, Form(form): Form<SubmitForm>) -> Result<Response, AppError> {
    let s = st.snapshot();
    if !s.is_open() {
        return Err(SessionError::NotOpen.into());
    }
    let stale = st.is_stale();
    let reject = |msg: &str| -> Response {
        let s = st.snapshot();
        let page = pages::submit(&s, preview(&st, &s).as_ref(), stale, Some(msg), Some(&form));
        (StatusCode::UNPROCESSABLE_ENTITY, Html(page.into_string())).into_response()
    };
    if stale && form.ack_stale.is_none() {
        return Ok(reject("The change has new commits since these Findings were proposed. Tick the acknowledgement to submit anyway."));
    }
    let choice = SubmitChoice { approve: form.approve.is_some(), summary: form.summary.trim().to_string() };
    let diff = st.repo().diff_map(&s.base_sha, &s.head_sha).map_err(AppError::Internal)?;
    // Compose, export and commit under the session lock, so a Verdict saved from another tab
    // can never slip in between composing the Review and recording it.
    let committed = st.update(|s| {
        if !s.is_open() {
            return Err(SessionError::NotOpen);
        }
        let review = compose(&s.entries, &diff, &choice, &cite_plain).map_err(|e| SessionError::Invalid(e.to_string()))?;
        if let Some(path) = &s.export_md {
            std::fs::write(path, to_markdown(&review))
                .map_err(|e| SessionError::Invalid(format!("Could not write the Review to {}: {e}", path.display())))?;
        }
        s.status = SessionStatus::Submitted;
        s.outcome = Some(Outcome { state: review.state, summary: choice.summary.clone(), export_path: s.export_md.clone() });
        Ok(())
    });
    match committed {
        Ok(()) => {}
        Err(AppError::Session(SessionError::Invalid(msg))) => return Ok(reject(&msg)),
        Err(e) => return Err(e),
    }
    st.finish();
    Ok(Html(pages::done("Review submitted. You can close this tab.").into_string()).into_response())
}

pub async fn abandon(State(st): State<AppState>) -> Result<Html<String>, AppError> {
    st.update(|s| {
        if !s.is_open() {
            return Err(SessionError::NotOpen);
        }
        s.status = SessionStatus::Abandoned;
        Ok(())
    })?;
    st.finish();
    Ok(Html(pages::done("Review abandoned. Nothing was posted. You can close this tab.").into_string()))
}
