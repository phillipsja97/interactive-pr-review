//! The GitHub adapter. prr reaches GitHub only through the `gh` CLI, reusing its authentication.

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::compose::{ComposedReview, cite_plain};
use crate::session::ReviewState;

/// A GitHub repository: host plus `owner/name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubRepo {
    pub host: String,
    pub name: String,
}

impl GithubRepo {
    /// Parses `https://host/owner/name[.git][/more]`.
    pub fn from_url(url: &str) -> Option<GithubRepo> {
        let mut parts = url.split_once("://")?.1.split('/');
        let host = parts.next()?;
        let owner = parts.next()?;
        let name = parts.next()?.trim_end_matches(".git");
        if host.is_empty() || owner.is_empty() || name.is_empty() {
            return None;
        }
        Some(GithubRepo { host: host.into(), name: format!("{owner}/{name}") })
    }

    pub fn web_url(&self) -> String {
        format!("https://{}/{}", self.host, self.name)
    }
}

/// What the Agent passed to `--pr`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRef {
    /// `None` for a bare number: the clone's own repository.
    pub repo: Option<GithubRepo>,
    pub number: u64,
}

impl PrRef {
    pub fn parse(s: &str) -> Result<PrRef> {
        let s = s.trim();
        if let Ok(number) = s.parse() {
            return Ok(PrRef { repo: None, number });
        }
        let from_url = || {
            let repo = GithubRepo::from_url(s)?;
            let parts: Vec<&str> = s.split_once("://")?.1.split('/').collect();
            if parts.get(3) != Some(&"pull") {
                return None;
            }
            Some(PrRef { repo: Some(repo), number: parts.get(4)?.parse().ok()? })
        };
        from_url().context("--pr must be a pull request number or URL, e.g. 42 or https://github.com/owner/repo/pull/42")
    }
}

/// GitHub's review event for a Review state. GitHub refuses REQUEST_CHANGES and APPROVE on
/// your own pull request, so there every Review is a COMMENT (Blocks keep their label).
pub fn review_event(state: ReviewState, own_pr: bool) -> &'static str {
    match state {
        ReviewState::RequestChanges if !own_pr => "REQUEST_CHANGES",
        ReviewState::Approve if !own_pr => "APPROVE",
        _ => "COMMENT",
    }
}

/// The body of `POST /repos/{repo}/pulls/{n}/reviews`. Without an event GitHub creates a pending review.
pub fn review_payload(review: &ComposedReview, head_sha: &str, event: Option<&str>) -> Value {
    let comments: Vec<Value> = review
        .inline
        .iter()
        .map(|c| {
            let mut comment = json!({ "path": c.path, "line": c.line, "side": "RIGHT", "body": c.body });
            if c.start_line != c.line {
                comment["start_line"] = json!(c.start_line);
                comment["start_side"] = json!("RIGHT");
            }
            comment
        })
        .collect();
    let mut payload = json!({ "commit_id": head_sha, "body": review.body, "comments": comments });
    if let Some(event) = event {
        payload["event"] = json!(event);
    }
    payload
}

/// A Markdown link to lines of a file at a commit, used for citations in the Review body.
pub fn permalink(repo: &GithubRepo, sha: &str, path: &str, start: u32, end: u32) -> String {
    let anchor = if start == end { format!("L{start}") } else { format!("L{start}-L{end}") };
    format!("[{}]({}/blob/{sha}/{}#{anchor})", cite_plain(path, start, end), repo.web_url(), encode_path(path))
}

fn encode_path(path: &str) -> String {
    path.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::InlineComment;

    fn gh_repo() -> GithubRepo {
        GithubRepo { host: "github.com".into(), name: "o/r".into() }
    }

    #[test]
    fn pr_refs_parse_numbers_and_urls() {
        assert_eq!(PrRef::parse("42").unwrap(), PrRef { repo: None, number: 42 });
        assert_eq!(PrRef::parse("https://github.com/o/r/pull/42").unwrap(), PrRef { repo: Some(gh_repo()), number: 42 });
        let ghe = PrRef::parse("https://ghe.example.com/team/app/pull/9/files").unwrap();
        assert_eq!(ghe, PrRef { repo: Some(GithubRepo { host: "ghe.example.com".into(), name: "team/app".into() }), number: 9 });
        let err = PrRef::parse("https://github.com/o/r/issues/4").unwrap_err();
        assert!(err.to_string().contains("pull request number or URL"), "{err}");
        assert!(PrRef::parse("abc").is_err());
    }

    #[test]
    fn repos_come_from_web_urls() {
        assert_eq!(GithubRepo::from_url("https://github.com/o/r"), Some(gh_repo()));
        assert_eq!(GithubRepo::from_url("https://github.com/o/r.git"), Some(gh_repo()));
        assert_eq!(GithubRepo::from_url("github.com/o/r"), None);
        assert_eq!(gh_repo().web_url(), "https://github.com/o/r");
    }

    #[test]
    fn review_event_maps_states() {
        assert_eq!(review_event(ReviewState::RequestChanges, false), "REQUEST_CHANGES");
        assert_eq!(review_event(ReviewState::Approve, false), "APPROVE");
        assert_eq!(review_event(ReviewState::Comment, false), "COMMENT");
    }

    #[test]
    fn review_event_on_own_pr_is_always_comment() {
        assert_eq!(review_event(ReviewState::RequestChanges, true), "COMMENT");
        assert_eq!(review_event(ReviewState::Approve, true), "COMMENT");
    }

    #[test]
    fn payload_carries_inline_comments_on_the_head_side() {
        let review = ComposedReview {
            state: Some(ReviewState::RequestChanges),
            body: "Thanks".into(),
            inline: vec![
                InlineComment { entry_id: "a".into(), path: "src/x.rs".into(), start_line: 3, line: 3, body: "one".into() },
                InlineComment { entry_id: "b".into(), path: "src/x.rs".into(), start_line: 10, line: 12, body: "range".into() },
            ],
        };
        let p = review_payload(&review, "abc123", Some("REQUEST_CHANGES"));
        assert_eq!(p["commit_id"], "abc123");
        assert_eq!(p["event"], "REQUEST_CHANGES");
        assert_eq!(p["body"], "Thanks");
        assert_eq!(p["comments"][0], serde_json::json!({"path": "src/x.rs", "line": 3, "side": "RIGHT", "body": "one"}));
        assert_eq!(
            p["comments"][1],
            serde_json::json!({"path": "src/x.rs", "line": 12, "side": "RIGHT", "start_line": 10, "start_side": "RIGHT", "body": "range"})
        );
        assert!(review_payload(&review, "abc123", None).get("event").is_none(), "pending reviews have no event");
    }

    #[test]
    fn permalink_encodes_the_path() {
        assert_eq!(
            permalink(&gh_repo(), "abc", "src/lib.rs", 1, 2),
            "[`src/lib.rs:1-2`](https://github.com/o/r/blob/abc/src/lib.rs#L1-L2)"
        );
        assert_eq!(
            permalink(&gh_repo(), "abc", "my dir/a#b.rs", 5, 5),
            "[`my dir/a#b.rs:5`](https://github.com/o/r/blob/abc/my%20dir/a%23b.rs#L5)"
        );
    }
}
