# prr GitHub Adapter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let `prr review --pr <n>` adjudicate a GitHub pull request and post the Review to it on submit (or as a pending review), reusing the local-mode core unchanged.

**Architecture:** A new `github` module wraps the `gh` CLI (`Gh`) and holds the pure GitHub-specific pieces: PR references, the review payload, the event mapping and permalinks. `Target` gains a `Github` variant; the server's stale check and submit step branch on it; `Outcome` records what was posted. Tests never reach GitHub: a fake `gh` shell script (`tests/fixtures/fake-gh.sh`) answers from files and records the payload, wired in through `PRR_GH`.

**Tech Stack:** Existing crate (Rust 2024, axum, maud, serde_json). No new dependencies. Requires `sh` for tests.

**Spec:** `docs/superpowers/specs/2026-10-07-prr-design.md` — section "GitHub adapter (plan 2)".

## Global Constraints

- prr talks to GitHub only via the `gh` CLI; `PRR_GH` (whitespace-separated command) overrides it. Never read or handle GitHub tokens.
- Calls used, exactly: `gh repo view --json url --jq .url`; `gh api [--hostname H] repos/{owner}/{repo}/pulls/{n}`; `gh api [--hostname H] user`; `gh api [--hostname H] --method POST repos/{owner}/{repo}/pulls/{n}/reviews --input -`. `--hostname` only when the host is not `github.com`.
- Posting happens only from the submit step, inside `AppState::update`, after composing and exporting; any failure leaves the session open and shows the error on the submit page (422).
- Own PR: event `COMMENT` instead of `REQUEST_CHANGES`/`APPROVE`; Approve disabled and refused.
- `--pending`: no `event` field. Nothing to post (state `None`): no API call.
- Local mode behaviour and all existing tests stay unchanged.
- Every task ends with `cargo test` green and `cargo clippy --all-targets -- -D warnings` clean.

## Review Focus

1. **GitHub rejects the review (422, network down, gh not logged in)** — the Reviewer sees GitHub's message on the submit page, the session stays open, nothing is marked posted. (Test: Task 3 `github_rejection_keeps_the_session_open`.)
2. **Reviewing your own PR** — never sends REQUEST_CHANGES/APPROVE, which GitHub would reject after the Reviewer did all the work. (Tests: Task 1 `review_event_on_own_pr_is_always_comment`, Task 3 `own_pr_posts_a_comment_and_refuses_approve`.)
3. **The PR's commits aren't in the clone** — exits 1 before any session, telling the Agent exactly what to run. (Test: Task 4 `pr_commits_missing_from_the_clone_explain_how_to_fetch`.)
4. **Paths with spaces or odd characters in body permalinks** — percent-encoded so the links work. (Test: Task 1 `permalink_encodes_the_path`.)
5. **The PR moves on GitHub while the Reviewer adjudicates** — stale warning and required acknowledgement, same as local mode. (Test: Task 3 `github_stale_when_the_pr_head_moves`.)

---

## File Structure

```
src/github.rs              NEW  Gh (gh CLI client), GithubRepo, PrRef, PullRequest, review_event, review_payload, permalink
src/session.rs             Target::Github, Target::github_repo, Posted, Outcome.posted
src/result.rs              posted / posted_url / posted_state from Outcome.posted
src/server/mod.rs          AppState holds Gh; stale check for GitHub; serve_session takes Gh
src/server/handlers.rs     cite_for (permalinks), submit posts to GitHub
src/server/pages.rs        submit page: destination, own-PR note, button label
src/cli.rs                 --pr / --pending, resolve_pr, Gh into run_server
src/testutil.rs            FakeGh, pr_json
tests/fixtures/fake-gh.sh  NEW  fake gh used by unit and CLI tests
tests/cli.rs               GitHub end-to-end tests
src/guide.md, skills/prr/SKILL.md, README.md   document --pr / --pending
```

---

### Task 1: GitHub model — target, posted outcome, payload, permalinks

**Files:**
- Create: `src/github.rs`
- Modify: `src/lib.rs`, `src/session.rs`, `src/result.rs`

