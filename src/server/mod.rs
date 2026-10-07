//! The localhost web UI where the Reviewer adjudicates Findings.

mod handlers;
mod highlight;
mod pages;

use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tokio::sync::watch;

use crate::git::Repo;
use crate::session::{Session, SessionError, Store, Target};

pub fn url(token: &str, rest: &str) -> String {
    format!("/s/{token}/{rest}")
}

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    session: Mutex<Session>,
    store: Store,
    repo: Repo,
    port: u16,
    done: watch::Sender<bool>,
}

impl AppState {
    pub fn new(session: Session, store: Store, repo: Repo, port: u16) -> AppState {
        let (done, _) = watch::channel(false);
        AppState { inner: Arc::new(Inner { session: Mutex::new(session), store, repo, port, done }) }
    }

    pub fn snapshot(&self) -> Session {
        self.inner.session.lock().unwrap().clone()
    }

    pub fn token(&self) -> String {
        self.inner.session.lock().unwrap().token.clone()
    }

    pub fn repo(&self) -> &Repo {
        &self.inner.repo
    }

    pub fn port(&self) -> u16 {
        self.inner.port
    }

    /// Applies `f` to a copy of the session and persists it; the change is kept only if both succeed.
    pub fn update<T>(&self, f: impl FnOnce(&mut Session) -> Result<T, SessionError>) -> Result<T, AppError> {
        let mut session = self.inner.session.lock().unwrap();
        let mut draft = session.clone();
        let out = f(&mut draft)?;
        self.inner.store.save(&draft).map_err(AppError::Internal)?;
        *session = draft;
        Ok(out)
    }

    /// Whether the change has new commits since the session's Findings were proposed.
    pub fn is_stale(&self) -> bool {
        let s = self.snapshot();
        match &s.target {
            Target::Local { head_ref, .. } => !self.inner.repo.rev_parse(head_ref).is_ok_and(|sha| sha == s.head_sha),
        }
    }

    /// Signals that the session has ended and the server should shut down.
    pub fn finish(&self) {
        self.inner.done.send_replace(true);
    }

    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.inner.done.subscribe()
    }
}

#[derive(Debug)]
pub enum AppError {
    Session(SessionError),
    Internal(anyhow::Error),
}

impl From<SessionError> for AppError {
    fn from(e: SessionError) -> Self {
        AppError::Session(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::Session(e @ SessionError::NotOpen) => (StatusCode::CONFLICT, e.to_string()),
            AppError::Session(e @ SessionError::NoSuchFinding(_)) => (StatusCode::NOT_FOUND, e.to_string()),
            AppError::Session(e @ SessionError::Invalid(_)) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()),
            AppError::Internal(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        };
        (status, Html(pages::done(&message).into_string())).into_response()
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/s/{token}/", get(handlers::index))
        .route("/s/{token}/f/{idx}", get(handlers::finding_page))
        .route("/s/{token}/assets/{name}", get(handlers::asset))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// Rejects DNS-rebinding (foreign Host), cross-site form posts (foreign Origin) and unknown tokens.
async fn guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let port = state.port();
    let allowed = |value: &str, scheme: &str| {
        value == format!("{scheme}127.0.0.1:{port}") || value == format!("{scheme}localhost:{port}")
    };
    let host_ok = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).is_some_and(|h| allowed(h, ""));
    if !host_ok {
        return (StatusCode::FORBIDDEN, "forbidden host").into_response();
    }
    if req.method() != Method::GET
        && let Some(origin) = req.headers().get(header::ORIGIN)
        && !origin.to_str().is_ok_and(|o| allowed(o, "http://"))
    {
        return (StatusCode::FORBIDDEN, "forbidden origin").into_response();
    }
    if !req.uri().path().starts_with(&url(&state.token(), "")) {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FindingBatch;
    use crate::session::{Store, Target};
    use crate::testutil::{sample_batch, sample_repo, TestRepo};
    use crate::validate::validate;
    use axum::body::Body;
    use axum::http::{HeaderMap, Request as HttpRequest};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    pub(super) const PORT: u16 = 4321;
    pub(super) const HOST: &str = "127.0.0.1:4321";

    #[allow(dead_code)] // TEMP: fields and helpers used by Task 7–8 tests; removed in Task 8
    pub(super) struct Fixture {
        pub repo: TestRepo,
        pub _state_dir: tempfile::TempDir,
        pub store: Store,
        pub state: AppState,
        pub app: Router,
        pub token: String,
        pub id: String,
    }

    pub(super) fn fixture() -> Fixture {
        fixture_with(None, sample_batch())
    }

    pub(super) fn fixture_with(export_md: Option<std::path::PathBuf>, batch: FindingBatch) -> Fixture {
        let (repo, base, head) = sample_repo();
        let entries = validate(batch, &repo.repo(), &head).unwrap();
        let session = Session::new(
            repo.repo().root().to_path_buf(),
            Target::Local { base_ref: "main".into(), head_ref: "feature".into() },
            base,
            head,
            entries,
            export_md,
        );
        let state_dir = tempfile::tempdir().unwrap();
        let store = Store::new(state_dir.path().to_path_buf());
        store.save(&session).unwrap();
        let (token, id) = (session.token.clone(), session.id.clone());
        let state = AppState::new(session, store.clone(), repo.repo(), PORT);
        Fixture { app: router(state.clone()), repo, _state_dir: state_dir, store, state, token, id }
    }

    #[allow(dead_code)] // TEMP: removed in Task 8
    impl Fixture {
        pub fn path(&self, rest: &str) -> String {
            url(&self.token, rest)
        }
        pub async fn get(&self, rest: &str) -> (StatusCode, HeaderMap, String) {
            send(&self.app, Method::GET, &self.path(rest), None, HOST, None).await
        }
        pub async fn post(&self, rest: &str, form: &str) -> (StatusCode, HeaderMap, String) {
            send(&self.app, Method::POST, &self.path(rest), Some(form), HOST, None).await
        }
    }

    pub(super) async fn send(
        app: &Router,
        method: Method,
        uri: &str,
        form: Option<&str>,
        host: &str,
        extra: Option<(&str, &str)>,
    ) -> (StatusCode, HeaderMap, String) {
        let mut req = HttpRequest::builder().method(method).uri(uri).header(header::HOST, host);
        if let Some((k, v)) = extra {
            req = req.header(k, v);
        }
        let body = match form {
            Some(f) => {
                req = req.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
                Body::from(f.to_string())
            }
            None => Body::empty(),
        };
        let res = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let (status, headers) = (res.status(), res.headers().clone());
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, headers, String::from_utf8_lossy(&bytes).into_owned())
    }

