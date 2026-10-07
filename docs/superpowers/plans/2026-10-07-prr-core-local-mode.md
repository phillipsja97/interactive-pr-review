# prr Core (Local Mode) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `prr`, a single Rust binary an Agent invokes to have a human Reviewer adjudicate its Findings in a localhost browser UI, for a local git range, returning the Verdicts as JSON and optionally exporting the composed Review as markdown.

**Architecture:** The Agent writes a findings JSON file; `prr review` validates it against the repo (resolving code excerpts from git itself), persists a Review session to disk, serves a server-rendered web UI on `127.0.0.1` behind a per-session token, and blocks until the Reviewer submits or abandons. Pure modules (model, validate, session, compose) carry the logic; `server` is a thin axum layer; `cli` wires it together. GitHub posting and Copilot import are later plans built on this core.

**Tech Stack:** Rust 2024 (≥1.89 for `File::try_lock`), clap 4 (derive), serde/serde_json, schemars 1, axum 0.8 + tokio, maud 0.27, syntect 5 (`default-fancy`, pure Rust), uuid v4, dirs, webbrowser, thiserror/anyhow. Dev: tempfile, tower (util), http-body-util, ureq 2. Mermaid 11.4.1 vendored JS.

**Spec:** `docs/superpowers/specs/2026-10-07-prr-design.md` (glossary: `CONTEXT.md`; decisions: `docs/adr/`).

## Global Constraints

- Binary and crate name: `prr`. Single binary; no Node/JS toolchain at build or run time; all assets embedded with `include_str!`.
- Exit codes: `0` submitted · `1` error · `10` invalid findings · `11` abandoned · `12` session already open · `130` interrupted. Never `2` (clap's usage-error code).
- stdout carries only the result JSON (or `schema`/`guide`/`sessions` output). Human/agent progress goes to stderr, with exact line prefixes `prr: session <id>` and `prr: open <url>`.
- prr never calls an LLM (ADR 0001).
- Server binds `127.0.0.1:0`; all routes under `/s/<token>/`; Host must be `127.0.0.1:<port>` or `localhost:<port>`; non-GET with a foreign `Origin` → 403.
- State dir: `$PRR_STATE_DIR` if set, else `dirs::state_dir()/prr`, else `dirs::data_local_dir()/prr`; sessions in `<state>/sessions/<id>.json`, written atomically.
- Use the glossary terms from `CONTEXT.md` in user-facing copy: Finding, Verdict, Block, Comment, Nit, Dismiss, Escalate, Reviewer, Review.
- Every task ends with `cargo test` passing and `cargo clippy --all-targets -- -D warnings` clean.

## Review Focus

1. **A findings file with zero Findings** — the session must still open and go straight to the submit screen, where the Reviewer can approve, add their own findings, or post nothing. (Tests: Task 4 `empty_batch_is_valid`, Task 8 `empty_batch_goes_straight_to_submit`.)
2. **Agent-supplied paths like `/abs/path`, `./src/x.rs`, `src\x.rs`, `../x`** — rejected with a message saying paths must be repo-relative with forward slashes, not a confusing "file not found". (Test: Task 4 `rejects_non_repo_relative_paths`.)
3. **Agent process killed mid-Adjudication** — the session stays open on disk with all Verdicts made so far, and `prr wait <id>` resumes it. (Test: Task 10 `wait_resumes_a_killed_session_with_verdicts_intact`.)
4. **Two prr processes for one session** (`prr review` still alive when `prr wait` runs) — the second must not serve concurrently; it waits and prints the same result. (Tests: Task 3 `lock_is_exclusive`, Task 10 `wait_defers_to_a_live_server`.)
5. **HTML/script in Agent-supplied text** (titles, command output, Mermaid) — rendered inert. (Test: Task 6 `finding_page_renders_evidence_and_escapes_agent_text`.)

---

## File Structure

```
Cargo.toml
src/main.rs              entry: exit with cli::run()
src/lib.rs               module list
src/cli.rs               clap commands, exit codes, review/wait/sessions orchestration
src/model.rs             findings-file types (FindingBatch, Finding, Location, Severity, Confidence, Evidence), example_batch()
src/git.rs               Repo (shells out to git), DiffMap (head-side hunk ranges)
src/session.rs           Session, Entry, Proposal, Origin, Verdict, Ruling, ReviewState, Outcome, Target, Store
src/validate.rs          parse_batch, validate → Vec<Entry>, resolve_lines, check_repo_path
src/compose.rs           review_state, compose → ComposedReview, to_markdown, cite_plain
src/result.rs            ReviewResult (stdout contract)
src/guide.rs + guide.md  agent-facing guide
src/server/mod.rs        AppState, AppError, router, guard, serve_session
src/server/handlers.rs   route handlers
src/server/pages.rs      maud pages
src/server/highlight.rs  syntect code blocks
src/testutil.rs          #[cfg(test)] TestRepo, sample_repo, sample_batch, finding
assets/app.css, assets/app.js, assets/vendor/mermaid.min.js (+ README.md)
tests/cli.rs             end-to-end tests against the built binary
skills/prr/SKILL.md      Claude Code skill
README.md
```

---

### Task 1: Scaffold, Finding model, `prr schema`

**Files:**
- Create: `Cargo.toml` (via cargo), `src/main.rs`, `src/lib.rs`, `src/cli.rs`, `src/model.rs`, `tests/cli.rs`

**Interfaces:**
- Produces: `prr::model::{FindingBatch, Finding, Location, Severity, ConfidenceLevel, Confidence, Evidence, example_batch}`; `Severity::label(self) -> &'static str`; `ConfidenceLevel::label(self) -> &'static str`; `prr::cli::{run() -> i32, exit::{OK, ERROR, INVALID_FINDINGS, ABANDONED, SESSION_EXISTS, INTERRUPTED}}`; `prr schema` command.

- [ ] **Step 1: Initialise the repo and crate**

```bash
cd /home/jphill/Projects/rust/interactive-pr-review
git init -b main
cargo init --name prr
cargo add anyhow axum dirs maud schemars serde_json thiserror webbrowser
cargo add clap --features derive
cargo add serde --features derive
cargo add tokio --features full
cargo add uuid --features v4
cargo add syntect --no-default-features --features default-fancy
cargo add --dev http-body-util tempfile ureq@2
cargo add --dev tower --features util
```

Then add `rust-version = "1.89"` under `[package]` in `Cargo.toml`.

- [ ] **Step 2: Write the failing model tests** — create `src/model.rs` with only the test module first:

```rust
//! The findings file an Agent hands to `prr review`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_round_trips() {
        let json = serde_json::to_string(&example_batch()).unwrap();
        let back: FindingBatch = serde_json::from_str(&json).unwrap();
        assert_eq!(back, example_batch());
    }

    #[test]
    fn unknown_finding_field_is_rejected() {
        let mut v = serde_json::to_value(example_batch()).unwrap();
        v["findings"][0]["severity_level"] = "major".into();
        let err = serde_json::from_value::<FindingBatch>(v).unwrap_err();
        assert!(err.to_string().contains("severity_level"), "{err}");
    }

    #[test]
    fn wire_format_uses_snake_case_tags() {
        let v = serde_json::to_value(example_batch()).unwrap();
        let f = &v["findings"][0];
        assert_eq!(f["location"]["kind"], "lines");
        assert_eq!(f["severity"], "major");
        assert_eq!(f["confidence"]["level"], "high");
        assert_eq!(f["evidence"][0]["kind"], "code_excerpt");
        assert!(f["evidence"][0].get("ref").is_none());
        assert_eq!(f["evidence"][3]["kind"], "command_output");
    }

    #[test]
    fn pr_wide_location_parses() {
        let l: Location = serde_json::from_str(r#"{"kind":"pr_wide"}"#).unwrap();
        assert_eq!(l, Location::PrWide);
    }

    #[test]
    fn severity_and_confidence_order_by_strength() {
        assert!(Severity::Critical > Severity::Major);
        assert!(Severity::Major > Severity::Minor);
        assert!(Severity::Minor > Severity::Info);
        assert!(ConfidenceLevel::High > ConfidenceLevel::Medium);
        assert!(ConfidenceLevel::Medium > ConfidenceLevel::Low);
    }
}
```

`src/lib.rs`:

```rust
pub mod cli;
pub mod model;
```

`src/main.rs`:

```rust
fn main() {
    std::process::exit(prr::cli::run());
}
```

`src/cli.rs` (minimal so it compiles):

```rust
pub fn run() -> i32 {
    0
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --lib model`
Expected: compile errors — `cannot find function example_batch`, `cannot find type FindingBatch`.

- [ ] **Step 4: Implement the model** — put this above the test module in `src/model.rs`:

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The document an Agent passes to `prr review --findings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindingBatch {
    pub findings: Vec<Finding>,
}

/// One claim by the Agent about a problem or concern in the change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    /// Unique within the batch. Must not start with `reviewer-`.
    pub id: String,
    /// One line, shown as the heading.
    pub title: String,
    /// What the problem is.
    pub summary: String,
    /// Where it is, on the head commit.
    pub location: Location,
    pub severity: Severity,
    pub confidence: Confidence,
    /// Who or what breaks, and how.
    pub impact: String,
    /// What should be done about it.
    pub recommendation: String,
    /// Replacement text for the location's lines; posted as a suggestion block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_patch: Option<String>,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    /// The comment that would be posted. The Reviewer may edit it.
    pub draft_comment: String,
    /// Suggested `@user` or `@org/team` if the Reviewer escalates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalate_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Location {
    /// Lines `start..=end` (1-based) of `path` on the head commit.
    Lines { path: String, start: u32, end: u32 },
    /// Not tied to any line.
    PrWide,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Minor,
    Major,
    Critical,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Info => "Info",
            Severity::Minor => "Minor",
            Severity::Major => "Major",
            Severity::Critical => "Critical",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceLevel {
    Low,
    Medium,
    High,
}

impl ConfidenceLevel {
    pub fn label(self) -> &'static str {
        match self {
            ConfidenceLevel::Low => "Low",
            ConfidenceLevel::Medium => "Medium",
            ConfidenceLevel::High => "High",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Confidence {
    pub level: ConfidenceLevel,
    /// Why this level, e.g. "traced all 4 callers" or "pattern match only".
    pub rationale: String,
}

/// Support for a Finding's claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence {
    /// Lines of a file. prr reads the text from git; the Agent supplies only the reference.
    CodeExcerpt {
        path: String,
        start: u32,
        end: u32,
        /// Commit-ish to read from; defaults to the head commit.
        #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
    },
    /// A Mermaid diagram.
    Diagram {
        mermaid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
    },
    Note {
        text: String,
    },
    CommandOutput {
        command: String,
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },
}

/// A complete, valid example used by `prr guide` and tests.
pub fn example_batch() -> FindingBatch {
    FindingBatch {
        findings: vec![Finding {
            id: "F1".into(),
            title: "Retry loop never backs off".into(),
            summary: "`fetch` retries immediately on 503, with no delay between attempts.".into(),
            location: Location::Lines { path: "src/lib.rs".into(), start: 10, end: 12 },
            severity: Severity::Major,
            confidence: Confidence {
                level: ConfidenceLevel::High,
                rationale: "Traced every caller of `fetch`; none adds a delay.".into(),
            },
            impact: "A struggling upstream gets a burst of retries, which can turn a brief outage into a long one.".into(),
            recommendation: "Sleep between attempts with exponential backoff.".into(),
            suggested_patch: Some("    sleep(backoff);\n    backoff *= 2;\n    attempt += 1;".into()),
            evidence: vec![
                Evidence::CodeExcerpt {
                    path: "src/lib.rs".into(),
                    start: 10,
                    end: 12,
                    git_ref: None,
                    caption: Some("The retry loop".into()),
                },
                Evidence::Diagram {
                    mermaid: "sequenceDiagram\n  Client->>Server: request\n  Server-->>Client: 503\n  Client->>Server: retry (no delay)".into(),
                    caption: Some("What happens during an outage".into()),
                },
                Evidence::Note { text: "The previous implementation used `backoff::retry`.".into() },
                Evidence::CommandOutput {
                    command: "cargo test retry".into(),
                    output: "test retry_backs_off ... FAILED".into(),
                    exit_code: Some(101),
                },
            ],
            draft_comment: "This retries immediately on 503. Could we back off between attempts so we don't hammer the upstream during an outage?".into(),
            escalate_to: None,
        }],
    }
}
```

- [ ] **Step 5: Run model tests to verify they pass**

Run: `cargo test --lib model`
Expected: 5 passed.

- [ ] **Step 6: Write the failing CLI test** — create `tests/cli.rs`:

```rust
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
```

- [ ] **Step 7: Run to verify failure**

Run: `cargo test --test cli`
Expected: FAIL — `--version` prints nothing; `schema` output is not JSON.

- [ ] **Step 8: Implement the CLI skeleton** — replace `src/cli.rs`:

```rust
//! Command-line entry point.

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::model::FindingBatch;

pub mod exit {
    pub const OK: i32 = 0;
    pub const ERROR: i32 = 1;
    pub const INVALID_FINDINGS: i32 = 10;
    pub const ABANDONED: i32 = 11;
    pub const SESSION_EXISTS: i32 = 12;
    pub const INTERRUPTED: i32 = 130;
}

#[derive(Parser)]
#[command(
    name = "prr",
    version,
    about = "Human-in-the-loop adjudication of agent-proposed pull request review findings"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the JSON Schema of the findings file.
    Schema,
}

pub fn run() -> i32 {
    let cli = Cli::parse();
    match dispatch(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("prr: error: {e:#}");
            exit::ERROR
        }
    }
}

fn dispatch(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Schema => {
            let schema = schemars::schema_for!(FindingBatch);
            println!("{}", serde_json::to_string_pretty(&schema)?);
            Ok(exit::OK)
        }
    }
}
```

- [ ] **Step 9: Run all tests and clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all pass, no warnings.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat: scaffold prr with findings model and schema command"
```

---

### Task 2: Git access and diff hunk map

**Files:**
- Create: `src/git.rs`, `src/testutil.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `prr::git::Repo` (`Clone`, `Debug`): `Repo::discover(dir: &Path) -> anyhow::Result<Repo>`, `root(&self) -> &Path`, `rev_parse(&self, rev: &str) -> Result<String>` (full SHA; error context `unknown git ref`), `merge_base(&self, a: &str, b: &str) -> Result<String>`, `show_file(&self, sha: &str, path: &str) -> Result<Option<Vec<u8>>>` (`None` if missing or not a file), `diff_map(&self, base: &str, head: &str) -> Result<DiffMap>`.
  - `prr::git::DiffMap` (`Default`, `Clone`, `Debug`, `PartialEq`): `DiffMap::parse(diff: &str) -> DiffMap`, `contains(&self, path: &str, start: u32, end: u32) -> bool` (true iff the whole range lies inside one head-side hunk).
  - `crate::testutil` (`#[cfg(test)]`): `TestRepo { new(), path(), git(&[&str]) -> String, write(path, contents), commit(msg) -> String (sha), repo() -> Repo }`; `sample_repo() -> (TestRepo, String /*base sha*/, String /*head sha*/)` — `main` has `src/lib.rs` with lines `line 1`..`line 20`; branch `feature` (checked out) changes lines 10–12 to `changed 10`..`changed 12`.

- [ ] **Step 1: Write the test helper** — `src/testutil.rs`:

```rust
//! Test fixtures shared by unit tests.

use std::fs;
use std::path::Path;
use std::process::Command;

use crate::git::Repo;

pub struct TestRepo {
    dir: tempfile::TempDir,
}

impl TestRepo {
    pub fn new() -> TestRepo {
        let t = TestRepo { dir: tempfile::tempdir().unwrap() };
        t.git(&["init", "-q", "-b", "main"]);
        t
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(self.path())
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }

    pub fn write(&self, path: &str, contents: &str) {
        let p = self.path().join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    pub fn commit(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    pub fn repo(&self) -> Repo {
        Repo::discover(self.path()).unwrap()
    }
}

/// `main` has a 20-line `src/lib.rs`; `feature` (checked out) changes lines 10–12.
pub fn sample_repo() -> (TestRepo, String, String) {
    let t = TestRepo::new();
    let lines: Vec<String> = (1..=20).map(|i| format!("line {i}")).collect();
    t.write("src/lib.rs", &(lines.join("\n") + "\n"));
    let base = t.commit("base");
    t.git(&["checkout", "-q", "-b", "feature"]);
    let mut changed = lines.clone();
    for (i, line) in changed.iter_mut().enumerate().take(12).skip(9) {
        *line = format!("changed {}", i + 1);
    }
    t.write("src/lib.rs", &(changed.join("\n") + "\n"));
    let head = t.commit("change");
    (t, base, head)
}
```

Add to `src/lib.rs`:

```rust
pub mod git;
#[cfg(test)]
pub(crate) mod testutil;
```

- [ ] **Step 2: Write the failing tests** — `src/git.rs` with tests only:

```rust
//! Read-only access to the repository via the `git` CLI.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::sample_repo;

    const DIFF: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -7,9 +7,9 @@ fn x()
 line 7
-line 10
+changed 10
+++ not a header, an added line starting with ++
@@ -40 +40 @@
-a
+b
diff --git a/gone.rs b/gone.rs
deleted file mode 100644
--- a/gone.rs
+++ /dev/null
@@ -1,2 +0,0 @@
-x
-y
";

    #[test]
    fn diff_map_records_head_side_hunks() {
        let map = DiffMap::parse(DIFF);
        assert!(map.contains("src/lib.rs", 7, 15));
        assert!(map.contains("src/lib.rs", 10, 12));
        assert!(!map.contains("src/lib.rs", 6, 7));
        assert!(!map.contains("src/lib.rs", 14, 16));
        assert!(map.contains("src/lib.rs", 40, 40));
        assert!(!map.contains("src/lib.rs", 41, 41));
        assert!(!map.contains("gone.rs", 1, 1));
    }

    #[test]
    fn repo_resolves_refs_and_files() {
        let (t, base, head) = sample_repo();
        let repo = t.repo();
        assert_eq!(repo.rev_parse("HEAD").unwrap(), head);
        assert_eq!(repo.rev_parse("main").unwrap(), base);
        let err = repo.rev_parse("no-such-branch").unwrap_err();
        assert!(format!("{err:#}").contains("unknown git ref `no-such-branch`"), "{err:#}");
        assert_eq!(repo.merge_base("main", "feature").unwrap(), base);

        let text = repo.show_file(&head, "src/lib.rs").unwrap().unwrap();
        assert!(String::from_utf8(text).unwrap().contains("changed 11"));
        assert_eq!(repo.show_file(&head, "missing.rs").unwrap(), None);
        assert_eq!(repo.show_file(&head, "src").unwrap(), None, "directories are not files");
    }

    #[test]
    fn repo_diff_map_covers_the_change() {
        let (t, base, head) = sample_repo();
        let map = t.repo().diff_map(&base, &head).unwrap();
        assert!(map.contains("src/lib.rs", 10, 12));
        assert!(!map.contains("src/lib.rs", 1, 2));
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test --lib git`
Expected: compile errors — `DiffMap`/`Repo` not found.

- [ ] **Step 4: Implement** — above the tests in `src/git.rs`:

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone)]
pub struct Repo {
    root: PathBuf,
}