**Interfaces:**
- Produces:
  - `session::Target::Github { host: String, repo: String, number: u64, own_pr: bool, pending: bool }`; `Target::label` → `"o/r#7"`; `Target::github_repo(&self) -> Option<github::GithubRepo>`
  - `session::Posted { url: String, state: String }`; `Outcome.posted: Option<Posted>` (`#[serde(default)]`)
  - `result::ReviewResult` gains `posted_url: Option<String>`, `posted_state: Option<String>`; `posted` = `outcome.posted.is_some()`
  - `github::GithubRepo { host: String, name: String }` with `from_url(&str) -> Option<GithubRepo>`, `web_url() -> String`
  - `github::PrRef { repo: Option<GithubRepo>, number: u64 }` with `parse(&str) -> anyhow::Result<PrRef>`
  - `github::review_event(state: ReviewState, own_pr: bool) -> &'static str`
  - `github::review_payload(review: &ComposedReview, head_sha: &str, event: Option<&str>) -> serde_json::Value`
  - `github::permalink(repo: &GithubRepo, sha: &str, path: &str, start: u32, end: u32) -> String`

- [ ] **Step 1: Write the failing tests** — create `src/github.rs`:

```rust
//! The GitHub adapter. prr reaches GitHub only through the `gh` CLI, reusing its authentication.

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
```

Add to `src/lib.rs`: `pub mod github;`

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib github`
Expected: compile errors — `PrRef`, `GithubRepo`, `review_event`, `review_payload`, `permalink` not found.

- [ ] **Step 3: Add the session and result model** — in `src/session.rs` replace the `Target` enum and its `impl`:

```rust
/// What is being reviewed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// A local git range with no Code host.
    Local { base_ref: String, head_ref: String },
    /// A GitHub pull request; `repo` is `owner/name`.
    Github { host: String, repo: String, number: u64, own_pr: bool, pending: bool },
}

impl Target {
    pub fn label(&self) -> String {
        match self {
            Target::Local { base_ref, head_ref } => format!("{base_ref}...{head_ref}"),
            Target::Github { repo, number, .. } => format!("{repo}#{number}"),
        }
    }

    pub fn github_repo(&self) -> Option<crate::github::GithubRepo> {
        match self {
            Target::Github { host, repo, .. } => Some(crate::github::GithubRepo { host: host.clone(), name: repo.clone() }),
            Target::Local { .. } => None,
        }
    }
}
```

and replace `Outcome` with:

```rust
/// A Review as accepted by the Code host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Posted {
    pub url: String,
    /// The host's state for the review, e.g. `COMMENTED` or `PENDING`.
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub state: Option<ReviewState>,
    pub summary: String,
    pub export_path: Option<PathBuf>,
    #[serde(default)]
    pub posted: Option<Posted>,
}
```

Add `posted: None` to every existing `Outcome { … }` literal: `src/server/handlers.rs` (submit), `src/session.rs` test `abandon_open_never_overwrites_a_finished_session`, `src/result.rs` test `result_flattens_verdicts`.

In `src/result.rs`, add to `ReviewResult` after `posted`:

```rust
    /// Link to the posted review, when one was posted.
    pub posted_url: Option<String>,
    /// The Code host's state for the posted review, e.g. `CHANGES_REQUESTED` or `PENDING`.
    pub posted_state: Option<String>,
```

and in `from_session` replace `posted: false,` with:

```rust
            posted: outcome.and_then(|o| o.posted.as_ref()).is_some(),
            posted_url: outcome.and_then(|o| o.posted.as_ref()).map(|p| p.url.clone()),
            posted_state: outcome.and_then(|o| o.posted.as_ref()).map(|p| p.state.clone()),
```

Extend `result_flattens_verdicts` with `assert_eq!(v["posted_url"], serde_json::Value::Null);`.

- [ ] **Step 4: Implement the pure GitHub pieces** — above the tests in `src/github.rs`:

```rust
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
```

The `Target` match in `AppState::is_stale` (`src/server/mod.rs`) must stay exhaustive; until Task 3 add the arm `Target::Github { .. } => true,` (conservative: treated as stale).

- [ ] **Step 5: Run tests and clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all pass (6 new in `github`), no warnings.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: add GitHub target, review payload and permalinks"
```

---

### Task 2: The `gh` client and the fake `gh`

