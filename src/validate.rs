//! Checks an Agent's findings file against the repository before any Reviewer sees it.

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