impl Repo {
    pub fn discover(dir: &Path) -> Result<Repo> {
        let out = run_git(dir, &["rev-parse", "--show-toplevel"])
            .with_context(|| format!("{} is not inside a git repository", dir.display()))?;
        Ok(Repo { root: PathBuf::from(out.trim()) })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Full commit SHA for `rev`.
    pub fn rev_parse(&self, rev: &str) -> Result<String> {
        let spec = format!("{rev}^{{commit}}");
        let out = run_git(&self.root, &["rev-parse", "--verify", "--quiet", &spec])
            .with_context(|| format!("unknown git ref `{rev}`"))?;
        Ok(out.trim().to_string())
    }

    pub fn merge_base(&self, a: &str, b: &str) -> Result<String> {
        Ok(run_git(&self.root, &["merge-base", a, b])?.trim().to_string())
    }

    /// Contents of `path` at `sha`; `None` if it does not exist there or is not a file.
    pub fn show_file(&self, sha: &str, path: &str) -> Result<Option<Vec<u8>>> {
        let spec = format!("{sha}:{path}");
        let kind = self.git().args(["cat-file", "-t", &spec]).output().context("failed to run git")?;
        if !kind.status.success() || kind.stdout.trim_ascii() != b"blob" {
            return Ok(None);
        }
        let out = self.git().args(["cat-file", "blob", &spec]).output().context("failed to run git")?;
        if !out.status.success() {
            bail!("git cat-file blob {spec} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(Some(out.stdout))
    }

    pub fn diff_map(&self, base: &str, head: &str) -> Result<DiffMap> {
        let diff = run_git(
            &self.root,
            &["-c", "core.quotePath=false", "diff", "--no-color", "--no-ext-diff", "-U3", base, head],
        )?;
        Ok(DiffMap::parse(&diff))
    }

    fn git(&self) -> Command {
        let mut c = Command::new("git");
        c.arg("-C").arg(&self.root);
        c
    }
}

fn run_git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().context("failed to run git")?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    String::from_utf8(out.stdout).context("git output was not UTF-8")
}

/// Head-side line ranges covered by each file's diff hunks — the lines a Code host accepts inline comments on.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DiffMap {
    hunks: HashMap<String, Vec<(u32, u32)>>,
}

impl DiffMap {
    pub fn parse(diff: &str) -> DiffMap {
        let mut map = DiffMap::default();
        let mut path: Option<String> = None;
        let mut in_header = false;
        for line in diff.lines() {
            if line.starts_with("diff --git ") {
                in_header = true;
                path = None;
            } else if let Some(target) = line.strip_prefix("+++ ").filter(|_| in_header) {
                let target = unquote(target);
                path = (target != "/dev/null").then(|| target.strip_prefix("b/").unwrap_or(target).to_string());
            } else if line.starts_with("@@ ") {
                in_header = false;
                if let (Some(p), Some((start, len))) = (&path, parse_new_range(line))
                    && len > 0
                {
                    map.hunks.entry(p.clone()).or_default().push((start, start + len - 1));
                }
            }
        }
        map
    }

    pub fn contains(&self, path: &str, start: u32, end: u32) -> bool {
        self.hunks.get(path).is_some_and(|hunks| hunks.iter().any(|&(s, e)| s <= start && end <= e))
    }
}

/// `+c,d` from a hunk header like `@@ -12,7 +12,8 @@ fn foo()`; `d` defaults to 1.
fn parse_new_range(header: &str) -> Option<(u32, u32)> {
    let spec = header.split_whitespace().find(|t| t.starts_with('+'))?.strip_prefix('+')?;
    match spec.split_once(',') {
        Some((s, l)) => Some((s.parse().ok()?, l.parse().ok()?)),
        None => Some((spec.parse().ok()?, 1)),
    }
}

fn unquote(s: &str) -> &str {
    s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(s)
}
```

- [ ] **Step 5: Run tests and clippy**

Run: `cargo test --lib git && cargo clippy --all-targets -- -D warnings`
Expected: 3 passed, no warnings.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: add git access and diff hunk map"
```

---

### Task 3: Session model, Verdicts and Store

**Files:**
- Create: `src/session.rs`
- Modify: `src/lib.rs` (add `pub mod session;`), `src/testutil.rs` (add `finding`)

**Interfaces:**
- Consumes: `model::{Finding, Location, Severity, ConfidenceLevel}`.
- Produces (all in `prr::session`):
  - `REVIEWER_ID_PREFIX: &str = "reviewer-"`
  - `enum Target { Local { base_ref: String, head_ref: String } }` (serde tag `kind`), `Target::label(&self) -> String` → `"main...feature"`
  - `enum SessionStatus { Open, Submitted, Abandoned }`, `enum Origin { Agent, Reviewer }`, `enum ReviewState { RequestChanges, Comment, Approve }` with `label(self) -> &'static str` (`"Request changes" | "Comment" | "Approve"`) — all serde `snake_case`
  - `enum Proposal { Agent(Finding), Reviewer { location: Location, comment: String } }`
  - `enum Verdict { Block, Comment, Nit, Dismiss { reason: String }, Escalate { to: String } }` with `from_form(verdict: &str, reason: &str, escalate_to: &str) -> Result<Verdict, String>`, `key(&self) -> &'static str` (`"block"`…), `label(&self) -> &'static str` (`"Block"`…)
  - `is_valid_handle(s: &str) -> bool`
  - `struct Ruling { verdict: Verdict, comment: String }`
  - `struct Entry { id, proposal, location_text: Option<String>, excerpts: BTreeMap<usize, String>, ruling: Option<Ruling> }` with `finding() -> Option<&Finding>`, `origin() -> Origin`, `location() -> &Location`, `title() -> Option<&str>`, `display_title() -> String`, `draft_comment() -> &str`, `severity() -> Option<Severity>`
  - `struct Outcome { state: Option<ReviewState>, summary: String, export_path: Option<PathBuf> }`
  - `enum SessionError { NotOpen, NoSuchFinding(usize), Invalid(String) }` (thiserror)
  - `struct Session { id, token, repo_root: PathBuf, target, base_sha, head_sha, entries: Vec<Entry>, status, export_md: Option<PathBuf>, outcome: Option<Outcome> }` with `new(repo_root, target, base_sha, head_sha, entries, export_md) -> Session` (sorts entries), `is_open()`, `same_change(&Session) -> bool`, `unruled() -> Vec<usize>`, `next_unruled_after(idx) -> Option<usize>`, `rule(idx, Ruling) -> Result<(), SessionError>`, `add_reviewer_finding(location, comment, location_text) -> Result<usize, SessionError>`
  - `struct Store` (`Clone`): `new(dir: PathBuf)`, `from_env() -> anyhow::Result<Store>`, `save(&Session)`, `load(id) -> Result<Session>`, `list() -> Result<Vec<Session>>`, `find_open(like: &Session) -> Result<Option<Session>>`, `try_lock(id) -> Result<Option<File>>`
  - `testutil::finding(id: &str, severity: Severity, level: ConfidenceLevel) -> Finding` (PR-wide, all text filled)

- [ ] **Step 1: Add the fixture** — append to `src/testutil.rs`:

```rust
use crate::model::{Confidence, ConfidenceLevel, Finding, Location, Severity};

/// A minimal valid PR-wide Finding.
pub fn finding(id: &str, severity: Severity, level: ConfidenceLevel) -> Finding {
    Finding {
        id: id.into(),
        title: format!("Title {id}"),
        summary: "summary".into(),
        location: Location::PrWide,
        severity,
        confidence: Confidence { level, rationale: "because".into() },
        impact: "impact".into(),
        recommendation: "recommendation".into(),
        suggested_patch: None,
        evidence: vec![],
        draft_comment: format!("Comment on {id}"),
        escalate_to: None,
    }
}
```

- [ ] **Step 2: Write the failing tests** — `src/session.rs` with tests only:

```rust
//! A Review session: the Findings under Adjudication and the Reviewer's Verdicts.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ConfidenceLevel as C, Severity as S};
    use crate::testutil::finding;

    fn entry(id: &str, s: S, c: C) -> Entry {
        Entry {
            id: id.into(),
            proposal: Proposal::Agent(finding(id, s, c)),
            location_text: None,
            excerpts: BTreeMap::new(),
            ruling: None,
        }
    }

    fn session(entries: Vec<Entry>) -> Session {
        Session::new(
            PathBuf::from("/repo"),
            Target::Local { base_ref: "main".into(), head_ref: "feature".into() },
            "b".repeat(40),
            "h".repeat(40),
            entries,
            None,
        )
    }

    fn ruling(verdict: Verdict) -> Ruling {
        Ruling { verdict, comment: "text".into() }
    }

    #[test]
    fn new_orders_by_severity_then_confidence_keeping_input_order() {
        let s = session(vec![
            entry("minor", S::Minor, C::High),
            entry("major-low", S::Major, C::Low),
            entry("major-high", S::Major, C::High),
            entry("major-high-2", S::Major, C::High),
            entry("critical", S::Critical, C::Low),
        ]);
        let ids: Vec<&str> = s.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["critical", "major-high", "major-high-2", "major-low", "minor"]);
        assert_eq!(s.id.len(), 32);
        assert_ne!(s.id, s.token);
    }

    #[test]
    fn next_unruled_wraps_around() {
        let mut s = session(vec![entry("a", S::Major, C::High), entry("b", S::Minor, C::High), entry("c", S::Info, C::High)]);
        s.rule(1, ruling(Verdict::Nit)).unwrap();
        assert_eq!(s.next_unruled_after(0), Some(2));
        assert_eq!(s.next_unruled_after(2), Some(0));
        assert_eq!(s.unruled(), vec![0, 2]);
        s.rule(0, ruling(Verdict::Block)).unwrap();
        s.rule(2, ruling(Verdict::Comment)).unwrap();
        assert_eq!(s.next_unruled_after(0), None);
    }

    #[test]
    fn rule_validates() {
        let mut s = session(vec![entry("a", S::Major, C::High)]);
        assert_eq!(s.rule(5, ruling(Verdict::Block)), Err(SessionError::NoSuchFinding(5)));
        let empty = Ruling { verdict: Verdict::Comment, comment: "  ".into() };
        assert!(matches!(s.rule(0, empty), Err(SessionError::Invalid(_))));
        let dismissed = Ruling { verdict: Verdict::Dismiss { reason: "false positive".into() }, comment: String::new() };
        assert_eq!(s.rule(0, dismissed), Ok(()));
        s.status = SessionStatus::Submitted;
        assert_eq!(s.rule(0, ruling(Verdict::Block)), Err(SessionError::NotOpen));
    }

    #[test]
    fn verdict_from_form() {
        assert_eq!(Verdict::from_form("block", "", ""), Ok(Verdict::Block));
        assert_eq!(Verdict::from_form("nit", "", ""), Ok(Verdict::Nit));
        assert!(Verdict::from_form("dismiss", " ", "").unwrap_err().contains("reason"));
        assert_eq!(Verdict::from_form("dismiss", " noise ", ""), Ok(Verdict::Dismiss { reason: "noise".into() }));
        assert!(Verdict::from_form("escalate", "", "bob").is_err());
        assert_eq!(Verdict::from_form("escalate", "", "@org/team"), Ok(Verdict::Escalate { to: "@org/team".into() }));
        assert!(Verdict::from_form("", "", "").unwrap_err().contains("Choose"));
        assert!(Verdict::from_form("approve", "", "").is_err());
    }

    #[test]
    fn handles() {
        assert!(is_valid_handle("@alice"));
        assert!(is_valid_handle("@my-org/core_team.v2"));
        assert!(!is_valid_handle("alice"));
        assert!(!is_valid_handle("@"));
        assert!(!is_valid_handle("@a/b/c"));
        assert!(!is_valid_handle("@a b"));
    }

    #[test]
    fn reviewer_findings_get_reserved_ids_and_need_a_comment() {
        let mut s = session(vec![entry("a", S::Major, C::High)]);
        assert!(matches!(s.add_reviewer_finding(Location::PrWide, " ".into(), None), Err(SessionError::Invalid(_))));
        let i = s.add_reviewer_finding(Location::PrWide, "Rename this\nplease".into(), None).unwrap();
        let j = s.add_reviewer_finding(Location::PrWide, "Another".into(), None).unwrap();
        assert_eq!((i, j), (1, 2));
        assert_eq!(s.entries[1].id, "reviewer-1");
        assert_eq!(s.entries[2].id, "reviewer-2");
        assert_eq!(s.entries[1].origin(), Origin::Reviewer);
        assert_eq!(s.entries[1].display_title(), "Reviewer: Rename this");
        assert_eq!(s.entries[1].draft_comment(), "Rename this\nplease");
    }

    #[test]
    fn store_round_trips_and_finds_open_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf());
        let mut s = session(vec![entry("a", S::Major, C::High)]);
        s.rule(0, ruling(Verdict::Escalate { to: "@bob".into() })).unwrap();
        store.save(&s).unwrap();
        assert_eq!(store.load(&s.id).unwrap(), s);
        assert_eq!(store.list().unwrap().len(), 1);

        let same = session(vec![]);
        assert_eq!(store.find_open(&same).unwrap().map(|f| f.id), Some(s.id.clone()));
        s.status = SessionStatus::Submitted;
        store.save(&s).unwrap();
        assert_eq!(store.find_open(&same).unwrap(), None);
    }

    #[test]
    fn store_rejects_path_like_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf());
        assert!(store.load("../etc/passwd").is_err());
        assert!(store.load("").is_err());
    }

    #[test]
    fn lock_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf());
        let first = store.try_lock("abc").unwrap();
        assert!(first.is_some());
        assert!(store.try_lock("abc").unwrap().is_none());
        drop(first);
        assert!(store.try_lock("abc").unwrap().is_some());
    }
}
```

Add `pub mod session;` to `src/lib.rs`.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test --lib session`
Expected: compile errors — types not found.

- [ ] **Step 4: Implement** — above the tests in `src/session.rs`:

```rust
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{Finding, Location, Severity};

pub const REVIEWER_ID_PREFIX: &str = "reviewer-";

/// What is being reviewed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// A local git range with no Code host.
    Local { base_ref: String, head_ref: String },
}