**Files:**
- Create: `tests/fixtures/fake-gh.sh`
- Modify: `src/github.rs`, `src/testutil.rs`

**Interfaces:**
- Consumes: `GithubRepo`, `session::Posted`.
- Produces:
  - `github::Gh` (`Clone`, `Debug`): `Gh::new(program: impl Into<OsString>, prefix: Vec<OsString>)`, `Gh::from_env()`, `repo_of(&self, dir: &Path) -> Result<GithubRepo>`, `pull(&self, dir: &Path, repo: &GithubRepo, number: u64) -> Result<PullRequest>`, `viewer(&self, dir: &Path, repo: &GithubRepo) -> Result<String>`, `post_review(&self, dir: &Path, repo: &GithubRepo, number: u64, payload: &Value) -> Result<Posted>`
  - `github::PullRequest { head_sha, base_sha, author, state: String }`
  - `testutil::FakeGh`: `new(head, base, author, viewer)`, `gh() -> Gh`, `set_pr(head, base, author)`, `fail_reviews(stderr: &str)`, `payload() -> Option<Value>`, `calls() -> String`; `testutil::pr_json(head, base, author) -> Value`
  - The fake answers: `repo view…` → `https://github.com/o/r`; `api… user` → `{"login": <viewer>}`; `api… /pulls/N` → `pr.json`; `api… /reviews --input -` → records `payload.json`, replies `{"html_url": "https://github.com/o/r/pull/7#pullrequestreview-1", "state": "COMMENTED"}` (or fails with the `fail` file's text on stderr).

- [ ] **Step 1: Write the fake** — `tests/fixtures/fake-gh.sh`:

```sh
#!/bin/sh
# Stand-in for the GitHub CLI in prr's tests. Answers from files next to this script and logs every call.
D="$(dirname "$0")"
echo "$*" >> "$D/calls.log"
case "$*" in
  "repo view"*) echo "https://github.com/o/r" ;;
  *"/reviews --input -")
    if [ -f "$D/fail" ]; then cat "$D/fail" >&2; exit 1; fi
    cat > "$D/payload.json"
    echo '{"id":1,"html_url":"https://github.com/o/r/pull/7#pullrequestreview-1","state":"COMMENTED"}' ;;
  "api"*" user") printf '{"login":"%s"}\n' "$(cat "$D/viewer")" ;;
  "api"*"/pulls/"*) cat "$D/pr.json" ;;
  *) echo "unexpected gh call: $*" >&2; exit 2 ;;
esac
```

Append to `src/testutil.rs`:

```rust
use crate::github::Gh;

/// The JSON `gh api repos/o/r/pulls/7` would return, trimmed to what prr reads.
pub fn pr_json(head: &str, base: &str, author: &str) -> serde_json::Value {
    serde_json::json!({
        "number": 7,
        "state": "open",
        "html_url": "https://github.com/o/r/pull/7",
        "user": { "login": author },
        "head": { "sha": head },
        "base": { "sha": base },
    })
}

/// A fake `gh` (tests/fixtures/fake-gh.sh) run via `sh`, answering from files in its own directory.
pub struct FakeGh {
    dir: tempfile::TempDir,
}

impl FakeGh {
    pub fn new(head: &str, base: &str, author: &str, viewer: &str) -> FakeGh {
        let fake = FakeGh { dir: tempfile::tempdir().unwrap() };
        fs::write(fake.dir.path().join("gh.sh"), include_str!("../tests/fixtures/fake-gh.sh")).unwrap();
        fs::write(fake.dir.path().join("viewer"), viewer).unwrap();
        fake.set_pr(head, base, author);
        fake
    }

    pub fn gh(&self) -> Gh {
        Gh::new("sh", vec![self.dir.path().join("gh.sh").into()])
    }

    pub fn set_pr(&self, head: &str, base: &str, author: &str) {
        fs::write(self.dir.path().join("pr.json"), pr_json(head, base, author).to_string()).unwrap();
    }

    /// Makes the next review POST fail with `stderr`.
    pub fn fail_reviews(&self, stderr: &str) {
        fs::write(self.dir.path().join("fail"), stderr).unwrap();
    }

    pub fn payload(&self) -> Option<serde_json::Value> {
        let text = fs::read_to_string(self.dir.path().join("payload.json")).ok()?;
        Some(serde_json::from_str(&text).unwrap())
    }

    pub fn calls(&self) -> String {
        fs::read_to_string(self.dir.path().join("calls.log")).unwrap_or_default()
    }
}
```

- [ ] **Step 2: Write the failing tests** — append inside `mod tests` in `src/github.rs`:

```rust
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
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test --lib github`
Expected: compile errors — `Gh`, `PullRequest` not found.

- [ ] **Step 4: Implement the client** — in `src/github.rs`, extend the imports to:

```rust
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
```

and add:

```rust
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
```

- [ ] **Step 5: Run tests and clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all pass (5 new), no warnings.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: add gh client and a fake gh for tests"
```

---

### Task 3: Posting from the browser

**Files:**
- Modify: `src/server/mod.rs`, `src/server/handlers.rs`, `src/server/pages.rs`, `src/cli.rs` (only the `serve_session` call)

**Interfaces:**
- Consumes: `Gh`, `review_event`, `review_payload`, `permalink`, `Target::github_repo`, `FakeGh`.
- Produces: `AppState::new(session, store, repo, gh: Gh, port)`; `AppState::gh(&self) -> &Gh`; `serve_session(session, store, repo, gh: Gh, open_browser)`; `handlers::cite_for(&Session) -> impl Fn(&str, u32, u32) -> String`.

- [ ] **Step 1: Write the failing tests** — in `src/server/mod.rs` tests: add `use crate::github::Gh;` and `use crate::testutil::FakeGh;` to the test imports; replace the body of `fixture_with` after `let session = …;` with a call to a new shared helper, and add a GitHub fixture:

```rust
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
        serve(repo, session, Gh::new("gh", vec![]))
    }

    fn serve(repo: TestRepo, session: Session, gh: Gh) -> Fixture {
        let state_dir = tempfile::tempdir().unwrap();
        let store = Store::new(state_dir.path().to_path_buf());
        store.save(&session).unwrap();
        let (token, id) = (session.token.clone(), session.id.clone());
        let state = AppState::new(session, store.clone(), repo.repo(), gh, PORT);
        Fixture { app: router(state.clone()), repo, _state_dir: state_dir, store, state, token, id }
    }

    /// A session for o/r#7 whose fake `gh` reports the sample repo's commits; "author" opened the PR.
    fn github_fixture(own_pr: bool, pending: bool) -> (Fixture, FakeGh, String) {
        let (repo, base, head) = sample_repo();
        let fake = FakeGh::new(&head, &base, "author", if own_pr { "author" } else { "me" });
        let entries = validate(sample_batch(), &repo.repo(), &head).unwrap();
        let target = Target::Github { host: "github.com".into(), repo: "o/r".into(), number: 7, own_pr, pending };
        let session = Session::new(repo.repo().root().to_path_buf(), target, base, head.clone(), entries, None);
        (serve(repo, session, fake.gh()), fake, head)
    }

    #[tokio::test]
    async fn github_submit_posts_the_review_and_records_it() {
        let (fx, fake, head) = github_fixture(false, false);
        rule_all(&fx, "block").await;
        let (_, _, page) = fx.get("submit").await;
        assert!(page.contains("Posts the Review to o/r#7 on GitHub"), "{page}");
        let (s, _, body) = fx.post("submit", "summary=Thanks").await;
        assert_eq!(s, StatusCode::OK, "{body}");

        let p = fake.payload().expect("review was posted");
        assert_eq!(p["event"], "REQUEST_CHANGES");
        assert_eq!(p["commit_id"], head.as_str());
        assert_eq!(p["comments"].as_array().unwrap().len(), 1);
        assert_eq!(p["comments"][0]["start_line"], 10);
        assert_eq!(p["comments"][0]["line"], 12);
        let link = format!("[`src/lib.rs:1-2`](https://github.com/o/r/blob/{head}/src/lib.rs#L1-L2)");
        assert!(p["body"].as_str().unwrap().contains(&link), "{}", p["body"]);

        let posted = fx.store.load(&fx.id).unwrap().outcome.unwrap().posted.unwrap();
        assert_eq!(posted.url, "https://github.com/o/r/pull/7#pullrequestreview-1");
    }

    #[tokio::test]
    async fn own_pr_posts_a_comment_and_refuses_approve() {
        let (fx, fake, _) = github_fixture(true, false);
        rule_all(&fx, "comment").await;
        let (_, _, page) = fx.get("submit").await;
        assert!(page.contains("your own pull request"), "{page}");
        let (s, _, body) = fx.post("submit", "approve=on").await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("approve your own pull request"), "{body}");
        assert!(fake.payload().is_none());

        fx.post("f/0/ruling", "verdict=block&comment=Must+fix").await;
        let (s, _, _) = fx.post("submit", "summary=").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(fake.payload().unwrap()["event"], "COMMENT", "GitHub refuses REQUEST_CHANGES on your own PR");
    }

    #[tokio::test]
    async fn pending_review_has_no_event() {
        let (fx, fake, _) = github_fixture(false, true);
        rule_all(&fx, "comment").await;
        let (_, _, page) = fx.get("submit").await;
        assert!(page.contains("Create pending review"), "{page}");
        fx.post("submit", "summary=").await;
        assert!(fake.payload().unwrap().get("event").is_none());
    }

    #[tokio::test]
    async fn github_rejection_keeps_the_session_open() {
        let (fx, fake, _) = github_fixture(false, false);
        rule_all(&fx, "comment").await;
        fake.fail_reviews("gh: Validation Failed (HTTP 422)");
        let (s, _, body) = fx.post("submit", "summary=Keep+me").await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("GitHub did not accept the review") && body.contains("Validation Failed"), "{body}");
        assert!(body.contains("Keep me"));
        let saved = fx.store.load(&fx.id).unwrap();
        assert!(saved.is_open() && saved.outcome.is_none());
        assert!(!*fx.state.subscribe().borrow());
    }

    #[tokio::test]
    async fn nothing_to_post_makes_no_api_call() {
        let (fx, fake, _) = github_fixture(false, false);
        fx.post("f/0/ruling", "verdict=dismiss&reason=noise").await;
        fx.post("f/1/ruling", "verdict=dismiss&reason=noise").await;
        let (s, _, _) = fx.post("submit", "summary=").await;
        assert_eq!(s, StatusCode::OK);
        assert!(fake.payload().is_none());
        assert!(!fake.calls().contains("/reviews"));
    }

    #[tokio::test]
    async fn github_stale_when_the_pr_head_moves() {
        let (fx, fake, _) = github_fixture(false, false);
        rule_all(&fx, "comment").await;
        let (_, _, page) = fx.get("submit").await;
        assert!(!page.contains("new commits"));
        fake.set_pr(&"9".repeat(40), &"b".repeat(40), "author");
        let (_, _, page) = fx.get("submit").await;
        assert!(page.contains("new commits"), "{page}");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib server`
Expected: compile error — `AppState::new` takes 4 arguments, 5 supplied.

- [ ] **Step 3: Implement state and staleness** — in `src/server/mod.rs`:
  - add `use crate::github::Gh;`
  - add a `gh: Gh` field to `Inner`; change `AppState::new` to `pub fn new(session: Session, store: Store, repo: Repo, gh: Gh, port: u16) -> AppState` storing it; add

```rust
    pub fn gh(&self) -> &Gh {
        &self.inner.gh
    }
```

  - replace the `Target::Github { .. } => true,` arm in `is_stale` with

```rust
            Target::Github { number, .. } => {
                let repo = s.target.github_repo().expect("GitHub target");
                !self.inner.gh.pull(self.inner.repo.root(), &repo, *number).is_ok_and(|pr| pr.head_sha == s.head_sha)
            }
```

  - change `serve_session` to `pub async fn serve_session(session: Session, store: Store, repo: Repo, gh: Gh, open_browser: bool) -> anyhow::Result<ServeEnd>` and pass `gh` to `AppState::new(session, store, repo, gh, port)`.

In `src/cli.rs` `run_server`, call `server::serve_session(session, store, repo, crate::github::Gh::from_env(), open_browser)`.

- [ ] **Step 4: Implement posting** — in `src/server/handlers.rs` add imports `use crate::github::{permalink, review_event, review_payload};` and extend the session import with `Target`. Add:

```rust
/// Body citations: permalinks on GitHub, plain `path:lines` locally.
pub fn cite_for(s: &Session) -> impl Fn(&str, u32, u32) -> String + '_ {
    move |path, start, end| match s.target.github_repo() {
        Some(repo) => permalink(&repo, &s.head_sha, path, start, end),
        None => cite_plain(path, start, end),
    }
}
```

In `preview`, replace `&cite_plain` with `&cite_for(s)`. In `submit`, before `let committed = st.update(…)` add:

```rust
    let gh = st.gh().clone();
    let root = st.repo().root().to_path_buf();
