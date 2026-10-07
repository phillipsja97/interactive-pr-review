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