impl Target {
    pub fn label(&self) -> String {
        match self {
            Target::Local { base_ref, head_ref } => format!("{base_ref}...{head_ref}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Open,
    Submitted,
    Abandoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Agent,
    Reviewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    RequestChanges,
    Comment,
    Approve,
}

impl ReviewState {
    pub fn label(self) -> &'static str {
        match self {
            ReviewState::RequestChanges => "Request changes",
            ReviewState::Comment => "Comment",
            ReviewState::Approve => "Approve",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Proposal {
    Agent(Finding),
    Reviewer { location: Location, comment: String },
}

/// The Reviewer's ruling on a Finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    Block,
    Comment,
    Nit,
    Dismiss { reason: String },
    Escalate { to: String },
}

impl Verdict {
    pub fn from_form(verdict: &str, reason: &str, escalate_to: &str) -> Result<Verdict, String> {
        let (reason, to) = (reason.trim(), escalate_to.trim());
        match verdict {
            "block" => Ok(Verdict::Block),
            "comment" => Ok(Verdict::Comment),
            "nit" => Ok(Verdict::Nit),
            "dismiss" if reason.is_empty() => Err("Dismiss needs a reason.".into()),
            "dismiss" => Ok(Verdict::Dismiss { reason: reason.into() }),
            "escalate" if !is_valid_handle(to) => {
                Err("Escalate needs an @user or @org/team handle.".into())
            }
            "escalate" => Ok(Verdict::Escalate { to: to.into() }),
            "" => Err("Choose a verdict.".into()),
            other => Err(format!("Unknown verdict `{other}`.")),
        }
    }

    pub fn key(&self) -> &'static str {
        match self {
            Verdict::Block => "block",
            Verdict::Comment => "comment",
            Verdict::Nit => "nit",
            Verdict::Dismiss { .. } => "dismiss",
            Verdict::Escalate { .. } => "escalate",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Block => "Block",
            Verdict::Comment => "Comment",
            Verdict::Nit => "Nit",
            Verdict::Dismiss { .. } => "Dismiss",
            Verdict::Escalate { .. } => "Escalate",
        }
    }
}

/// `@user` or `@org/team`.
pub fn is_valid_handle(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('@') else { return false };
    let parts: Vec<&str> = rest.split('/').collect();
    parts.len() <= 2
        && parts.iter().all(|p| {
            !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ruling {
    pub verdict: Verdict,
    /// The text to post, as edited by the Reviewer.
    pub comment: String,
}

/// A Finding under Adjudication, with the text prr resolved for it and the Reviewer's ruling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub proposal: Proposal,
    /// Text of the location's lines on the head commit.
    #[serde(default)]
    pub location_text: Option<String>,
    /// Resolved text of each code excerpt, keyed by evidence index.
    #[serde(default)]
    pub excerpts: BTreeMap<usize, String>,
    #[serde(default)]
    pub ruling: Option<Ruling>,
}

impl Entry {
    pub fn finding(&self) -> Option<&Finding> {
        match &self.proposal {
            Proposal::Agent(f) => Some(f),
            Proposal::Reviewer { .. } => None,
        }
    }

    pub fn origin(&self) -> Origin {
        match &self.proposal {
            Proposal::Agent(_) => Origin::Agent,
            Proposal::Reviewer { .. } => Origin::Reviewer,
        }
    }

    pub fn location(&self) -> &Location {
        match &self.proposal {
            Proposal::Agent(f) => &f.location,
            Proposal::Reviewer { location, .. } => location,
        }
    }

    pub fn title(&self) -> Option<&str> {
        self.finding().map(|f| f.title.as_str())
    }

    pub fn display_title(&self) -> String {
        match &self.proposal {
            Proposal::Agent(f) => f.title.clone(),
            Proposal::Reviewer { comment, .. } => {
                let first: String = comment.lines().next().unwrap_or("").chars().take(60).collect();
                format!("Reviewer: {first}")
            }
        }
    }

    pub fn draft_comment(&self) -> &str {
        match &self.proposal {
            Proposal::Agent(f) => &f.draft_comment,
            Proposal::Reviewer { comment, .. } => comment,
        }
    }

    pub fn severity(&self) -> Option<Severity> {
        self.finding().map(|f| f.severity)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub state: Option<ReviewState>,
    pub summary: String,
    pub export_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SessionError {
    #[error("This session is no longer open.")]
    NotOpen,
    #[error("There is no finding #{0}.")]
    NoSuchFinding(usize),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub token: String,
    pub repo_root: PathBuf,
    pub target: Target,
    pub base_sha: String,
    pub head_sha: String,
    pub entries: Vec<Entry>,
    pub status: SessionStatus,
    #[serde(default)]
    pub export_md: Option<PathBuf>,
    #[serde(default)]
    pub outcome: Option<Outcome>,
}

impl Session {
    pub fn new(
        repo_root: PathBuf,
        target: Target,
        base_sha: String,
        head_sha: String,
        mut entries: Vec<Entry>,
        export_md: Option<PathBuf>,
    ) -> Session {
        // Stable sort: most severe first, then most confident; Reviewer findings (no severity) last.
        entries.sort_by_key(|e| {
            let f = e.finding();
            (Reverse(f.map(|f| f.severity)), Reverse(f.map(|f| f.confidence.level)))
        });
        Session {
            id: uuid::Uuid::new_v4().simple().to_string(),
            token: uuid::Uuid::new_v4().simple().to_string(),
            repo_root,
            target,
            base_sha,
            head_sha,
            entries,
            status: SessionStatus::Open,
            export_md,
            outcome: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.status == SessionStatus::Open
    }

    /// Whether `other` reviews the same change (repo, target and head commit).
    pub fn same_change(&self, other: &Session) -> bool {
        self.repo_root == other.repo_root && self.target == other.target && self.head_sha == other.head_sha
    }

    pub fn unruled(&self) -> Vec<usize> {
        (0..self.entries.len()).filter(|&i| self.entries[i].ruling.is_none()).collect()
    }

    pub fn next_unruled_after(&self, idx: usize) -> Option<usize> {
        let n = self.entries.len();
        (idx + 1..n).chain(0..idx.min(n)).find(|&i| self.entries[i].ruling.is_none())
    }

    pub fn rule(&mut self, idx: usize, ruling: Ruling) -> Result<(), SessionError> {
        if !self.is_open() {
            return Err(SessionError::NotOpen);
        }
        let entry = self.entries.get_mut(idx).ok_or(SessionError::NoSuchFinding(idx))?;
        if ruling.comment.trim().is_empty() && !matches!(ruling.verdict, Verdict::Dismiss { .. }) {
            return Err(SessionError::Invalid("The comment to post must not be empty.".into()));
        }
        entry.ruling = Some(ruling);
        Ok(())
    }

    pub fn add_reviewer_finding(
        &mut self,
        location: Location,
        comment: String,
        location_text: Option<String>,
    ) -> Result<usize, SessionError> {
        if !self.is_open() {
            return Err(SessionError::NotOpen);
        }
        if comment.trim().is_empty() {
            return Err(SessionError::Invalid("Write the comment for your finding.".into()));
        }
        let n = self.entries.iter().filter(|e| e.origin() == Origin::Reviewer).count() + 1;
        self.entries.push(Entry {
            id: format!("{REVIEWER_ID_PREFIX}{n}"),
            proposal: Proposal::Reviewer { location, comment: comment.trim().into() },
            location_text,
            excerpts: BTreeMap::new(),
            ruling: None,
        });
        Ok(self.entries.len() - 1)
    }
}

/// Sessions on disk, one JSON file each.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Store {
        Store { dir }
    }

    pub fn from_env() -> Result<Store> {
        if let Some(dir) = std::env::var_os("PRR_STATE_DIR") {
            return Ok(Store::new(dir.into()));
        }
        let base = dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .context("no state directory found; set PRR_STATE_DIR")?;
        Ok(Store::new(base.join("prr")))
    }

    fn sessions_dir(&self) -> PathBuf {
        self.dir.join("sessions")
    }

    fn file(&self, id: &str, ext: &str) -> Result<PathBuf> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            bail!("no session `{id}`");
        }
        Ok(self.sessions_dir().join(format!("{id}.{ext}")))
    }

    /// Writes atomically: a crash never leaves a half-written session.
    pub fn save(&self, session: &Session) -> Result<()> {
        fs::create_dir_all(self.sessions_dir())?;
        let path = self.file(&session.id, "json")?;
        let tmp = self.file(&session.id, "json.tmp")?;
        fs::write(&tmp, serde_json::to_vec_pretty(session)?)?;
        fs::rename(&tmp, &path).with_context(|| format!("saving {}", path.display()))?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<Session> {
        let path = self.file(id, "json")?;
        let bytes = fs::read(&path).with_context(|| format!("no session `{id}`"))?;
        serde_json::from_slice(&bytes).with_context(|| format!("{} is corrupt", path.display()))
    }

    pub fn list(&self) -> Result<Vec<Session>> {
        let Ok(dir) = fs::read_dir(self.sessions_dir()) else { return Ok(vec![]) };
        let mut out = Vec::new();
        for entry in dir {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let parsed = fs::read(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|b| serde_json::from_slice::<Session>(&b).map_err(Into::into));
                match parsed {
                    Ok(s) => out.push(s),
                    Err(e) => eprintln!("prr: skipping {}: {e:#}", path.display()),
                }
            }
        }
        Ok(out)
    }

    pub fn find_open(&self, like: &Session) -> Result<Option<Session>> {
        Ok(self.list()?.into_iter().find(|s| s.is_open() && s.same_change(like)))
    }

    /// An exclusive lock marking the session as served by this process; `None` if another process holds it.
    /// The lock is released when the returned file is dropped or the process exits.
    pub fn try_lock(&self, id: &str) -> Result<Option<File>> {
        fs::create_dir_all(self.sessions_dir())?;
        let file = OpenOptions::new().create(true).truncate(false).write(true).open(self.file(id, "lock")?)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }
}
```

- [ ] **Step 5: Run tests and clippy**

Run: `cargo test --lib session && cargo clippy --all-targets -- -D warnings`
Expected: 9 passed, no warnings.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: add review session model, verdicts and on-disk store"
```

---

### Task 4: Validation and excerpt resolution

**Files:**
- Create: `src/validate.rs`
- Modify: `src/lib.rs` (add `pub mod validate;`), `src/testutil.rs` (add `sample_batch`)

**Interfaces:**
- Consumes: `git::Repo`, `model::*`, `session::{Entry, Proposal, REVIEWER_ID_PREFIX, is_valid_handle}`.
- Produces (in `prr::validate`):
  - `MAX_EXCERPT_LINES: u32 = 400`
  - `struct ValidationError { path: String, message: String }` (`Serialize`; `path` is a JSON pointer such as `/findings/0/location/path`)
  - `parse_batch(json: &str) -> Result<FindingBatch, Vec<ValidationError>>`
  - `validate(batch: FindingBatch, repo: &Repo, head_sha: &str) -> Result<Vec<Entry>, Vec<ValidationError>>` (entries in input order; `Session::new` sorts)
  - `struct LineError { field: &'static str, message: String }` (`field` is `""`, `"/path"` or `"/end"`)
  - `resolve_lines(repo: &Repo, sha: &str, path: &str, start: u32, end: u32) -> Result<String, LineError>` (lines joined with `\n`)
  - `check_repo_path(path: &str) -> Result<(), String>`
  - `testutil::sample_batch() -> FindingBatch` — two findings in this input order: `"minor"` (Minor/Medium at `src/lib.rs` 1–2, no evidence) and `"major"` (Major/High, title `Retry loop never backs off`, at `src/lib.rs` 10–12, `suggested_patch` set, evidence: code excerpt 10–12, Mermaid diagram, command output containing `<script>alert(1)</script>`).

- [ ] **Step 1: Add the fixture** — append to `src/testutil.rs`:

```rust
use crate::model::{Evidence, FindingBatch};

/// Two findings against `sample_repo()`: a Minor one outside the diff and a Major one inside it.
pub fn sample_batch() -> FindingBatch {
    let mut minor = finding("minor", Severity::Minor, ConfidenceLevel::Medium);
    minor.location = Location::Lines { path: "src/lib.rs".into(), start: 1, end: 2 };
    let mut major = finding("major", Severity::Major, ConfidenceLevel::High);
    major.title = "Retry loop never backs off".into();
    major.location = Location::Lines { path: "src/lib.rs".into(), start: 10, end: 12 };
    major.suggested_patch = Some("fixed 10\nfixed 11\nfixed 12".into());
    major.evidence = vec![
        Evidence::CodeExcerpt { path: "src/lib.rs".into(), start: 10, end: 12, git_ref: None, caption: Some("the loop".into()) },
        Evidence::Diagram { mermaid: "graph TD; A-->B".into(), caption: None },
        Evidence::CommandOutput { command: "cargo test".into(), output: "<script>alert(1)</script>".into(), exit_code: Some(101) },
    ];
    FindingBatch { findings: vec![minor, major] }
}
```

- [ ] **Step 2: Write the failing tests** — `src/validate.rs`:

```rust
//! Checks an Agent's findings file against the repository before any Reviewer sees it.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ConfidenceLevel, Severity};
    use crate::testutil::{finding, sample_batch, sample_repo};

    fn errors_for(batch: FindingBatch) -> Vec<ValidationError> {
        let (t, _, head) = sample_repo();
        validate(batch, &t.repo(), &head).unwrap_err()
    }

    fn lines(path: &str, start: u32, end: u32) -> Location {
        Location::Lines { path: path.into(), start, end }
    }

    fn one(f: Finding) -> FindingBatch {
        FindingBatch { findings: vec![f] }
    }

    fn base() -> Finding {
        finding("F1", Severity::Major, ConfidenceLevel::High)
    }

    #[test]
    fn valid_batch_resolves_location_and_excerpts_from_git() {
        let (t, _, head) = sample_repo();
        let entries = validate(sample_batch(), &t.repo(), &head).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "minor");
        assert_eq!(entries[0].location_text.as_deref(), Some("line 1\nline 2"));
        assert_eq!(entries[1].location_text.as_deref(), Some("changed 10\nchanged 11\nchanged 12"));
        assert_eq!(entries[1].excerpts.get(&0).map(String::as_str), Some("changed 10\nchanged 11\nchanged 12"));
        assert_eq!(entries[1].excerpts.len(), 1, "only code excerpts are resolved");
    }

    #[test]
    fn excerpt_ref_reads_another_commit() {
        let (t, _, head) = sample_repo();
        let mut f = base();
        f.evidence = vec![Evidence::CodeExcerpt { path: "src/lib.rs".into(), start: 10, end: 10, git_ref: Some("main".into()), caption: None }];
        let entries = validate(one(f), &t.repo(), &head).unwrap();
        assert_eq!(entries[0].excerpts[&0], "line 10");
    }

    #[test]
    fn empty_batch_is_valid() {
        let (t, _, head) = sample_repo();
        assert_eq!(validate(FindingBatch { findings: vec![] }, &t.repo(), &head).unwrap(), vec![]);
    }

    #[test]
    fn malformed_json_is_reported() {
        let errs = parse_batch("{\"findings\": [").unwrap_err();
        assert!(errs[0].message.contains("invalid findings JSON"), "{errs:?}");
    }

    #[test]
    fn ids_must_be_unique_non_empty_and_unreserved() {
        let errs = errors_for(FindingBatch { findings: vec![base(), base()] });
        assert_eq!(errs[0].path, "/findings/1/id");
        assert!(errs[0].message.contains("duplicate"));

        let mut f = base();
        f.id = "reviewer-1".into();
        assert!(errors_for(one(f))[0].message.contains("reserved"));
    }

    #[test]
    fn required_text_must_not_be_blank() {
        let mut f = base();
        f.title = " ".into();
        f.confidence.rationale = String::new();
        let paths: Vec<String> = errors_for(one(f)).into_iter().map(|e| e.path).collect();
        assert_eq!(paths, ["/findings/0/title", "/findings/0/confidence/rationale"]);
    }

    #[test]
    fn rejects_non_repo_relative_paths() {
        for bad in ["/abs/src/lib.rs", "./src/lib.rs", "src\\lib.rs", "../src/lib.rs", "src//lib.rs"] {
            let mut f = base();
            f.location = lines(bad, 1, 1);
            let errs = errors_for(one(f));
            assert_eq!(errs[0].path, "/findings/0/location/path", "{bad}");
            assert!(errs[0].message.contains("repo-relative"), "{bad}: {}", errs[0].message);
        }
    }

    #[test]
    fn rejects_bad_locations() {
        let mut f = base();
        f.location = lines("src/missing.rs", 1, 1);
        assert!(errors_for(one(f))[0].message.contains("does not exist"));

        let mut f = base();
        f.location = lines("src/lib.rs", 19, 25);
        let e = &errors_for(one(f))[0];
        assert_eq!(e.path, "/findings/0/location/end");
        assert!(e.message.contains("only 20 lines"));

        let mut f = base();
        f.location = lines("src/lib.rs", 0, 1);
        assert!(errors_for(one(f))[0].message.contains("1-based"));

        let mut f = base();
        f.location = lines("src/lib.rs", 1, 401);
        assert!(errors_for(one(f))[0].message.contains("400"));
    }

    #[test]
    fn rejects_bad_evidence_and_extras() {
        let mut f = base();
        f.evidence = vec![
            Evidence::CodeExcerpt { path: "src/lib.rs".into(), start: 1, end: 1, git_ref: Some("nope".into()), caption: None },
            Evidence::Diagram { mermaid: " ".into(), caption: None },
        ];
        f.suggested_patch = Some("x".into());
        f.escalate_to = Some("bob".into());
        let paths: Vec<String> = errors_for(one(f)).into_iter().map(|e| e.path).collect();
        assert_eq!(
            paths,
            [
                "/findings/0/escalate_to",
                "/findings/0/evidence/1/mermaid",
                "/findings/0/suggested_patch",
                "/findings/0/evidence/0/ref",
            ]
        );
    }

    #[test]
    fn collects_errors_across_findings() {
        let mut a = base();
        a.title = String::new();
        let mut b = finding("F2", Severity::Minor, ConfidenceLevel::Low);
        b.location = lines("nope.rs", 1, 1);
        let errs = errors_for(FindingBatch { findings: vec![a, b] });
        assert_eq!(errs.len(), 2);
        assert!(errs[1].path.starts_with("/findings/1/"));
    }
}
```

Add `pub mod validate;` to `src/lib.rs`.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test --lib validate`
Expected: compile errors — `validate`, `ValidationError` not found.

- [ ] **Step 4: Implement** — above the tests in `src/validate.rs`:

```rust
use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

use crate::git::Repo;
use crate::model::{Evidence, Finding, FindingBatch, Location};
use crate::session::{is_valid_handle, Entry, Proposal, REVIEWER_ID_PREFIX};

pub const MAX_EXCERPT_LINES: u32 = 400;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValidationError {
    /// JSON pointer into the findings file, e.g. `/findings/0/location/path`.
    pub path: String,
    pub message: String,
}

fn err(path: String, message: impl Into<String>) -> ValidationError {
    ValidationError { path, message: message.into() }
}

pub fn parse_batch(json: &str) -> Result<FindingBatch, Vec<ValidationError>> {
    serde_json::from_str(json).map_err(|e| vec![err(String::new(), format!("invalid findings JSON: {e}"))])
}

/// Checks every Finding and resolves its location and code excerpts from git. Reports all errors at once.
pub fn validate(batch: FindingBatch, repo: &Repo, head_sha: &str) -> Result<Vec<Entry>, Vec<ValidationError>> {
    let mut errors = Vec::new();
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for (i, finding) in batch.findings.into_iter().enumerate() {
        let p = format!("/findings/{i}");
        let before = errors.len();
        check_fields(&finding, &p, &mut seen, &mut errors);

        let location_text = match &finding.location {
            Location::Lines { path, start, end } => {
                resolve_or_report(repo, head_sha, path, *start, *end, &format!("{p}/location"), &mut errors)
            }
            Location::PrWide => {
                if finding.suggested_patch.is_some() {
                    errors.push(err(format!("{p}/suggested_patch"), "a suggested_patch needs a `lines` location"));
                }
                None
            }
        };

        let mut excerpts = BTreeMap::new();
        for (j, ev) in finding.evidence.iter().enumerate() {
            let Evidence::CodeExcerpt { path, start, end, git_ref, .. } = ev else { continue };
            let ep = format!("{p}/evidence/{j}");
            let sha = match git_ref {
                None => head_sha.to_string(),
                Some(r) => match repo.rev_parse(r) {
                    Ok(sha) => sha,
                    Err(_) => {
                        errors.push(err(format!("{ep}/ref"), format!("unknown git ref `{r}`")));
                        continue;
                    }
                },
            };
            if let Some(text) = resolve_or_report(repo, &sha, path, *start, *end, &ep, &mut errors) {
                excerpts.insert(j, text);
            }
        }

        if errors.len() == before {
            entries.push(Entry {
                id: finding.id.clone(),
                proposal: Proposal::Agent(finding),
                location_text,
                excerpts,
                ruling: None,
            });
        }
    }
    if errors.is_empty() { Ok(entries) } else { Err(errors) }
}

fn check_fields(f: &Finding, p: &str, seen: &mut HashSet<String>, errors: &mut Vec<ValidationError>) {
    if f.id.trim().is_empty() {
        errors.push(err(format!("{p}/id"), "must not be empty"));
    } else if f.id.starts_with(REVIEWER_ID_PREFIX) {
        errors.push(err(format!("{p}/id"), format!("ids starting with `{REVIEWER_ID_PREFIX}` are reserved")));
    } else if !seen.insert(f.id.clone()) {
        errors.push(err(format!("{p}/id"), format!("duplicate id `{}`", f.id)));
    }
    let required = [
        ("title", &f.title),
        ("draft_comment", &f.draft_comment),
        ("confidence/rationale", &f.confidence.rationale),
    ];
    for (field, value) in required {
        if value.trim().is_empty() {
            errors.push(err(format!("{p}/{field}"), "must not be empty"));
        }
    }
    if let Some(to) = &f.escalate_to
        && !is_valid_handle(to)
    {
        errors.push(err(format!("{p}/escalate_to"), "must be an @user or @org/team handle"));
    }
    for (j, ev) in f.evidence.iter().enumerate() {
        if let Evidence::Diagram { mermaid, .. } = ev
            && mermaid.trim().is_empty()
        {
            errors.push(err(format!("{p}/evidence/{j}/mermaid"), "must not be empty"));
        }
    }
}

fn resolve_or_report(
    repo: &Repo,
    sha: &str,
    path: &str,
    start: u32,
    end: u32,
    at: &str,
    errors: &mut Vec<ValidationError>,
) -> Option<String> {
    match resolve_lines(repo, sha, path, start, end) {
        Ok(text) => Some(text),
        Err(e) => {
            errors.push(err(format!("{at}{}", e.field), e.message));
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LineError {
    /// Suffix of the JSON pointer: `""`, `"/path"` or `"/end"`.
    pub field: &'static str,
    pub message: String,
}

/// Text of lines `start..=end` of `path` at `sha`, joined with `\n`.
pub fn resolve_lines(repo: &Repo, sha: &str, path: &str, start: u32, end: u32) -> Result<String, LineError> {
    let fail = |field, message: String| LineError { field, message };
    check_repo_path(path).map_err(|m| fail("/path", m))?;
    if start == 0 || end < start {
        return Err(fail("", format!("invalid line range {start}-{end}; lines are 1-based and start <= end")));
    }
    if end - start + 1 > MAX_EXCERPT_LINES {
        return Err(fail("", format!("ranges are limited to {MAX_EXCERPT_LINES} lines; narrow it")));
    }
    let short = &sha[..sha.len().min(12)];
    let bytes = match repo.show_file(sha, path) {
        Ok(Some(b)) => b,
        Ok(None) => return Err(fail("/path", format!("`{path}` does not exist at {short}"))),
        Err(e) => return Err(fail("/path", format!("{e:#}"))),
    };
    let text = String::from_utf8(bytes).map_err(|_| fail("/path", format!("`{path}` is not UTF-8 text")))?;
    let lines: Vec<&str> = text.lines().collect();
    if end as usize > lines.len() {
        return Err(fail("/end", format!("`{path}` has only {} lines at {short}", lines.len())));
    }
    Ok(lines[start as usize - 1..end as usize].join("\n"))
}

pub fn check_repo_path(path: &str) -> Result<(), String> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.split('/').any(|s| s.is_empty() || s == "." || s == "..");
    if bad {
        return Err(format!("`{path}` must be a repo-relative path with forward slashes, e.g. src/main.rs"));
    }
    Ok(())
}
```

Note the test `rejects_bad_evidence_and_extras` expects field checks first (escalate_to, mermaid), then the location check (suggested_patch on PR-wide), then excerpts (ref) — matching the order above.

- [ ] **Step 5: Run tests and clippy**

Run: `cargo test --lib validate && cargo clippy --all-targets -- -D warnings`
Expected: 10 passed, no warnings.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: validate findings and resolve code excerpts from git"
```

---

### Task 5: Review composition and the result contract

**Files:**
- Create: `src/compose.rs`, `src/result.rs`
- Modify: `src/lib.rs` (add `pub mod compose; pub mod result;`)

**Interfaces:**
- Consumes: `git::DiffMap`, `model::{Finding, Location}`, `session::{Entry, ReviewState, Session, SessionStatus, Origin, Verdict, Ruling}`.
- Produces:
  - `prr::compose::SubmitChoice { approve: bool, summary: String }` (`Default`)
  - `prr::compose::InlineComment { entry_id: String, path: String, start_line: u32, line: u32, body: String }`
  - `prr::compose::ComposedReview { state: Option<ReviewState>, body: String, inline: Vec<InlineComment> }`
  - `prr::compose::ComposeError { Unruled(Vec<String>), ApproveWithBlock }` (thiserror; Unruled message contains `still need a Verdict`; ApproveWithBlock message contains `Approve`)
  - `review_state(entries: &[Entry], choice: &SubmitChoice) -> Result<Option<ReviewState>, ComposeError>`
  - `compose(entries: &[Entry], diff: &DiffMap, choice: &SubmitChoice, cite: &dyn Fn(&str, u32, u32) -> String) -> Result<ComposedReview, ComposeError>`
  - `to_markdown(review: &ComposedReview) -> String` (starts `# Review: <state label | Nothing to post>`)
  - `cite_plain(path: &str, start: u32, end: u32) -> String` → `` `src/lib.rs:1-2` `` or `` `src/lib.rs:3` ``
  - `prr::result::{ReviewResult, ResultFinding}`; `ReviewResult::from_session(&Session) -> ReviewResult`

- [ ] **Step 1: Write the failing tests** — `src/compose.rs`:

```rust
//! Turns Verdicts into the Review: its state, body and inline comments.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ConfidenceLevel, Severity};
    use crate::session::{Proposal, Ruling};
    use crate::testutil::finding;
    use std::collections::BTreeMap;

    fn ruled(id: &str, location: Location, verdict: Verdict, comment: &str) -> Entry {
        let mut f = finding(id, Severity::Major, ConfidenceLevel::High);
        f.location = location;
        Entry {
            id: id.into(),
            proposal: Proposal::Agent(f),
            location_text: None,
            excerpts: BTreeMap::new(),
            ruling: Some(Ruling { verdict, comment: comment.into() }),
        }
    }

    fn at(start: u32, end: u32) -> Location {
        Location::Lines { path: "src/lib.rs".into(), start, end }
    }

    fn diff() -> DiffMap {
        DiffMap::parse("diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -7,9 +7,9 @@\n")
    }

    fn choice(approve: bool, summary: &str) -> SubmitChoice {
        SubmitChoice { approve, summary: summary.into() }
    }

    fn dismiss() -> Verdict {
        Verdict::Dismiss { reason: "noise".into() }
    }

    #[test]
    fn state_rules() {
        let block = ruled("a", at(10, 10), Verdict::Block, "x");
        let nit = ruled("b", at(10, 10), Verdict::Nit, "x");
        let gone = ruled("c", at(10, 10), dismiss(), "");
        assert_eq!(review_state(&[block.clone(), nit.clone()], &choice(false, "")), Ok(Some(ReviewState::RequestChanges)));
        assert_eq!(review_state(&[block], &choice(true, "")), Err(ComposeError::ApproveWithBlock));
        assert_eq!(review_state(&[nit.clone()], &choice(false, "")), Ok(Some(ReviewState::Comment)));
        assert_eq!(review_state(&[nit], &choice(true, "")), Ok(Some(ReviewState::Approve)));
        assert_eq!(review_state(&[gone.clone()], &choice(false, "")), Ok(None));
        assert_eq!(review_state(&[gone.clone()], &choice(false, "LGTM")), Ok(Some(ReviewState::Comment)));
        assert_eq!(review_state(&[gone], &choice(true, "")), Ok(Some(ReviewState::Approve)));
        assert_eq!(review_state(&[], &choice(false, "")), Ok(None));
    }

    #[test]
    fn unruled_findings_block_composition() {
        let mut e = ruled("a", at(10, 10), Verdict::Nit, "x");
        e.ruling = None;
        let err = review_state(&[e], &choice(false, "")).unwrap_err();
        assert_eq!(err, ComposeError::Unruled(vec!["a".into()]));
        assert!(err.to_string().contains("still need a Verdict"));
    }

    #[test]
    fn findings_inside_a_hunk_go_inline_with_suggestions() {
        let mut e = ruled("in", at(10, 12), Verdict::Block, "Please back off");
        if let Proposal::Agent(f) = &mut e.proposal {
            f.suggested_patch = Some("sleep(b);".into());
        }
        let r = compose(&[e], &diff(), &choice(false, ""), &cite_plain).unwrap();
        assert_eq!(r.state, Some(ReviewState::RequestChanges));
        assert_eq!(r.body, "");
        assert_eq!(r.inline.len(), 1);
        let c = &r.inline[0];
        assert_eq!((c.path.as_str(), c.start_line, c.line), ("src/lib.rs", 10, 12));
        assert_eq!(c.body, "**Blocking:** Please back off\n\n```suggestion\nsleep(b);\n```");
    }

    #[test]
    fn other_findings_go_to_the_body_grouped_by_verdict() {
        let entries = [
            ruled("nit", at(1, 2), Verdict::Nit, "Tidy this"),
            ruled("partial", at(14, 16), Verdict::Comment, "Spans past the hunk"),
            ruled("wide", Location::PrWide, Verdict::Escalate { to: "@sec".into() }, "Auth model?"),
            ruled("blocker", at(1, 1), Verdict::Block, "Must fix\nsecond line"),
            ruled("gone", at(1, 1), dismiss(), ""),
        ];
        let r = compose(&entries, &diff(), &choice(false, "Thanks!"), &cite_plain).unwrap();
        assert!(r.inline.is_empty());
        assert_eq!(
            r.body,
            "Thanks!\n\n\
### Blocking\n\n- **Title blocker** — `src/lib.rs:1`\n  **Blocking:** Must fix\n  second line\n\n\
### Needs input\n\n- **Title wide** — PR-wide\n  **Needs input from @sec:** Auth model?\n\n\
### Comments\n\n- **Title partial** — `src/lib.rs:14-16`\n  Spans past the hunk\n\n\
### Nits\n\n- **Title nit** — `src/lib.rs:1-2`\n  **Nit (optional):** Tidy this"
        );
    }

    #[test]
    fn reviewer_findings_have_no_title() {
        let e = Entry {
            id: "reviewer-1".into(),
            proposal: Proposal::Reviewer { location: Location::PrWide, comment: "Mine".into() },
            location_text: None,
            excerpts: BTreeMap::new(),
            ruling: Some(Ruling { verdict: Verdict::Comment, comment: "Mine".into() }),
        };
        let r = compose(&[e], &diff(), &choice(false, ""), &cite_plain).unwrap();
        assert_eq!(r.body, "### Comments\n\n- PR-wide\n  Mine");
    }

    #[test]
    fn markdown_export() {
        let entries = [ruled("in", at(10, 10), Verdict::Comment, "Inline one"), ruled("out", at(1, 1), Verdict::Nit, "Body one")];
        let md = to_markdown(&compose(&entries, &diff(), &choice(false, ""), &cite_plain).unwrap());
        assert!(md.starts_with("# Review: Comment\n"), "{md}");
        assert!(md.contains("### Nits"));
        assert!(md.contains("## Inline comments\n\n### `src/lib.rs:10`\n\nInline one\n"), "{md}");
        assert_eq!(to_markdown(&ComposedReview { state: None, body: String::new(), inline: vec![] }), "# Review: Nothing to post\n");
    }
}
```

`src/result.rs`:

```rust
//! The JSON printed on stdout when a Review session ends — the Agent's view of the Reviewer's decisions.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ConfidenceLevel, Severity};
    use crate::session::{Entry, Outcome, Proposal, ReviewState, Ruling, Target, Verdict};
    use crate::testutil::finding;
    use std::collections::BTreeMap;

    #[test]
    fn result_flattens_verdicts() {
        let entry = |id: &str, verdict: Option<Verdict>| Entry {
            id: id.into(),
            proposal: Proposal::Agent(finding(id, Severity::Major, ConfidenceLevel::High)),
            location_text: None,
            excerpts: BTreeMap::new(),
            ruling: verdict.map(|verdict| Ruling { verdict, comment: format!("c-{id}") }),
        };
        let mut s = Session::new(
            "/r".into(),
            Target::Local { base_ref: "main".into(), head_ref: "HEAD".into() },
            "b".into(),
            "h".into(),
            vec![
                entry("d", Some(Verdict::Dismiss { reason: "dup".into() })),
                entry("e", Some(Verdict::Escalate { to: "@ops".into() })),
                entry("u", None),
            ],
            None,
        );
        s.status = SessionStatus::Submitted;
        s.outcome = Some(Outcome { state: Some(ReviewState::Comment), summary: "hi".into(), export_path: None });

        let v = serde_json::to_value(ReviewResult::from_session(&s)).unwrap();
        assert_eq!(v["status"], "submitted");
        assert_eq!(v["review_state"], "comment");
        assert_eq!(v["summary"], "hi");
        assert_eq!(v["posted"], false);
        assert_eq!(v["findings"][0]["verdict"], "dismiss");
        assert_eq!(v["findings"][0]["dismiss_reason"], "dup");
        assert_eq!(v["findings"][1]["escalate_to"], "@ops");
        assert_eq!(v["findings"][1]["comment"], "c-e");
        assert_eq!(v["findings"][1]["origin"], "agent");
        assert_eq!(v["findings"][2]["verdict"], serde_json::Value::Null);
        assert_eq!(v["findings"][2]["title"], "Title u");
    }
}
```

Add `pub mod compose;` and `pub mod result;` to `src/lib.rs`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib compose result`
Expected: compile errors — items not found. (`cargo test` takes one filter; run `cargo test --lib compose` and `cargo test --lib result` separately if needed.)

- [ ] **Step 3: Implement compose** — above the tests in `src/compose.rs`:

```rust
use crate::git::DiffMap;
use crate::model::Location;
use crate::session::{Entry, ReviewState, Verdict};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SubmitChoice {
    pub approve: bool,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InlineComment {
    pub entry_id: String,
    pub path: String,
    pub start_line: u32,
    pub line: u32,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComposedReview {
    /// `None` means there is nothing to post.
    pub state: Option<ReviewState>,
    pub body: String,
    pub inline: Vec<InlineComment>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ComposeError {
    #[error("{} finding(s) still need a Verdict: {}", .0.len(), .0.join(", "))]
    Unruled(Vec<String>),
    #[error("Approve is not available while a Finding is Blocked.")]
    ApproveWithBlock,
}

pub fn review_state(entries: &[Entry], choice: &SubmitChoice) -> Result<Option<ReviewState>, ComposeError> {
    let unruled: Vec<String> = entries.iter().filter(|e| e.ruling.is_none()).map(|e| e.id.clone()).collect();
    if !unruled.is_empty() {
        return Err(ComposeError::Unruled(unruled));
    }
    let verdicts = || entries.iter().filter_map(|e| e.ruling.as_ref()).map(|r| &r.verdict);
    if verdicts().any(|v| matches!(v, Verdict::Block)) {
        return if choice.approve { Err(ComposeError::ApproveWithBlock) } else { Ok(Some(ReviewState::RequestChanges)) };
    }
    if choice.approve {
        return Ok(Some(ReviewState::Approve));
    }
    let anything_raised = verdicts().any(|v| !matches!(v, Verdict::Dismiss { .. }));
    if anything_raised || !choice.summary.trim().is_empty() {
        Ok(Some(ReviewState::Comment))
    } else {
        Ok(None)
    }
}

const HEADINGS: [&str; 4] = ["### Blocking", "### Needs input", "### Comments", "### Nits"];

fn group(v: &Verdict) -> Option<usize> {
    match v {
        Verdict::Block => Some(0),
        Verdict::Escalate { .. } => Some(1),
        Verdict::Comment => Some(2),
        Verdict::Nit => Some(3),
        Verdict::Dismiss { .. } => None,
    }
}

fn format_comment(entry: &Entry, verdict: &Verdict, comment: &str, inline: bool) -> String {
    let mut text = match verdict {
        Verdict::Block => format!("**Blocking:** {comment}"),
        Verdict::Nit => format!("**Nit (optional):** {comment}"),
        Verdict::Escalate { to } => format!("**Needs input from {to}:** {comment}"),
        Verdict::Comment | Verdict::Dismiss { .. } => comment.to_string(),
    };
    if inline && let Some(patch) = entry.finding().and_then(|f| f.suggested_patch.as_ref()) {
        text.push_str(&format!("\n\n```suggestion\n{patch}\n```"));
    }
    text
}

pub fn compose(
    entries: &[Entry],
    diff: &DiffMap,
    choice: &SubmitChoice,
    cite: &dyn Fn(&str, u32, u32) -> String,
) -> Result<ComposedReview, ComposeError> {
    let state = review_state(entries, choice)?;
    let mut inline = Vec::new();
    let mut groups: [Vec<String>; 4] = Default::default();
    for entry in entries {
        let Some(ruling) = &entry.ruling else { continue };
        let Some(g) = group(&ruling.verdict) else { continue };
        match entry.location() {
            Location::Lines { path, start, end } if diff.contains(path, *start, *end) => {
                inline.push(InlineComment {
                    entry_id: entry.id.clone(),
                    path: path.clone(),
                    start_line: *start,
                    line: *end,
                    body: format_comment(entry, &ruling.verdict, &ruling.comment, true),
                });
            }
            location => {
                let place = match location {
                    Location::Lines { path, start, end } => cite(path, *start, *end),
                    Location::PrWide => "PR-wide".to_string(),
                };
                let head = match entry.title() {
                    Some(title) => format!("- **{title}** — {place}"),
                    None => format!("- {place}"),
                };
                let text = format_comment(entry, &ruling.verdict, &ruling.comment, false);
                let indented: Vec<String> = text.lines().map(|l| format!("  {l}")).collect();
                groups[g].push(format!("{head}\n{}", indented.join("\n")));
            }
        }
    }
    let mut parts = Vec::new();
    if !choice.summary.trim().is_empty() {
        parts.push(choice.summary.trim().to_string());
    }
    for (heading, items) in HEADINGS.iter().zip(&groups) {
        if !items.is_empty() {
            parts.push(format!("{heading}\n\n{}", items.join("\n\n")));
        }
    }
    Ok(ComposedReview { state, body: parts.join("\n\n"), inline })
}

pub fn cite_plain(path: &str, start: u32, end: u32) -> String {
    if start == end { format!("`{path}:{start}`") } else { format!("`{path}:{start}-{end}`") }
}

pub fn to_markdown(review: &ComposedReview) -> String {
    let state = review.state.map_or("Nothing to post", |s| s.label());
    let mut out = format!("# Review: {state}\n");
    if !review.body.is_empty() {
        out.push_str(&format!("\n{}\n", review.body));
    }
    if !review.inline.is_empty() {
        out.push_str("\n## Inline comments\n");
        for c in &review.inline {
            let at = if c.start_line == c.line {
                format!("{}:{}", c.path, c.line)
            } else {
                format!("{}:{}-{}", c.path, c.start_line, c.line)
            };
            out.push_str(&format!("\n### `{at}`\n\n{}\n", c.body));
        }
    }
    out
}
```

- [ ] **Step 4: Implement the result** — above the tests in `src/result.rs`:

```rust
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::Location;
use crate::session::{Origin, ReviewState, Session, SessionStatus, Verdict};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewResult {
    pub session_id: String,
    pub status: SessionStatus,
    pub review_state: Option<ReviewState>,
    pub summary: String,
    pub export_path: Option<PathBuf>,
    /// Whether the Review was posted to a Code host. Always false in local mode.
    pub posted: bool,
    pub findings: Vec<ResultFinding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultFinding {
    pub id: String,
    pub origin: Origin,
    pub title: String,
    pub location: Location,
    pub verdict: Option<String>,
    pub comment: Option<String>,
    pub dismiss_reason: Option<String>,
    pub escalate_to: Option<String>,
}

impl ReviewResult {
    pub fn from_session(s: &Session) -> ReviewResult {
        let findings = s
            .entries
            .iter()
            .map(|e| {
                let ruling = e.ruling.as_ref();
                let verdict = ruling.map(|r| &r.verdict);
                ResultFinding {
                    id: e.id.clone(),
                    origin: e.origin(),
                    title: e.display_title(),
                    location: e.location().clone(),
                    verdict: verdict.map(|v| v.key().to_string()),
                    comment: ruling.map(|r| r.comment.clone()),
                    dismiss_reason: match verdict {
                        Some(Verdict::Dismiss { reason }) => Some(reason.clone()),
                        _ => None,
                    },
                    escalate_to: match verdict {
                        Some(Verdict::Escalate { to }) => Some(to.clone()),
                        _ => None,
                    },
                }
            })
            .collect();
        let outcome = s.outcome.as_ref();
        ReviewResult {
            session_id: s.id.clone(),
            status: s.status,
            review_state: outcome.and_then(|o| o.state),
            summary: outcome.map(|o| o.summary.clone()).unwrap_or_default(),
            export_path: outcome.and_then(|o| o.export_path.clone()),
            posted: false,
            findings,
        }
    }
}
```

- [ ] **Step 5: Run tests and clippy**

Run: `cargo test --lib && cargo clippy --all-targets -- -D warnings`
Expected: all pass (6 new in compose, 1 in result), no warnings.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: compose the review from verdicts and define the result contract"
```

---

### Task 6: Web UI — security guard, assets, finding page

**Files:**
- Create: `src/server/mod.rs`, `src/server/handlers.rs`, `src/server/pages.rs`, `src/server/highlight.rs`, `assets/app.css`, `assets/app.js`, `assets/vendor/mermaid.min.js`, `assets/vendor/README.md`
- Modify: `src/lib.rs` (add `pub mod server;`)

**Interfaces:**
- Consumes: `session::*`, `git::Repo`, `model::*`.
- Produces (in `prr::server`):
  - `url(token: &str, rest: &str) -> String` → `/s/<token>/<rest>`
  - `AppState` (`Clone`): `new(session: Session, store: Store, repo: Repo, port: u16)`, `snapshot() -> Session`, `token() -> String`, `repo() -> &Repo`, `port() -> u16`, `update<T>(f: impl FnOnce(&mut Session) -> Result<T, SessionError>) -> Result<T, AppError>` (applies to a copy, saves to the Store, then commits), `is_stale() -> bool`, `finish()`, `subscribe() -> watch::Receiver<bool>`
  - `AppError { Session(SessionError), Internal(anyhow::Error) }` (`IntoResponse`: NotOpen→409, NoSuchFinding→404, Invalid→422, Internal→500), `From<SessionError>`
  - `router(state: AppState) -> axum::Router` — this task registers `/s/{token}/`, `/s/{token}/f/{idx}`, `/s/{token}/assets/{name}`
  - `pages::{finding(s: &Session, idx: usize, error: Option<&str>) -> Markup, done(message: &str) -> Markup}`; `highlight::code_block(path: &str, text: &str, first_line: u32) -> Markup`
  - Test fixture inside `server::tests`: `Fixture`, `fixture()`, `fixture_with(export_md, batch)`, `send(...)` — extended by Tasks 7–8.

- [ ] **Step 1: Vendor Mermaid**

```bash
mkdir -p assets/vendor
curl -fsSL https://cdn.jsdelivr.net/npm/mermaid@11.4.1/dist/mermaid.min.js -o assets/vendor/mermaid.min.js
printf 'mermaid.min.js: Mermaid 11.4.1 (MIT), from https://cdn.jsdelivr.net/npm/mermaid@11.4.1/dist/mermaid.min.js\n' > assets/vendor/README.md
head -c 200 assets/vendor/mermaid.min.js
```

Expected: minified JavaScript (not an HTML error page).

- [ ] **Step 2: Write the static assets**

`assets/app.css`:

```css
:root { --bg: #fff; --fg: #1f2328; --muted: #59636e; --line: #d1d9e0; --panel: #f6f8fa; --accent: #0969da; --danger: #cf222e; --warn: #9a6700; }
@media (prefers-color-scheme: dark) { :root { --bg: #0d1117; --fg: #e6edf3; --muted: #9198a1; --line: #3d444d; --panel: #151b23; --accent: #4493f8; --danger: #f85149; --warn: #d29922; } }
* { box-sizing: border-box; }
body { margin: 0; font: 15px/1.5 system-ui, sans-serif; background: var(--bg); color: var(--fg); }
a { color: var(--accent); }
header.top { display: flex; gap: 1rem; align-items: center; padding: .6rem 1rem; border-bottom: 1px solid var(--line); flex-wrap: wrap; }
header.top .brand { font-weight: 700; text-decoration: none; }
header.top .submit { margin-left: auto; }
div.main { display: flex; min-height: calc(100vh - 3rem); }
nav.sidebar { width: 18rem; flex: none; border-right: 1px solid var(--line); padding: .5rem; overflow-y: auto; }
nav.sidebar ol { list-style: none; margin: 0; padding: 0; }
nav.sidebar li { padding: .35rem .5rem; border-radius: 6px; display: flex; justify-content: space-between; gap: .5rem; }
nav.sidebar li.current { background: var(--panel); outline: 1px solid var(--line); }
nav.sidebar .sev, nav.sidebar .verdict { font-size: .75rem; color: var(--muted); white-space: nowrap; }
main { flex: 1; padding: 1rem 2rem 4rem; max-width: 70rem; min-width: 0; }
.badge { display: inline-block; font-size: .8rem; padding: .05rem .5rem; border: 1px solid var(--line); border-radius: 999px; }
.badge.ruled { border-color: var(--accent); }
.prose { white-space: pre-wrap; }
.error { color: var(--danger); font-weight: 600; }
.warning { color: var(--warn); font-weight: 600; }
.hint { color: var(--muted); font-size: .85rem; }
pre, table.code { background: var(--panel); border: 1px solid var(--line); border-radius: 6px; padding: .5rem; overflow-x: auto; }
table.code { border-collapse: collapse; width: 100%; background: #fff; color: #1f2328; font: 13px/1.45 ui-monospace, monospace; }
table.code td.ln { color: #8c959f; text-align: right; padding-right: 1rem; user-select: none; width: 1%; }
table.code td.src { white-space: pre; }
.evidence { margin: 1rem 0; padding-left: 1rem; border-left: 3px solid var(--line); }
form.ruling, form.submit, form.new { margin-top: 2rem; padding: 1rem; border: 1px solid var(--line); border-radius: 8px; background: var(--panel); }
form label { display: block; margin: .6rem 0; }
form textarea, form input[type=text] { width: 100%; font: inherit; padding: .4rem; background: var(--bg); color: var(--fg); border: 1px solid var(--line); border-radius: 6px; }
fieldset.verdicts { border: 0; padding: 0; display: flex; gap: 1rem; flex-wrap: wrap; }
fieldset.verdicts label { display: inline-flex; gap: .3rem; align-items: center; margin: 0; }
form.ruling:not(:has(input[value=dismiss]:checked)) label.reason { display: none; }
form.ruling:not(:has(input[value=escalate]:checked)) label.escalate { display: none; }
button { font: inherit; padding: .4rem 1rem; border-radius: 6px; border: 1px solid var(--accent); background: var(--accent); color: #fff; cursor: pointer; }
button.secondary { background: transparent; color: var(--danger); border-color: var(--danger); }
kbd { font-size: .75rem; border: 1px solid var(--line); border-radius: 4px; padding: 0 .3rem; color: var(--muted); }
```

`assets/app.js`:

```js
(() => {
  const form = document.querySelector('form.ruling');
  const article = document.querySelector('article.finding');
  const typing = (el) => el && (el.tagName === 'TEXTAREA' || el.tagName === 'INPUT');
  const fieldFor = { dismiss: 'reason', escalate: 'escalate_to' };

  document.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter' && (ev.ctrlKey || ev.metaKey) && form) {
      ev.preventDefault();
      form.requestSubmit();
      return;
    }
    if (typing(document.activeElement) || ev.ctrlKey || ev.metaKey || ev.altKey) return;
    const radio = form && form.querySelector(`input[data-key="${ev.key}"]`);
    if (radio) {
      ev.preventDefault();
      radio.checked = true;
      const field = fieldFor[radio.value];
      if (field) form.elements[field].focus();
      return;
    }
    if (article && (ev.key === 'j' || ev.key === 'k')) {
      const to = article.dataset[ev.key === 'j' ? 'next' : 'prev'];
      if (to) location.href = to;
    }
  });

  if (window.mermaid) {
    const dark = matchMedia('(prefers-color-scheme: dark)').matches;
    mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: dark ? 'dark' : 'default' });
    (async () => {
      for (const el of document.querySelectorAll('pre.mermaid')) {
        try {
          await mermaid.parse(el.textContent);
        } catch (e) {
          el.classList.remove('mermaid');
          const msg = document.createElement('p');
          msg.className = 'error';
          msg.textContent = 'This diagram could not be rendered: ' + (e && e.message ? e.message : e);
          el.before(msg);
        }
      }
      await mermaid.run({ querySelector: 'pre.mermaid' });
    })();
  }
})();
```

- [ ] **Step 3: Write the failing tests** — `src/server/mod.rs` (tests only, plus module declarations so it compiles once implemented):

```rust
//! The localhost web UI where the Reviewer adjudicates Findings.

mod handlers;
mod highlight;
mod pages;

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
```

Create empty `src/server/handlers.rs`, `src/server/pages.rs`, `src/server/highlight.rs` and add `pub mod server;` to `src/lib.rs`.

- [ ] **Step 4: Run to verify failure**

Run: `cargo test --lib server`
Expected: compile errors — `AppState`, `router`, `url` not found.

- [ ] **Step 5: Implement highlighting** — `src/server/highlight.rs`:

```rust
//! Server-side syntax highlighting for code excerpts.

use std::path::Path;
use std::sync::LazyLock;

use maud::{html, Markup, PreEscaped};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::html::{styled_line_to_highlighted_html, IncludeBackground};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

/// `text` as a line-numbered, highlighted table starting at `first_line`.
pub fn code_block(path: &str, text: &str, first_line: u32) -> Markup {
    let ext = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("");
    let syntax = SYNTAXES.find_syntax_by_extension(ext).unwrap_or_else(|| SYNTAXES.find_syntax_plain_text());
    let mut h = HighlightLines::new(syntax, &THEMES.themes["InspiredGitHub"]);
    let rows: Vec<(u32, String)> = LinesWithEndings::from(text)
        .enumerate()
        .map(|(i, line)| {
            let highlighted = h
                .highlight_line(line, &SYNTAXES)
                .ok()
                .and_then(|regions| styled_line_to_highlighted_html(&regions, IncludeBackground::No).ok())
                .unwrap_or_else(|| html! { (line) }.into_string());
            (first_line + i as u32, highlighted)
        })
        .collect();
    html! {
        table.code {
            @for (n, line) in &rows {
                tr { td.ln { (n) } td.src { (PreEscaped(line)) } }
            }
        }
    }
}
```

- [ ] **Step 6: Implement the pages** — `src/server/pages.rs`:

```rust
//! HTML pages, rendered with maud. Every interpolation is HTML-escaped unless wrapped in PreEscaped.

use std::collections::BTreeMap;

use maud::{html, Markup, DOCTYPE};

use super::highlight::code_block;
use super::url;
use crate::model::{Evidence, Finding, Location};
use crate::session::{Origin, Session, Verdict};

const VERDICTS: [(&str, &str, &str); 5] = [
    ("block", "Block", "b"),
    ("comment", "Comment", "c"),
    ("nit", "Nit", "n"),
    ("dismiss", "Dismiss", "d"),
    ("escalate", "Escalate", "e"),
];

pub(super) fn location_label(loc: &Location) -> String {
    match loc {
        Location::Lines { path, start, end } if start == end => format!("{path}:{start}"),
        Location::Lines { path, start, end } => format!("{path}:{start}-{end}"),
        Location::PrWide => "PR-wide".into(),
    }
}

pub(super) fn layout(s: &Session, title: &str, current: Option<usize>, body: Markup) -> Markup {
    let t = &s.token;
    let ruled = s.entries.len() - s.unruled().len();
    let short_sha = s.head_sha.get(..12).unwrap_or(&s.head_sha);
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " · prr" }
                link rel="stylesheet" href=(url(t, "assets/app.css"));
            }
            body {
                header.top {
                    a.brand href=(url(t, "")) { "prr" }
                    span { (s.target.label()) " @ " code { (short_sha) } }
                    span.hint { (ruled) "/" (s.entries.len()) " have a Verdict" }
                    a href=(url(t, "new")) { "+ Add your own finding" }
                    a.submit href=(url(t, "submit")) { "Review & submit →" }
                }
                div.main {
                    nav.sidebar {
                        ol {
                            @for (i, e) in s.entries.iter().enumerate() {
                                li.current[current == Some(i)] {
                                    a href=(url(t, &format!("f/{i}"))) { (e.display_title()) }
                                    span {
                                        span.sev { (e.severity().map_or("Yours", |sev| sev.label())) }
                                        " · "
                                        span.verdict { (e.ruling.as_ref().map_or("—", |r| r.verdict.label())) }
                                    }
                                }
                            }
                        }
                    }
                    main { (body) }
                }
                script src=(url(t, "assets/mermaid.min.js")) {}
                script src=(url(t, "assets/app.js")) {}
            }
        }
    }
}

pub fn finding(s: &Session, idx: usize, error: Option<&str>) -> Markup {
    let t = &s.token;
    let e = &s.entries[idx];
    let prev = idx.checked_sub(1).map(|p| url(t, &format!("f/{p}")));
    let next = (idx + 1 < s.entries.len()).then(|| url(t, &format!("f/{}", idx + 1)));
    let location_code = match (e.location(), &e.location_text) {
        (Location::Lines { path, start, .. }, Some(text)) => Some(code_block(path, text, *start)),
        _ => None,
    };
    let origin = match e.origin() {
        Origin::Agent => "From the Agent",
        Origin::Reviewer => "Your finding",
    };
    let body = html! {
        article.finding data-prev=[prev] data-next=[next] {
            h1 { (e.display_title()) }
            p.meta {
                @if let Some(f) = e.finding() {
                    span.badge { (f.severity.label()) } " "
                    span.badge { "Confidence: " (f.confidence.level.label()) } " "
                }
                span.badge { (origin) }
                @if let Some(r) = &e.ruling { " " span.badge.ruled { "Verdict: " (r.verdict.label()) } }
            }
            @if let Some(msg) = error { p.error { (msg) } }
            section {
                h2 { "Location" }
                p { code { (location_label(e.location())) } }
                @if let Some(code) = location_code { (code) }
            }
            @if let Some(f) = e.finding() { (finding_details(f, &e.excerpts)) }
            (ruling_form(s, idx))
        }
    };
    layout(s, &e.display_title(), Some(idx), body)
}

fn finding_details(f: &Finding, excerpts: &BTreeMap<usize, String>) -> Markup {
    html! {
        p { strong { "Why this confidence: " } (f.confidence.rationale) }
        section { h2 { "Summary" } p.prose { (f.summary) } }
        section { h2 { "Impact" } p.prose { (f.impact) } }
        section {
            h2 { "Recommendation" }
            p.prose { (f.recommendation) }
            @if let Some(patch) = &f.suggested_patch { h3 { "Suggested change" } pre { (patch) } }
        }
        @if !f.evidence.is_empty() {
            section {
                h2 { "Evidence" }
                @for (j, ev) in f.evidence.iter().enumerate() {
                    div.evidence { (evidence_item(ev, excerpts.get(&j))) }
                }
            }
        }
    }
}

fn evidence_item(ev: &Evidence, resolved: Option<&String>) -> Markup {
    match ev {
        Evidence::CodeExcerpt { path, start, end, git_ref, caption } => html! {
            h3 { code { (location_label(&Location::Lines { path: path.clone(), start: *start, end: *end })) } @if let Some(r) = git_ref { " at " code { (r) } } }
            @if let Some(c) = caption { p.hint { (c) } }
            @if let Some(text) = resolved { (code_block(path, text, *start)) }
        },
        Evidence::Diagram { mermaid, caption } => html! {
            @if let Some(c) = caption { p.hint { (c) } }
            pre.mermaid { (mermaid) }
        },
        Evidence::Note { text } => html! { p.prose { (text) } },
        Evidence::CommandOutput { command, output, exit_code } => html! {
            h3 { "$ " code { (command) } @if let Some(c) = exit_code { " (exit " (c) ")" } }
            pre { (output) }
        },
    }
}

fn ruling_form(s: &Session, idx: usize) -> Markup {
    let e = &s.entries[idx];
    let ruling = e.ruling.as_ref();
    let current = ruling.map(|r| r.verdict.key());
    let comment = ruling.map_or(e.draft_comment(), |r| r.comment.as_str());
    let reason = match ruling.map(|r| &r.verdict) {
        Some(Verdict::Dismiss { reason }) => reason.as_str(),
        _ => "",
    };
    let escalate_to = match ruling.map(|r| &r.verdict) {
        Some(Verdict::Escalate { to }) => to.as_str(),
        _ => e.finding().and_then(|f| f.escalate_to.as_deref()).unwrap_or(""),
    };
    html! {
        form.ruling method="post" action=(url(&s.token, &format!("f/{idx}/ruling"))) {
            fieldset.verdicts {
                legend { "Verdict" }
                @for (key, name, shortcut) in VERDICTS {
                    label {
                        input type="radio" name="verdict" value=(key) data-key=(shortcut) checked[current == Some(key)];
                        (name) " " kbd { (shortcut) }
                    }
                }
            }
            label { "Comment to post" textarea name="comment" rows="6" { (comment) } }
            label.reason { "Reason for dismissing (kept in the record, not posted)" input type="text" name="reason" value=(reason); }
            label.escalate { "Escalate to (@user or @org/team)" input type="text" name="escalate_to" value=(escalate_to); }
            button type="submit" { "Save verdict" }
            " " span.hint { "Ctrl+Enter saves · j/k move between findings" }
        }
    }
}

/// A standalone page for final or error messages.
pub fn done(message: &str) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head { meta charset="utf-8"; title { "prr" } }
            body style="font-family: system-ui, sans-serif; padding: 2rem" { p { (message) } }
        }
    }
}
```

- [ ] **Step 7: Implement the handlers** — `src/server/handlers.rs`:

```rust
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
```

- [ ] **Step 8: Implement state, errors, guard and router** — in `src/server/mod.rs`, between the `mod` lines and the tests:

```rust
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
```

- [ ] **Step 9: Run tests and clippy**

Run: `cargo test --lib server && cargo clippy --all-targets -- -D warnings`
Expected: 5 passed, no warnings. (Building the first time compiles syntect; allow a minute.)

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat: serve the finding page behind a localhost guard"
```

---

### Task 7: Recording Verdicts and Reviewer findings

**Files:**
- Modify: `src/server/handlers.rs`, `src/server/pages.rs`, `src/server/mod.rs` (routes + tests)

**Interfaces:**
- Consumes: `Verdict::from_form`, `Session::{rule, add_reviewer_finding, next_unruled_after}`, `validate::{check_repo_path, resolve_lines}`.
- Produces: routes `POST /s/{token}/f/{idx}/ruling` (form `verdict, comment, reason, escalate_to`; 303 to next unruled finding or `submit`; 422 with the finding page and message on invalid input), `GET|POST /s/{token}/new` (form `path, start, end, comment`; 303 to the new finding; 422 re-renders with the values kept). `handlers::NewFindingForm` (pub fields `path, start, end, comment: String`, `Default`). `pages::new_finding(s: &Session, form: &NewFindingForm, error: Option<&str>) -> Markup`.

- [ ] **Step 1: Write the failing tests** — append inside `mod tests` in `src/server/mod.rs`:

```rust
    #[tokio::test]
    async fn rejects_cross_origin_posts() {
        let fx = fixture();
        let origin = |o| Some(("origin", o));
        let form = Some("verdict=nit&comment=x");
        let (s, _, _) = send(&fx.app, Method::POST, &fx.path("f/0/ruling"), form, HOST, origin("http://evil.example")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, _, _) = send(&fx.app, Method::POST, &fx.path("f/0/ruling"), form, HOST, origin("http://127.0.0.1:4321")).await;
        assert_eq!(s, StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn dismiss_without_reason_is_rejected_and_nothing_is_saved() {
        let fx = fixture();
        let (s, _, body) = fx.post("f/0/ruling", "verdict=dismiss&comment=&reason=").await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("Dismiss needs a reason"));
        assert!(fx.store.load(&fx.id).unwrap().entries[0].ruling.is_none());
    }

    #[tokio::test]
    async fn ruling_saves_to_disk_and_moves_to_next_unruled() {
        let fx = fixture();
        let (s, h, _) = fx.post("f/0/ruling", "verdict=block&comment=Fix+it").await;
        assert_eq!(s, StatusCode::SEE_OTHER);
        assert_eq!(location(&h), fx.path("f/1"));
        let saved = fx.store.load(&fx.id).unwrap();
        let r = saved.entries[0].ruling.as_ref().unwrap();
        assert_eq!((&r.verdict, r.comment.as_str()), (&crate::session::Verdict::Block, "Fix it"));

        let (_, h, _) = fx.post("f/1/ruling", "verdict=escalate&comment=Ask&escalate_to=%40sec").await;
        assert_eq!(location(&h), fx.path("submit"));

        let (_, _, page) = fx.get("f/1").await;
        assert!(page.contains(r#"value="@sec""#), "the saved ruling pre-fills the form");
    }

    #[tokio::test]
    async fn reviewer_can_add_a_finding() {
        let fx = fixture();
        let (s, h, _) = fx.post("new", "path=src%2Flib.rs&start=3&end=&comment=Rename+this").await;
        assert_eq!(s, StatusCode::SEE_OTHER);
        assert_eq!(location(&h), fx.path("f/2"));
        let saved = fx.store.load(&fx.id).unwrap();
        assert_eq!(saved.entries[2].id, "reviewer-1");
        assert_eq!(saved.entries[2].location_text.as_deref(), Some("line 3"));

        let (s, _, body) = fx.post("new", "path=..%2Fetc&start=1&comment=Keep+me").await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("repo-relative"));
        assert!(body.contains("Keep me"), "typed values survive the error");

        let (s, h, _) = fx.post("new", "path=&start=&comment=General+point").await;
        assert_eq!(s, StatusCode::SEE_OTHER);
        assert_eq!(location(&h), fx.path("f/3"));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib server`
Expected: the four new tests FAIL with 404 status (routes not registered; unmatched routes bypass the guard).

- [ ] **Step 3: Implement the handlers** — append to `src/server/handlers.rs`, and extend its imports to:

```rust
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;

use super::{pages, url, AppError, AppState};
use crate::model::Location;
use crate::session::{Ruling, Session, SessionError, Verdict};
use crate::validate::{check_repo_path, resolve_lines};
```

```rust
#[derive(Deserialize)]
pub struct RulingForm {
    #[serde(default)]
    verdict: String,
    #[serde(default)]
    comment: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    escalate_to: String,
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
            let page = pages::finding(&st.snapshot(), idx, Some(&msg));
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
```

- [ ] **Step 4: Implement the page** — append to `src/server/pages.rs` (add `use super::handlers::NewFindingForm;` to the imports):

```rust
pub fn new_finding(s: &Session, form: &NewFindingForm, error: Option<&str>) -> Markup {
    let body = html! {
        h1 { "Add your own finding" }
        @if let Some(msg) = error { p.error { (msg) } }
        form.new method="post" action=(url(&s.token, "new")) {
            label { "File (leave empty for a PR-wide finding)" input type="text" name="path" value=(form.path) placeholder="src/main.rs"; }
            label { "Start line" input type="text" name="start" value=(form.start) inputmode="numeric"; }
            label { "End line (optional)" input type="text" name="end" value=(form.end) inputmode="numeric"; }
            label { "Comment" textarea name="comment" rows="6" { (form.comment) } }
            button type="submit" { "Add, then choose a verdict" }
        }
    };
    layout(s, "Add your own finding", None, body)
}
```

- [ ] **Step 5: Register the routes** — in `router` in `src/server/mod.rs`, add `use axum::routing::post;` and after the `f/{idx}` route:

```rust
        .route("/s/{token}/f/{idx}/ruling", post(handlers::save_ruling))
        .route("/s/{token}/new", get(handlers::new_page).post(handlers::create_reviewer_finding))
```

- [ ] **Step 6: Run tests and clippy**

Run: `cargo test --lib server && cargo clippy --all-targets -- -D warnings`
Expected: 9 passed, no warnings.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: record verdicts and reviewer findings from the browser"
```

---

### Task 8: Submit and abandon

**Files:**
- Modify: `src/server/handlers.rs`, `src/server/pages.rs`, `src/server/mod.rs` (routes + tests)

**Interfaces:**
- Consumes: `compose::{compose, cite_plain, to_markdown, SubmitChoice, ComposedReview}`, `AppState::{is_stale, finish, update}`, `session::{Outcome, SessionStatus}`.
- Produces: `GET /s/{token}/submit` (lists findings still without a Verdict, or the form and preview); `POST /s/{token}/submit` (form `summary`, `approve`, `ack_stale`; on success writes `export_md` if set, sets status `Submitted` and `outcome`, calls `finish()`, returns 200 page containing `submitted`; on failure 422 with message); `POST /s/{token}/abandon` (status `Abandoned`, `finish()`). `pages::submit(s: &Session, preview: Option<&ComposedReview>, stale: bool, error: Option<&str>) -> Markup`.

- [ ] **Step 1: Write the failing tests** — append inside `mod tests` in `src/server/mod.rs`:

```rust
    async fn rule_all(fx: &Fixture, first: &str) {
        fx.post("f/0/ruling", &format!("verdict={first}&comment=First")).await;
        fx.post("f/1/ruling", "verdict=nit&comment=Second").await;
    }

    #[tokio::test]
    async fn submit_requires_every_verdict() {
        let fx = fixture();
        let (_, _, page) = fx.get("submit").await;
        assert!(page.contains("Still needs a verdict"));
        let (s, _, body) = fx.post("submit", "summary=").await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("still need a Verdict"));
    }

    #[tokio::test]
    async fn approve_is_refused_when_something_blocks() {
        let fx = fixture();
        rule_all(&fx, "block").await;
        let (s, _, body) = fx.post("submit", "approve=on").await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("Approve is not available"));
    }

    #[tokio::test]
    async fn submit_finishes_the_session_and_writes_export() {
        let out = tempfile::tempdir().unwrap();
        let md = out.path().join("review.md");
        let fx = fixture_with(Some(md.clone()), sample_batch());
        rule_all(&fx, "block").await;
        let (_, _, page) = fx.get("submit").await;
        assert!(page.contains("Preview") && page.contains("Request changes"), "{page}");

        let (s, _, body) = fx.post("submit", "summary=Thanks").await;
        assert_eq!(s, StatusCode::OK);
        assert!(body.contains("submitted"));
        assert!(*fx.state.subscribe().borrow());
        let saved = fx.store.load(&fx.id).unwrap();
        assert_eq!(saved.status, SessionStatus::Submitted);
        let outcome = saved.outcome.unwrap();
        assert_eq!(outcome.state, Some(ReviewState::RequestChanges));
        assert_eq!(outcome.summary, "Thanks");
        assert_eq!(outcome.export_path, Some(md.clone()));
        let text = std::fs::read_to_string(&md).unwrap();
        assert!(text.starts_with("# Review: Request changes"), "{text}");
        assert!(text.contains("**Blocking:** First"));

        let (s, _, _) = fx.post("f/0/ruling", "verdict=nit&comment=late").await;
        assert_eq!(s, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn stale_head_needs_acknowledgement() {
        let fx = fixture();
        rule_all(&fx, "comment").await;
        fx.repo.write("src/other.rs", "new\n");
        fx.repo.commit("more work");
        let (_, _, page) = fx.get("submit").await;
        assert!(page.contains("new commits"));
        let (s, _, body) = fx.post("submit", "summary=").await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("new commits"));
        let (s, _, _) = fx.post("submit", "summary=&ack_stale=on").await;
        assert_eq!(s, StatusCode::OK);
    }

    #[tokio::test]
    async fn abandon_ends_the_session_without_an_outcome() {
        let fx = fixture();
        let (s, _, body) = fx.post("abandon", "").await;
        assert_eq!(s, StatusCode::OK);
        assert!(body.contains("abandoned"));
        let saved = fx.store.load(&fx.id).unwrap();
        assert_eq!(saved.status, SessionStatus::Abandoned);
        assert!(saved.outcome.is_none());
        assert!(*fx.state.subscribe().borrow());
    }

    #[tokio::test]
    async fn empty_batch_goes_straight_to_submit() {
        let fx = fixture_with(None, FindingBatch { findings: vec![] });
        let (_, h, _) = fx.get("").await;
        assert_eq!(location(&h), fx.path("submit"));
        let (s, _, _) = fx.post("submit", "summary=").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(fx.store.load(&fx.id).unwrap().outcome.unwrap().state, None);
    }
```

Add `use crate::session::{ReviewState, SessionStatus};` to the test module's imports.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib server`
Expected: the six new tests FAIL (404/405 — routes missing).

- [ ] **Step 3: Implement the submit page** — append to `src/server/pages.rs` (add `use crate::compose::ComposedReview;` to the imports):

```rust
pub fn submit(s: &Session, preview: Option<&ComposedReview>, stale: bool, error: Option<&str>) -> Markup {
    let t = &s.token;
    let unruled = s.unruled();
    let has_block = s.entries.iter().any(|e| e.ruling.as_ref().is_some_and(|r| r.verdict == Verdict::Block));
    let body = html! {
        h1 { "Review & submit" }
        @if let Some(msg) = error { p.error { (msg) } }
        @if !s.is_open() {
            p { "This session is closed." }
        } @else if !unruled.is_empty() {
            p { "Still needs a verdict:" }
            ul {
                @for i in &unruled {
                    li { a href=(url(t, &format!("f/{i}"))) { (s.entries[*i].display_title()) } }
                }
            }
        } @else {
            form.submit method="post" action=(url(t, "submit")) {
                @if stale {
                    p.warning { "The change has new commits since these Findings were proposed; some may no longer apply." }
                    label { input type="checkbox" name="ack_stale"; " I understand; submit anyway" }
                }
                label { "Summary (top of the Review)" textarea name="summary" rows="4" {} }
                label {
                    input type="checkbox" name="approve" disabled[has_block];
                    " Approve"
                    @if has_block { " (not available: something is Blocked)" }
                }
                p.hint { "If every Finding is dismissed: tick Approve to approve, write a summary to post a comment, or leave both empty to post nothing." }
                @if let Some(path) = &s.export_md { p.hint { "The Review will be written to " code { (path.display()) } "." } }
                button type="submit" { "Submit review" }
            }
            @if let Some(review) = preview {
                section {
                    h2 { "Preview" }
                    p { "State: " strong { (review.state.map_or("Nothing to post", |st| st.label())) } " (your summary and Approve choice are applied on submit)" }
                    @if !review.body.is_empty() { h3 { "Review body" } pre { (review.body) } }
                    @for c in &review.inline {
                        h3 { "Inline on " code { (location_label(&Location::Lines { path: c.path.clone(), start: c.start_line, end: c.line })) } }
                        pre { (c.body) }
                    }
                }
            }
        }
        @if s.is_open() {
            form method="post" action=(url(t, "abandon")) {
                button.secondary type="submit" { "Abandon session (post nothing)" }
            }
        }
    };
    layout(s, "Review & submit", None, body)
}
```

- [ ] **Step 4: Implement the handlers** — append to `src/server/handlers.rs`, extending imports with:

```rust
use anyhow::Context;

use crate::compose::{cite_plain, compose, to_markdown, ComposedReview, SubmitChoice};
use crate::session::{Outcome, SessionStatus};
```

```rust
pub async fn submit_page(State(st): State<AppState>) -> Html<String> {
    let s = st.snapshot();
    Html(pages::submit(&s, preview(&st, &s).as_ref(), st.is_stale(), None).into_string())
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
    summary: String,
    #[serde(default)]
    approve: Option<String>,
    #[serde(default)]
    ack_stale: Option<String>,
}

pub async fn submit(State(st): State<AppState>, Form(form): Form<SubmitForm>) -> Result<Response, AppError> {
    let s = st.snapshot();
    if !s.is_open() {
        return Err(SessionError::NotOpen.into());
    }
    let stale = st.is_stale();
    let reject = |msg: &str| -> Response {
        let page = pages::submit(&s, preview(&st, &s).as_ref(), stale, Some(msg));
        (StatusCode::UNPROCESSABLE_ENTITY, Html(page.into_string())).into_response()
    };
    if stale && form.ack_stale.is_none() {
        return Ok(reject("The change has new commits since these Findings were proposed. Tick the acknowledgement to submit anyway."));
    }
    let choice = SubmitChoice { approve: form.approve.is_some(), summary: form.summary.trim().to_string() };
    let diff = st.repo().diff_map(&s.base_sha, &s.head_sha).map_err(AppError::Internal)?;
    let review = match compose(&s.entries, &diff, &choice, &cite_plain) {
        Ok(r) => r,
        Err(e) => return Ok(reject(&e.to_string())),
    };
    if let Some(path) = &s.export_md {
        std::fs::write(path, to_markdown(&review))
            .with_context(|| format!("writing {}", path.display()))
            .map_err(AppError::Internal)?;
    }
    st.update(|s| {
        if !s.is_open() {
            return Err(SessionError::NotOpen);
        }
        s.status = SessionStatus::Submitted;
        s.outcome = Some(Outcome { state: review.state, summary: choice.summary.clone(), export_path: s.export_md.clone() });
        Ok(())
    })?;
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
```

- [ ] **Step 5: Register the routes** — in `router`:

```rust
        .route("/s/{token}/submit", get(handlers::submit_page).post(handlers::submit))
        .route("/s/{token}/abandon", post(handlers::abandon))
```

- [ ] **Step 6: Run tests and clippy**

Run: `cargo test --lib server && cargo clippy --all-targets -- -D warnings`
Expected: 15 passed, no warnings.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: submit or abandon the review with stale-change protection"
```

---

### Task 9: `prr review` end to end

**Files:**
- Modify: `src/server/mod.rs` (add `ServeEnd`, `serve_session`), `src/cli.rs`, `tests/cli.rs`

**Interfaces:**
- Consumes: everything above.
- Produces:
  - `prr::server::ServeEnd { Finished(Session), Interrupted(Session) }`
  - `prr::server::serve_session(session: Session, store: Store, repo: Repo, open_browser: bool) -> anyhow::Result<ServeEnd>` (async; prints `prr: session <id>` then `prr: open <url>` to stderr)
  - `cli::run_server(session, store, repo, open_browser) -> Result<i32>` (prints result JSON, maps to exit codes) and `cli::print_result(&Session) -> Result<i32>` — reused by Task 10
  - `prr review` command per the spec.

- [ ] **Step 1: Write the failing end-to-end tests** — append to `tests/cli.rs`:

```rust
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test cli`
Expected: the four new tests FAIL — clap rejects the unknown `review` subcommand (exit 2), or `start` times out.

- [ ] **Step 3: Implement `serve_session`** — add to `src/server/mod.rs` (above the tests; add `use std::sync::atomic::{AtomicBool, Ordering};` and `use tokio::net::TcpListener;`):

```rust
pub enum ServeEnd {
    /// The Reviewer submitted or abandoned.
    Finished(Session),
    /// Ctrl-C; the session is left open on disk.
    Interrupted(Session),
}

/// Serves `session` on a random localhost port until the Reviewer finishes or the process is interrupted.
pub async fn serve_session(session: Session, store: Store, repo: Repo, open_browser: bool) -> anyhow::Result<ServeEnd> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let link = format!("http://127.0.0.1:{port}{}", url(&session.token, ""));
    eprintln!("prr: session {}", session.id);
    eprintln!("prr: open {link}");
    let state = AppState::new(session, store, repo, port);
    if open_browser && let Err(e) = webbrowser::open(&link) {
        eprintln!("prr: could not open a browser ({e}); open the URL above yourself");
    }
    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = interrupted.clone();
    let mut done = state.subscribe();
    let shutdown = async move {
        tokio::select! {
            _ = done.wait_for(|d| *d) => {}
            _ = tokio::signal::ctrl_c() => flag.store(true, Ordering::SeqCst),
        }
    };
    axum::serve(listener, router(state.clone())).with_graceful_shutdown(shutdown).await?;
    let session = state.snapshot();
    Ok(if interrupted.load(Ordering::SeqCst) { ServeEnd::Interrupted(session) } else { ServeEnd::Finished(session) })
}
```

- [ ] **Step 4: Implement the `review` command** — in `src/cli.rs`, replace the imports, `Command` enum and `dispatch` with:

```rust
use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

use crate::git::Repo;
use crate::model::FindingBatch;
use crate::result::ReviewResult;
use crate::server::{self, ServeEnd};
use crate::session::{Session, SessionStatus, Store, Target};
use crate::validate;
```

```rust
#[derive(Subcommand)]
enum Command {
    /// Validate Findings and open a Review session in the browser. Blocks until the Reviewer
    /// finishes, then prints the result JSON. Run it as a background task.
    Review(ReviewArgs),
    /// Print the JSON Schema of the findings file.
    Schema,
}

#[derive(Args)]
struct ReviewArgs {
    /// Findings JSON file, or `-` for stdin.
    #[arg(long)]
    findings: PathBuf,
    /// Base of the change, e.g. `main`.
    #[arg(long)]
    base: String,
    /// Head of the change.
    #[arg(long, default_value = "HEAD")]
    head: String,
    /// Print the URL instead of opening a browser.
    #[arg(long)]
    no_open: bool,
    /// Also write the composed Review as markdown to this path.
    #[arg(long)]
    export_md: Option<PathBuf>,
    /// Abandon an open session for the same change and start a new one.
    #[arg(long)]
    replace: bool,
}
```

```rust
fn dispatch(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Review(args) => review(args),
        Command::Schema => {
            let schema = schemars::schema_for!(FindingBatch);
            println!("{}", serde_json::to_string_pretty(&schema)?);
            Ok(exit::OK)
        }
    }
}

fn review(args: ReviewArgs) -> Result<i32> {
    let repo = Repo::discover(&std::env::current_dir()?)?;
    let head_sha = repo.rev_parse(&args.head)?;
    let base_sha = repo.merge_base(&repo.rev_parse(&args.base)?, &head_sha)?;
    let json = if args.findings.as_os_str() == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else {
        std::fs::read_to_string(&args.findings).with_context(|| format!("reading {}", args.findings.display()))?
    };
    let entries = match validate::parse_batch(&json).and_then(|b| validate::validate(b, &repo, &head_sha)) {
        Ok(entries) => entries,
        Err(errors) => {
            eprintln!("{}", serde_json::to_string_pretty(&serde_json::json!({ "errors": errors }))?);
            return Ok(exit::INVALID_FINDINGS);
        }
    };
    let export_md = args.export_md.map(std::path::absolute).transpose()?;
    let store = Store::from_env()?;
    let session = Session::new(
        repo.root().to_path_buf(),
        Target::Local { base_ref: args.base, head_ref: args.head },
        base_sha,
        head_sha,
        entries,
        export_md,
    );
    if let Some(mut existing) = store.find_open(&session)? {
        if !args.replace {
            eprintln!(
                "prr: session {id} is already open for this change; resume it with `prr wait {id}` or pass --replace",
                id = existing.id
            );
            return Ok(exit::SESSION_EXISTS);
        }
        let _old_lock = store
            .try_lock(&existing.id)?
            .with_context(|| format!("session {} is being served by another prr process; finish it there first", existing.id))?;
        existing.status = SessionStatus::Abandoned;
        store.save(&existing)?;
    }
    store.save(&session)?;
    let _lock = store.try_lock(&session.id)?.context("could not lock the new session")?;
    run_server(session, store, repo, !args.no_open)
}

/// Serves the session until it ends, then prints the result and returns the exit code.
pub(crate) fn run_server(session: Session, store: Store, repo: Repo, open_browser: bool) -> Result<i32> {
    let runtime = tokio::runtime::Runtime::new()?;
    match runtime.block_on(server::serve_session(session, store, repo, open_browser))? {
        ServeEnd::Interrupted(s) => {
            eprintln!("prr: interrupted; session {id} is still open — resume with `prr wait {id}`", id = s.id);
            Ok(exit::INTERRUPTED)
        }
        ServeEnd::Finished(s) => print_result(&s),
    }
}

pub(crate) fn print_result(session: &Session) -> Result<i32> {
    println!("{}", serde_json::to_string_pretty(&ReviewResult::from_session(session))?);
    Ok(match session.status {
        SessionStatus::Abandoned => exit::ABANDONED,
        _ => exit::OK,
    })
}
```

- [ ] **Step 5: Run all tests and clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all pass, no warnings.

- [ ] **Step 6: Try it by hand**

```bash
cd "$(mktemp -d)" && git init -q -b main && printf 'a\nb\nc\n' > f.txt && git add -A && git commit -qm base && git checkout -qb feat && printf 'a\nB\nc\n' > f.txt && git commit -qam change
cargo run --manifest-path /home/jphill/Projects/rust/interactive-pr-review/Cargo.toml -- schema > /dev/null && echo schema-ok
```

Then write a findings file pointing at `f.txt` line 2 and run `prr review --findings findings.json --base main`. The browser should open on the Finding; choose a Verdict, submit, and check the JSON printed in the terminal.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: add prr review command serving a session until the reviewer finishes"
```

---

### Task 10: `prr wait` and `prr sessions`

**Files:**
- Modify: `src/cli.rs`, `tests/cli.rs`

**Interfaces:**
- Consumes: `run_server`, `print_result`, `Store::{load, try_lock, list}`.
- Produces: `prr wait <id> [--no-open]` — prints the result immediately if the session has ended; serves it if no process holds its lock; otherwise prints `prr: session <id> is being served by another prr process; waiting for it to finish` and polls once a second, taking over if the lock frees while the session is still open. `prr sessions [--open]` — JSON array of `{id, status, target, head_sha, repo_root, findings, unruled}`.

- [ ] **Step 1: Write the failing tests** — append to `tests/cli.rs`:

```rust
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test cli wait`
Expected: FAIL — clap rejects `wait`/`sessions` (exit 2).

- [ ] **Step 3: Implement** — in `src/cli.rs`, add variants to `Command`:

```rust
    /// Re-attach to a Review session: serve it again if no prr process is serving it, otherwise
    /// wait for that process. Prints the result JSON when the session ends.
    Wait {
        id: String,
        /// Print the URL instead of opening a browser.
        #[arg(long)]
        no_open: bool,
    },
    /// List Review sessions as JSON.
    Sessions {
        /// Only sessions still open.
        #[arg(long)]
        open: bool,
    },
```

dispatch arms:

```rust
        Command::Wait { id, no_open } => wait(&id, no_open),
        Command::Sessions { open } => sessions(open),
```

and the functions:

```rust
fn wait(id: &str, no_open: bool) -> Result<i32> {
    let store = Store::from_env()?;
    let mut announced = false;
    loop {
        let session = store.load(id)?;
        if !session.is_open() {
            return print_result(&session);
        }
        if let Some(_lock) = store.try_lock(id)? {
            // Re-read under the lock: the previous server may have finished in between.
            let session = store.load(id)?;
            if !session.is_open() {
                return print_result(&session);
            }
            let repo = Repo::discover(&session.repo_root)?;
            return run_server(session, store, repo, !no_open);
        }
        if !announced {
            eprintln!("prr: session {id} is being served by another prr process; waiting for it to finish");
            announced = true;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn sessions(open_only: bool) -> Result<i32> {
    let rows: Vec<serde_json::Value> = Store::from_env()?
        .list()?
        .into_iter()
        .filter(|s| !open_only || s.is_open())
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "status": s.status,
                "target": s.target.label(),
                "head_sha": s.head_sha,
                "repo_root": s.repo_root,
                "findings": s.entries.len(),
                "unruled": s.unruled().len(),
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&rows)?);
    Ok(exit::OK)
}
```

- [ ] **Step 4: Run all tests and clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all pass, no warnings.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: add prr wait to re-attach to sessions and prr sessions to list them"
```

---

### Task 11: Agent onboarding — `prr guide`, Claude Code skill, README

**Files:**
- Create: `src/guide.rs`, `src/guide.md`, `skills/prr/SKILL.md`, `README.md`
- Modify: `src/lib.rs` (add `pub mod guide;`), `src/cli.rs` (add `Guide`), `tests/cli.rs`

**Interfaces:**
- Consumes: `model::example_batch`, `cli::exit`.
- Produces: `prr::guide::guide() -> String`; `prr guide` command.

- [ ] **Step 1: Write the failing tests**

`src/guide.rs`:

```rust
//! The agent-facing usage guide printed by `prr guide`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::exit;
    use crate::model::FindingBatch;

    #[test]
    fn guide_embeds_a_valid_example() {
        let text = guide();
        let start = text.find("```json\n").unwrap() + "```json\n".len();
        let end = start + text[start..].find("\n```").unwrap();
        let example: FindingBatch = serde_json::from_str(&text[start..end]).unwrap();
        assert_eq!(example, crate::model::example_batch());
    }

    #[test]
    fn guide_documents_every_exit_code() {
        let text = guide();
        for code in [exit::OK, exit::ERROR, exit::INVALID_FINDINGS, exit::ABANDONED, exit::SESSION_EXISTS, exit::INTERRUPTED] {
            assert!(text.contains(&format!("| {code} |")), "exit code {code} missing");
        }
    }
}
```

Append to `tests/cli.rs`:

```rust
#[test]
fn guide_prints() {
    let out = prr().arg("guide").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("prr review --findings"));
}
```

Add `pub mod guide;` to `src/lib.rs`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test guide`
Expected: compile error — `guide` not found.

- [ ] **Step 3: Write the guide** — `src/guide.md`:

````markdown
# prr — guide for agents

prr lets a human Reviewer adjudicate the Findings you propose for a change. You analyze; the Reviewer decides. prr shows your Evidence in a browser and returns the Reviewer's Verdicts to you as JSON. prr never posts anything the Reviewer did not approve, and you should not post review comments yourself.

## Workflow

1. Analyze the change, e.g. `git diff main...HEAD`.
2. Write your Findings to a JSON file. The exact format is `prr schema`; an example is below.
3. Start the session **as a background task**, because it blocks until the Reviewer finishes, which can take a long time:

       prr review --findings findings.json --base main

   prr prints `prr: session <id>` and `prr: open <url>` on stderr and opens the browser. Tell the Reviewer the URL.
4. When it exits, read the result JSON on stdout. Each Finding has the Reviewer's `verdict` (`block`, `comment`, `nit`, `dismiss`, `escalate`), the `comment` as they edited it, and any `dismiss_reason` or `escalate_to`. If the change is the Reviewer's own, offer to fix the Blocks. Treat dismiss reasons as feedback on your analysis.

If your process dies, re-attach with `prr wait <session-id>`; `prr sessions --open` lists open sessions. Verdicts already made are kept.

Options: `--head <ref>` (default `HEAD`), `--findings -` to read stdin, `--export-md <path>` to also write the composed Review as markdown, `--no-open` to only print the URL, `--replace` to abandon an open session for the same change.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Review submitted; result on stdout |
| 1 | Error; message on stderr |
| 10 | Findings file invalid; `{"errors": [{"path", "message"}]}` on stderr, each `path` a JSON pointer. Fix and rerun. |
| 11 | The Reviewer abandoned the session; result on stdout |
| 12 | A session for this change is already open; `prr wait <id>`, or rerun with `--replace` |
| 130 | Interrupted; the session stays open for `prr wait` |

## Writing good Findings

- One claim per Finding. Two problems in one function are two Findings.
- `location` lines are 1-based, on the head commit. Paths are repo-relative with forward slashes: `src/main.rs`, not `./src/main.rs` or an absolute path. Use `{"kind": "pr_wide"}` for concerns not tied to lines.
- Show code with `code_excerpt` Evidence rather than quoting it in prose. prr reads excerpts from git, so the Reviewer can trust them. Add `"ref"` to show another commit, e.g. the base version.
- `confidence.rationale` says *why*: "traced all 4 callers" vs "pattern match only, not verified".
- Use a `diagram` (Mermaid source) when a flow, sequence or dependency is easier to see than to read.
- `command_output` is for real output you ran, such as a failing test.
- `suggested_patch` replaces the location's lines exactly and needs a `lines` location.
- `draft_comment` is what gets posted. Write it for the change's author, not for the Reviewer.
- Order doesn't matter; prr sorts by severity and then confidence.

## Example

```json
{{EXAMPLE}}
```
````

- [ ] **Step 4: Implement** — above the tests in `src/guide.rs`:

```rust
use crate::model::example_batch;

pub fn guide() -> String {
    let example = serde_json::to_string_pretty(&example_batch()).expect("example serializes");
    include_str!("guide.md").replace("{{EXAMPLE}}", &example)
}
```

In `src/cli.rs` add the variant and arm:

```rust
    /// Print the agent-facing usage guide.
    Guide,
```

```rust
        Command::Guide => {
            print!("{}", crate::guide::guide());
            Ok(exit::OK)
        }
```

- [ ] **Step 5: Write the Claude Code skill** — `skills/prr/SKILL.md`:

```markdown
---
name: prr
description: Use when reviewing a pull request or local branch with a human in the loop — propose findings with evidence, have the Reviewer adjudicate them in the browser with the prr CLI, then act on their verdicts. Use instead of posting a list of review comments directly.
---

# Interactive review with prr

1. Run `prr guide` and follow it. Use `prr schema` when unsure of the findings format.
2. Analyze the change yourself; prr does no analysis.
3. Write the findings file, then run `prr review --findings <file> --base <base>` with the Bash tool's `run_in_background: true`. Tell the user the `prr: open` URL from its output.
4. If prr exits 10, fix the reported paths in the findings file and rerun.
5. When it finishes, summarise the Verdicts for the user. If the change is theirs, offer to fix the Blocks.
6. Never post review comments yourself; the Reviewer decides what gets posted.
7. If you lose track of a session, `prr sessions --open` lists sessions and `prr wait <id>` re-attaches.
```

- [ ] **Step 6: Write the README** — `README.md`:

````markdown
# prr

Human-in-the-loop pull request review. An AI agent proposes Findings, each with Evidence, Impact, a Recommendation and its Confidence. You give each one a Verdict (Block, Comment, Nit, Dismiss or Escalate) in a local browser page, and prr composes the Review from your decisions.

```bash
cargo install --path .
prr guide          # how an agent drives it
prr schema         # findings file format
prr review --findings findings.json --base main
```

To install the Claude Code skill: `cp -r skills/prr ~/.claude/skills/`.

Vocabulary: [CONTEXT.md](CONTEXT.md). Design: [docs/superpowers/specs/2026-10-07-prr-design.md](docs/superpowers/specs/2026-10-07-prr-design.md). Decisions: [docs/adr](docs/adr).
````

- [ ] **Step 7: Run all tests and clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all pass, no warnings.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat: add agent guide, Claude Code skill and README"
```

---

## Follow-up plans (not in this plan)

- **Plan 2 — GitHub adapter:** `Target::Github`, `prr review --pr <n>` (base/head from `gh api`), staleness against the PR's current head, posting one review on submit (`event`, inline `line`/`start_line`/`side: RIGHT`), `--pending`, permalink citations, `posted: true`.
- **Plan 3 — External reviewers:** `prr import copilot --pr <n>`, `Proposal::External`, the `corroborates` field, and Verdicts that act on existing threads (reply, resolve).