```

and replace the closure body from `let review = compose(…)` through `s.outcome = …;` with:

```rust
        if choice.approve && matches!(s.target, Target::Github { own_pr: true, .. }) {
            return Err(SessionError::Invalid("GitHub doesn't let you approve your own pull request.".into()));
        }
        let review = compose(&s.entries, &diff, &choice, &cite_for(s)).map_err(|e| SessionError::Invalid(e.to_string()))?;
        if let Some(path) = &s.export_md {
            std::fs::write(path, to_markdown(&review))
                .map_err(|e| SessionError::Invalid(format!("Could not write the Review to {}: {e}", path.display())))?;
        }
        let posted = match (&s.target, review.state) {
            (Target::Github { number, own_pr, pending, .. }, Some(state)) => {
                let repo = s.target.github_repo().expect("GitHub target");
                let event = (!pending).then(|| review_event(state, *own_pr));
                let payload = review_payload(&review, &s.head_sha, event);
                let posted = gh
                    .post_review(&root, &repo, *number, &payload)
                    .map_err(|e| SessionError::Invalid(format!("GitHub did not accept the review: {e:#}")))?;
                Some(posted)
            }
            _ => None,
        };
        s.status = SessionStatus::Submitted;
        s.outcome = Some(Outcome { state: review.state, summary: choice.summary.clone(), export_path: s.export_md.clone(), posted });
