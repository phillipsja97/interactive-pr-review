use std::process::Command;

fn prr() -> Command {
    Command::new(env!("CARGO_BIN_EXE_prr"))
}

#[test]
fn version_prints() {
    let out = prr().arg("--version").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("prr "));
}

#[test]
fn schema_describes_the_findings_file() {
    let out = prr().arg("schema").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let schema: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(schema["properties"]["findings"].is_object(), "{schema}");
}

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=T", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

/// A repo whose `feature` branch (checked out) changes line 3 of src/lib.rs.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    std::fs::create_dir(p.join("src")).unwrap();
    std::fs::write(p.join("src/lib.rs"), "a\nb\nc\nd\ne\n").unwrap();
    git(p, &["add", "-A"]);
    git(p, &["commit", "-q", "-m", "base"]);
    git(p, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(p.join("src/lib.rs"), "a\nb\nC\nd\ne\n").unwrap();
    git(p, &["commit", "-qam", "change"]);
    dir
}

fn findings_file(dir: &Path, path: &str) -> PathBuf {
    let json = format!(
        r#"{{"findings":[{{"id":"F1","title":"Uppercase C","summary":"s","location":{{"kind":"lines","path":"{path}","start":3,"end":3}},"severity":"major","confidence":{{"level":"high","rationale":"read it"}},"impact":"i","recommendation":"r","draft_comment":"Why uppercase?"}}]}}"#
    );
    let f = dir.join("findings.json");
    std::fs::write(&f, json).unwrap();
    f
}

fn review_cmd(repo: &Path, state: &Path, findings: &Path, extra: &[&str]) -> Command {
    let mut c = prr();
    c.current_dir(repo)
        .env("PRR_STATE_DIR", state)
        .args(["review", "--findings"])
        .arg(findings)
        .args(["--base", "main", "--no-open"])
        .args(extra);
    c
}

struct Running {
    child: Child,
    url: String,
    id: String,
}

/// Spawns a prr command that serves a session and waits until it prints its id and URL.
fn start(cmd: &mut Command) -> Running {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let (mut url, mut id) = (None, None);
    while url.is_none() || id.is_none() {
        let line = rx.recv_timeout(Duration::from_secs(30)).expect("prr did not print its session URL");
        if let Some(rest) = line.strip_prefix("prr: open ") {
            url = Some(rest.to_string());
        }
        if let Some(rest) = line.strip_prefix("prr: session ") {
            id = Some(rest.to_string());
        }
    }
    // The reader thread keeps draining stderr after `rx` is dropped, so prr never blocks on a full pipe.
    Running { child, url: url.unwrap(), id: id.unwrap() }
}

fn post(url: &str, form: &[(&str, &str)]) -> String {
    ureq::post(url).send_form(form).unwrap().into_string().unwrap()
}

fn result_of(child: Child) -> (i32, serde_json::Value) {
    let out = child.wait_with_output().unwrap();
    let code = out.status.code().unwrap();
    let json = serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null);
    (code, json)
}

#[test]
fn invalid_findings_exit_10_with_json_errors() {
    let (r, state) = (repo(), tempfile::tempdir().unwrap());
    let f = findings_file(state.path(), "./src/lib.rs");
    let out = review_cmd(r.path(), state.path(), &f, &[]).output().unwrap();
    assert_eq!(out.status.code(), Some(10));
    let errs: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(errs["errors"][0]["path"], "/findings/0/location/path");
}

#[test]
fn full_review_prints_result_and_exports_markdown() {
    let (r, state) = (repo(), tempfile::tempdir().unwrap());
    let f = findings_file(state.path(), "src/lib.rs");
    let md = state.path().join("review.md");
    let run = start(&mut review_cmd(r.path(), state.path(), &f, &["--export-md", md.to_str().unwrap()]));

    let page = ureq::get(&run.url).call().unwrap().into_string().unwrap();
    assert!(page.contains("Uppercase C"));
    post(&format!("{}f/0/ruling", run.url), &[("verdict", "block"), ("comment", "Please revert")]);
    let done = post(&format!("{}submit", run.url), &[("summary", "Thanks")]);
    assert!(done.contains("submitted"));

    let (code, result) = result_of(run.child);
    assert_eq!(code, 0);
    assert_eq!(result["session_id"], run.id.as_str());
    assert_eq!(result["review_state"], "request_changes");
    assert_eq!(result["findings"][0]["verdict"], "block");
    assert_eq!(result["findings"][0]["comment"], "Please revert");
    let text = std::fs::read_to_string(md).unwrap();
    assert!(text.contains("### `src/lib.rs:3`\n\n**Blocking:** Please revert"), "{text}");
}

#[test]
fn abandon_exits_11() {
    let (r, state) = (repo(), tempfile::tempdir().unwrap());
    let f = findings_file(state.path(), "src/lib.rs");
    let run = start(&mut review_cmd(r.path(), state.path(), &f, &[]));
    post(&format!("{}abandon", run.url), &[]);
    let (code, result) = result_of(run.child);
    assert_eq!(code, 11);
    assert_eq!(result["status"], "abandoned");
}

