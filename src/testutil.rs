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