```

- [ ] **Step 5: Implement the submit page copy** — in `src/server/pages.rs` add `Target` to the session import. At the top of `submit`, after `has_block`:

```rust
    let own_pr = matches!(s.target, Target::Github { own_pr: true, .. });
    let (destination, button) = match &s.target {
        Target::Local { .. } => ("Local mode: nothing is posted.".to_string(), "Submit review"),
        Target::Github { repo, number, pending: true, .. } => {
            (format!("Creates a pending review on {repo}#{number} that you finish on GitHub."), "Create pending review")
        }
        Target::Github { repo, number, .. } => (format!("Posts the Review to {repo}#{number} on GitHub."), "Submit review to GitHub"),
    };
```

In the form: change the Approve input's `disabled[has_block]` to `disabled[has_block || own_pr]`; after the Approve label add

```rust
                @if own_pr {
                    p.hint { "This is your own pull request. GitHub only accepts a comment review on it, so Blocks are posted as comments marked Blocking, and Approve is not available." }
                }
                p.hint { (destination) }
```

and change `button type="submit" { "Submit review" }` to `button type="submit" { (button) }`.

- [ ] **Step 6: Run tests and clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all pass (6 new), no warnings.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: post the review to GitHub on submit"
```

---

### Task 4: `prr review --pr` and documentation

