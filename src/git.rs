//! Read-only access to the repository via the `git` CLI.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

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
            // Explicit prefixes override diff.noprefix / diff.dstPrefix in the user's config.
            &[
                "-c",
                "core.quotePath=false",
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "-U3",
                base,
                head,
            ],
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
                // git appends a tab after paths that contain spaces.
                let target = unquote(target.strip_suffix('\t').unwrap_or(target));
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

    #[test]
    fn diff_map_ignores_prefix_config_and_handles_spaced_paths() {
        let t = crate::testutil::TestRepo::new();
        t.write("my file.rs", "a\nb\nc\n");
        t.write("b/inner.rs", "x\ny\n");
        let base = t.commit("base");
        t.write("my file.rs", "a\nB\nc\n");
        t.write("b/inner.rs", "x\nY\n");
        let head = t.commit("change");
        let map = t.repo().diff_map(&base, &head).unwrap();
        assert!(map.contains("my file.rs", 2, 2), "trailing tab after a spaced path");
        for (key, value) in [("diff.dstPrefix", "new/"), ("diff.noprefix", "true")] {
            t.git(&["config", key, value]);
            let map = t.repo().diff_map(&base, &head).unwrap();
            assert!(map.contains("my file.rs", 2, 2), "{key}");
            assert!(map.contains("b/inner.rs", 2, 2), "{key}");
            t.git(&["config", "--unset", key]);
        }
    }
}
