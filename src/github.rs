//! The GitHub adapter. prr reaches GitHub only through the `gh` CLI, reusing its authentication.

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::compose::{ComposedReview, cite_plain};
use crate::session::{Posted, ReviewState};

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

/// How prr runs the GitHub CLI. `PRR_GH` replaces the command (whitespace-separated), e.g. in tests.
#[derive(Debug, Clone)]
pub struct Gh {
    program: OsString,
    prefix: Vec<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub head_sha: String,
    pub base_sha: String,
    pub author: String,
    /// `open` or `closed`.
    pub state: String,
}

#[derive(Deserialize)]
struct PullJson {
    state: String,
    user: Login,
    head: ShaJson,
    base: ShaJson,
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

#[derive(Deserialize)]
struct ShaJson {
    sha: String,
}

#[derive(Deserialize)]
struct ReviewJson {
    html_url: String,
    state: String,
}

impl Gh {
    pub fn new(program: impl Into<OsString>, prefix: Vec<OsString>) -> Gh {
        Gh { program: program.into(), prefix }
    }

    pub fn from_env() -> Gh {
        match std::env::var("PRR_GH") {
            Ok(cmd) if !cmd.trim().is_empty() => {
                let mut parts = cmd.split_whitespace().map(OsString::from);
                let program = parts.next().unwrap_or_else(|| "gh".into());
                Gh::new(program, parts.collect())
            }
            _ => Gh::new("gh", vec![]),
        }
    }

    /// The GitHub repository of the clone at `dir`.
    pub fn repo_of(&self, dir: &Path) -> Result<GithubRepo> {
        let out = self.run(dir, &["repo", "view", "--json", "url", "--jq", ".url"], None)?;
        let url = String::from_utf8_lossy(&out).trim().to_string();
        GithubRepo::from_url(&url).with_context(|| format!("gh reported an unexpected repository URL `{url}`"))
    }

    pub fn pull(&self, dir: &Path, repo: &GithubRepo, number: u64) -> Result<PullRequest> {
        let path = format!("repos/{}/pulls/{number}", repo.name);
        let pr: PullJson = self.api(dir, repo, &[&path], None)?;
        Ok(PullRequest { head_sha: pr.head.sha, base_sha: pr.base.sha, author: pr.user.login, state: pr.state })
    }

    /// The login `gh` is authenticated as.
    pub fn viewer(&self, dir: &Path, repo: &GithubRepo) -> Result<String> {
        Ok(self.api::<Login>(dir, repo, &["user"], None)?.login)
    }

    pub fn post_review(&self, dir: &Path, repo: &GithubRepo, number: u64, payload: &Value) -> Result<Posted> {
        let path = format!("repos/{}/pulls/{number}/reviews", repo.name);
        let body = serde_json::to_vec(payload)?;
        let review: ReviewJson = self.api(dir, repo, &["--method", "POST", &path, "--input", "-"], Some(&body))?;
        Ok(Posted { url: review.html_url, state: review.state })
    }

    fn api<T: DeserializeOwned>(&self, dir: &Path, repo: &GithubRepo, args: &[&str], stdin: Option<&[u8]>) -> Result<T> {
        let mut full = vec!["api"];
        if repo.host != "github.com" {
            full.extend(["--hostname", repo.host.as_str()]);
        }
        full.extend_from_slice(args);
        let out = self.run(dir, &full, stdin)?;
        serde_json::from_slice(&out).with_context(|| format!("unexpected response from gh {}", full.join(" ")))
    }

    fn run(&self, dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> Result<Vec<u8>> {
        let mut child = Command::new(&self.program)
            .args(&self.prefix)
            .args(args)
            .current_dir(dir)
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!("could not run {}; is the GitHub CLI installed and on PATH? (https://cli.github.com)", self.program.to_string_lossy())
            })?;
        if let Some(input) = stdin {
            child.stdin.take().expect("stdin is piped").write_all(input)?;
        }
        let out = child.wait_with_output()?;
        if !out.status.success() {
            bail!("gh {} failed: {}{}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim(), api_message(&out.stdout));
        }
        Ok(out.stdout)
    }
}

/// GitHub's error message and details from an API error body, if any.
fn api_message(stdout: &[u8]) -> String {
    let Ok(body) = serde_json::from_slice::<Value>(stdout) else { return String::new() };
    let mut parts: Vec<String> = body["message"].as_str().map(String::from).into_iter().collect();
    if let Some(errors) = body["errors"].as_array() {
        parts.extend(errors.iter().filter_map(|e| e.as_str().or_else(|| e["message"].as_str())).map(String::from));
    }
    if parts.is_empty() { String::new() } else { format!(" ({})", parts.join("; ")) }
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

    use crate::testutil::FakeGh;

    #[test]
    fn gh_reads_the_repo_pr_and_viewer() {
        let fake = FakeGh::new("h".repeat(40).as_str(), "b".repeat(40).as_str(), "author", "me");
        let gh = fake.gh();
        let dir = std::env::temp_dir();
        assert_eq!(gh.repo_of(&dir).unwrap(), gh_repo());
        let pr = gh.pull(&dir, &gh_repo(), 7).unwrap();
        assert_eq!(pr, PullRequest { head_sha: "h".repeat(40), base_sha: "b".repeat(40), author: "author".into(), state: "open".into() });
        assert_eq!(gh.viewer(&dir, &gh_repo()).unwrap(), "me");
        assert_eq!(fake.calls(), "repo view --json url --jq .url\napi repos/o/r/pulls/7\napi user\n");
    }

    #[test]
    fn gh_posts_the_payload() {
        let fake = FakeGh::new("h", "b", "author", "me");
        let posted = fake.gh().post_review(&std::env::temp_dir(), &gh_repo(), 7, &serde_json::json!({"body": "hi"})).unwrap();
        assert_eq!(posted.url, "https://github.com/o/r/pull/7#pullrequestreview-1");
        assert_eq!(posted.state, "COMMENTED");
        assert_eq!(fake.payload().unwrap(), serde_json::json!({"body": "hi"}));
        assert!(fake.calls().contains("api --method POST repos/o/r/pulls/7/reviews --input -"));
    }

    #[test]
    fn gh_names_non_github_hosts() {
        let fake = FakeGh::new("h", "b", "author", "me");
        let ghe = GithubRepo { host: "ghe.example.com".into(), name: "o/r".into() };
        fake.gh().pull(&std::env::temp_dir(), &ghe, 7).unwrap();
        assert!(fake.calls().contains("api --hostname ghe.example.com repos/o/r/pulls/7"), "{}", fake.calls());
    }

    #[test]
    fn gh_failures_carry_its_message() {
        let fake = FakeGh::new("h", "b", "author", "me");
        fake.fail_reviews("gh: Validation Failed (HTTP 422)");
        let err = fake.gh().post_review(&std::env::temp_dir(), &gh_repo(), 7, &serde_json::json!({})).unwrap_err();
        assert!(format!("{err:#}").contains("Validation Failed (HTTP 422)"), "{err:#}");
    }

    #[test]
    fn missing_gh_says_what_to_install() {
        let err = Gh::new("/nonexistent/gh", vec![]).repo_of(&std::env::temp_dir()).unwrap_err();
        assert!(format!("{err:#}").contains("GitHub CLI"), "{err:#}");
    }
}