**Files:**
- Modify: `src/cli.rs`, `tests/cli.rs`, `src/guide.md`, `skills/prr/SKILL.md`, `README.md`

**Interfaces:**
- Consumes: `Gh::from_env`, `PrRef::parse`, `Gh::{repo_of, pull, viewer}`.
- Produces: `prr review --pr <number|url> [--pending]` (`--base`/`--head` conflict with `--pr`; `--base` required without it); `cli::resolve_pr(repo: &Repo, spec: &str, pending: bool) -> Result<(Target, String /*base_sha*/, String /*head_sha*/)>`.

- [ ] **Step 1: Write the failing tests** — append to `tests/cli.rs`:

```rust
fn rev(dir: &Path, r: &str) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(["rev-parse", r]).output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Writes the fake gh next to `pr.json`/`viewer` in `dir` and returns the PRR_GH command for it.
fn fake_gh(dir: &Path, head: &str, base: &str, author: &str, viewer: &str) -> String {
    let pr = serde_json::json!({
        "number": 7, "state": "open", "html_url": "https://github.com/o/r/pull/7",
        "user": {"login": author}, "head": {"sha": head}, "base": {"sha": base},
    });
    std::fs::write(dir.join("pr.json"), pr.to_string()).unwrap();
    std::fs::write(dir.join("viewer"), viewer).unwrap();
    std::fs::write(dir.join("gh.sh"), include_str!("fixtures/fake-gh.sh")).unwrap();
    format!("sh {}", dir.join("gh.sh").display())
}

fn pr_review_cmd(repo: &Path, state: &Path, gh: &str, findings: &Path, extra: &[&str]) -> Command {
    let mut c = prr();
    c.current_dir(repo)
        .env("PRR_STATE_DIR", state)
        .env("PRR_GH", gh)
        .args(["review", "--findings"])
        .arg(findings)
        .args(["--pr", "7", "--no-open"])
        .args(extra);
    c
}

#[test]
fn github_review_posts_on_submit() {
    let (r, state) = (repo(), tempfile::tempdir().unwrap());
    let head = rev(r.path(), "feature");
    let gh = fake_gh(state.path(), &head, &rev(r.path(), "main"), "author", "me");
    let f = findings_file(state.path(), "src/lib.rs");
    let run = start(&mut pr_review_cmd(r.path(), state.path(), &gh, &f, &[]));
    post(&format!("{}f/0/ruling", run.url), &[("verdict", "block"), ("comment", "Please revert")]);
    post(&format!("{}submit", run.url), &[("summary", "Thanks")]);

    let (code, result) = result_of(run.child);
    assert_eq!(code, 0);
    assert_eq!(result["posted"], true);
    assert_eq!(result["posted_url"], "https://github.com/o/r/pull/7#pullrequestreview-1");
    assert_eq!(result["posted_state"], "COMMENTED");
    let payload: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state.path().join("payload.json")).unwrap()).unwrap();
    assert_eq!(payload["event"], "REQUEST_CHANGES");
    assert_eq!(payload["commit_id"], head.as_str());
    assert_eq!(payload["body"], "Thanks");
    assert_eq!(
        payload["comments"][0],
        serde_json::json!({"path": "src/lib.rs", "line": 3, "side": "RIGHT", "body": "**Blocking:** Please revert"})
    );
}

#[test]
fn pr_commits_missing_from_the_clone_explain_how_to_fetch() {
    let (r, state) = (repo(), tempfile::tempdir().unwrap());
    let gh = fake_gh(state.path(), &"0".repeat(40), &rev(r.path(), "main"), "author", "me");
    let f = findings_file(state.path(), "src/lib.rs");
    let out = pr_review_cmd(r.path(), state.path(), &gh, &f, &[]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("gh pr checkout 7"), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn closed_prs_are_refused() {
    let (r, state) = (repo(), tempfile::tempdir().unwrap());
    let gh = fake_gh(state.path(), &rev(r.path(), "feature"), &rev(r.path(), "main"), "author", "me");
    let pr = std::fs::read_to_string(state.path().join("pr.json")).unwrap().replace("\"open\"", "\"closed\"");
    std::fs::write(state.path().join("pr.json"), pr).unwrap();
    let f = findings_file(state.path(), "src/lib.rs");
    let out = pr_review_cmd(r.path(), state.path(), &gh, &f, &[]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("o/r#7 is closed"));
}

#[test]
fn pr_and_base_are_exclusive() {
    let (r, state) = (repo(), tempfile::tempdir().unwrap());
    let f = findings_file(state.path(), "src/lib.rs");
    let out = review_cmd(r.path(), state.path(), &f, &["--pr", "7"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "clap rejects --base with --pr");
    let out = prr().current_dir(r.path()).args(["review", "--findings"]).arg(&f).arg("--pending").output().unwrap();
    assert_eq!(out.status.code(), Some(2), "--pending needs --pr");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test cli`