#[test]
fn second_review_of_same_change_exits_12() {
    let (r, state) = (repo(), tempfile::tempdir().unwrap());
    let f = findings_file(state.path(), "src/lib.rs");
    let mut first = start(&mut review_cmd(r.path(), state.path(), &f, &[]));
    let out = review_cmd(r.path(), state.path(), &f, &[]).output().unwrap();
    assert_eq!(out.status.code(), Some(12));
    assert!(String::from_utf8_lossy(&out.stderr).contains(&format!("prr wait {}", first.id)));

    let out = review_cmd(r.path(), state.path(), &f, &["--replace"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "cannot replace a session another process is serving");
    first.child.kill().unwrap();
    first.child.wait().unwrap();
}

fn wait_cmd(state: &Path, id: &str) -> Command {
    let mut c = prr();
    c.env("PRR_STATE_DIR", state).args(["wait", id, "--no-open"]);
    c
}

#[test]
fn wait_resumes_a_killed_session_with_verdicts_intact() {
    let (r, state) = (repo(), tempfile::tempdir().unwrap());
    let f = findings_file(state.path(), "src/lib.rs");
    let mut first = start(&mut review_cmd(r.path(), state.path(), &f, &[]));
    post(&format!("{}f/0/ruling", first.url), &[("verdict", "nit"), ("comment", "Kept")]);
    first.child.kill().unwrap();
    first.child.wait().unwrap();

    let listed = prr().env("PRR_STATE_DIR", state.path()).args(["sessions", "--open"]).output().unwrap();
    let rows: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(rows[0]["id"], first.id.as_str());
    assert_eq!(rows[0]["unruled"], 0);

    let resumed = start(&mut wait_cmd(state.path(), &first.id));
    assert_eq!(resumed.id, first.id);
    let page = ureq::get(&resumed.url).call().unwrap().into_string().unwrap();
    assert!(page.contains("Submit review"), "all Findings already have a Verdict, so it opens on submit");
    post(&format!("{}submit", resumed.url), &[("summary", "")]);
    let (code, result) = result_of(resumed.child);
    assert_eq!(code, 0);
    assert_eq!(result["findings"][0]["verdict"], "nit");
    assert_eq!(result["findings"][0]["comment"], "Kept");

    let again = wait_cmd(state.path(), &first.id).output().unwrap();
    assert_eq!(again.status.code(), Some(0), "an ended session prints its result immediately");
    let result: serde_json::Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(result["status"], "submitted");
}

#[test]
fn wait_defers_to_a_live_server() {
    let (r, state) = (repo(), tempfile::tempdir().unwrap());
    let f = findings_file(state.path(), "src/lib.rs");
    let run = start(&mut review_cmd(r.path(), state.path(), &f, &[]));
    let waiter = wait_cmd(state.path(), &run.id).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    post(&format!("{}abandon", run.url), &[]);
    let (code, result) = result_of(run.child);
    assert_eq!(code, 11);
    let out = waiter.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(11));
    assert!(String::from_utf8_lossy(&out.stderr).contains("being served by another prr process"));
    let waited: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(waited, result);
}

#[test]
fn wait_on_unknown_session_fails() {
    let state = tempfile::tempdir().unwrap();
    let out = wait_cmd(state.path(), "nope").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no session `nope`"));
}

#[test]
fn guide_prints() {
    let out = prr().arg("guide").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("prr review --findings"));
}

#[test]
fn bad_export_path_fails_before_opening_a_session() {
    let (r, state) = (repo(), tempfile::tempdir().unwrap());
    let f = findings_file(state.path(), "src/lib.rs");
    let mut child = review_cmd(r.path(), state.path(), &f, &["--export-md", "/nonexistent/dir/out.md"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > deadline {
            child.kill().unwrap();
            panic!("prr opened a session with an export path it can never write");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--export-md"));
}

fn rev(dir: &Path, r: &str) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(["rev-parse", r]).output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Writes the fake gh next to `pr.json`/`viewer` in `dir` and returns the PRR_GH command for it.
fn fake_gh(dir: &Path, head: &str, base: &str, author: &str, viewer: &str) -> String {
    let pr = serde_json::json!({
        "number": 7, "state": "open", "html_url": "https://github.com/o/r/pull/7",
        "user": {"login": author}, "head": {"sha": head, "ref": "feature"}, "base": {"sha": base, "ref": "main"},
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

#[test]
fn missing_base_commit_says_to_fetch_the_base_branch() {
    let (r, state) = (repo(), tempfile::tempdir().unwrap());
    let gh = fake_gh(state.path(), &rev(r.path(), "feature"), &"0".repeat(40), "author", "me");
    let f = findings_file(state.path(), "src/lib.rs");
    let out = pr_review_cmd(r.path(), state.path(), &gh, &f, &[]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("base commit") && err.contains("`main`") && err.contains("git fetch"), "{err}");
    assert!(!err.contains("gh pr checkout"), "checking out the PR does not fetch its base: {err}");
}