    pub(super) fn location(h: &HeaderMap) -> &str {
        h[header::LOCATION].to_str().unwrap()
    }

    #[tokio::test]
    async fn rejects_foreign_host_and_wrong_token() {
        let fx = fixture();
        let (s, _, _) = send(&fx.app, Method::GET, &fx.path(""), None, "evil.example:4321", None).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, _, _) = send(&fx.app, Method::GET, "/s/wrong/", None, HOST, None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, _, _) = send(&fx.app, Method::GET, &fx.path(""), None, "localhost:4321", None).await;
        assert_eq!(s, StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn index_redirects_to_first_unruled_finding() {
        let fx = fixture();
        let (s, h, _) = fx.get("").await;
        assert_eq!(s, StatusCode::SEE_OTHER);
        assert_eq!(location(&h), fx.path("f/0"));
    }

    #[tokio::test]
    async fn finding_page_renders_evidence_and_escapes_agent_text() {
        let fx = fixture();
        let (s, _, body) = fx.get("f/0").await;
        assert_eq!(s, StatusCode::OK);
        assert!(body.contains("Retry loop never backs off"), "most severe finding comes first");
        assert!(body.contains("src/lib.rs:10-12"), "location is shown");
        assert!(body.contains(">12<") && body.contains("changed"), "location text is shown, numbered");
        assert!(body.contains(r#"class="mermaid""#));
        assert!(body.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!body.contains("<script>alert(1)"));
        assert!(body.contains(r#"value="block""#));
        assert!(body.contains(&fx.path("f/1")), "sidebar links to the other finding");
        let (s, _, _) = fx.get("f/9").await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn serves_assets() {
        let fx = fixture();
        let (s, h, body) = fx.get("assets/app.js").await;
        assert_eq!(s, StatusCode::OK);
        assert!(h[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/javascript"));
        assert!(body.contains("mermaid.initialize"));
        let (s, _, _) = fx.get("assets/nope.js").await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }

    #[test]
    fn code_block_numbers_lines() {
        let html = highlight::code_block("x.rs", "fn main() {}\nlet a = 1;", 41).into_string();
        assert!(html.contains(">41<") && html.contains(">42<"), "{html}");
        assert!(html.contains("main"));
        let plain = highlight::code_block("NOTES", "<b>", 1).into_string();
        assert!(plain.contains("&lt;b&gt;"), "{plain}");
    }
}