Expected: the four new tests FAIL (unknown `--pr`).

- [ ] **Step 3: Implement** — in `src/cli.rs`, change `ReviewArgs`:

```rust
    /// Base of the change, e.g. `main` (local mode; not with --pr).
    #[arg(long, required_unless_present = "pr", conflicts_with = "pr")]
    base: Option<String>,
    /// Head of the change (local mode; not with --pr).
    #[arg(long, default_value = "HEAD", conflicts_with = "pr")]
    head: String,
    /// Review a GitHub pull request: its number in this clone's GitHub repository, or its URL.
    /// Needs the GitHub CLI (`gh`), authenticated, and the PR's commits in this clone.
    #[arg(long)]
    pr: Option<String>,
    /// With --pr: create a pending review that the Reviewer finishes on GitHub.
    #[arg(long, requires = "pr")]
    pending: bool,
```

In `review`, replace the lines from `let head_sha = …` through `let base_sha = …;` with:

```rust
    let (target, base_sha, head_sha) = match &args.pr {
        Some(spec) => resolve_pr(&repo, spec, args.pending)?,
        None => {
            let base = args.base.clone().context("--base is required without --pr")?;
            let head_sha = repo.rev_parse(&args.head)?;
            let base_sha = repo.merge_base(&repo.rev_parse(&base)?, &head_sha)?;
            (Target::Local { base_ref: base, head_ref: args.head.clone() }, base_sha, head_sha)
        }
    };
```

and in `Session::new(…)` replace `Target::Local { base_ref: args.base, head_ref: args.head }` with `target`. Add:

```rust
/// Looks the pull request up on GitHub and checks its commits are in this clone.
fn resolve_pr(repo: &Repo, spec: &str, pending: bool) -> Result<(Target, String, String)> {
    let gh = Gh::from_env();
    let pr_ref = PrRef::parse(spec)?;
    let gh_repo = match pr_ref.repo {
        Some(r) => r,
        None => gh.repo_of(repo.root())?,
    };
    let number = pr_ref.number;
    let name = format!("{}#{number}", gh_repo.name);
    let pr = gh.pull(repo.root(), &gh_repo, number)?;
    if pr.state != "open" {
        anyhow::bail!("{name} is {}; only open pull requests can be reviewed", pr.state);
    }
    for sha in [&pr.head_sha, &pr.base_sha] {
        if repo.rev_parse(sha).is_err() {
            anyhow::bail!(
                "commit {} of {name} is not in this clone; run `gh pr checkout {number}` (or `git fetch origin pull/{number}/head`) and retry",
                &sha[..sha.len().min(12)]
            );
        }
    }
    let base_sha = repo.merge_base(&pr.base_sha, &pr.head_sha)?;
    let own_pr = gh.viewer(repo.root(), &gh_repo)? == pr.author;
    let target = Target::Github { host: gh_repo.host, repo: gh_repo.name, number, own_pr, pending };
    Ok((target, base_sha, pr.head_sha))
}
```

with `use crate::github::{Gh, PrRef};` added to the imports, and `run_server` using `Gh::from_env()`.

- [ ] **Step 4: Document it** — in `src/guide.md`, after the Options paragraph of the Workflow section add:

```markdown
## GitHub pull requests

Review a pull request with `--pr` instead of `--base`/`--head`:

    prr review --findings findings.json --pr 42

`--pr` takes the PR number (in this clone's GitHub repository) or its URL. It needs the GitHub CLI (`gh`) logged in, and the PR's commits in this clone; if they are missing prr exits 1 and says to run `gh pr checkout 42`. When the Reviewer submits, prr posts the Review to the PR: one review, inline comments on changed lines, everything else in the body with permalinks. Add `--pending` to create a pending review the Reviewer finishes on GitHub instead. On the Reviewer's own PR GitHub only allows a comment review, so prr posts Blocks as comments marked Blocking.

The result then also has `"posted": true`, `posted_url` and `posted_state`. If GitHub rejects the review, the Reviewer sees why and the session stays open.
```

In `skills/prr/SKILL.md` step 3, append: ` For a GitHub pull request use `--pr <number>` instead of `--base` (check out its commits first with `gh pr checkout <number>`).` In `README.md`, add the line `prr review --findings findings.json --pr 42   # GitHub PR, posts on submit` to the code block.

- [ ] **Step 5: Run tests and clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all pass (4 new CLI tests), no warnings.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: add prr review --pr for GitHub pull requests"
```
