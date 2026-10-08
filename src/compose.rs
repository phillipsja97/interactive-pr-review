//! Turns Verdicts into the Review: its state, body and inline comments.

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
        // The fence must be longer than any backtick run inside the patch, or the patch would close it.
        let longest = patch.split(|c| c != '`').map(str::len).max().unwrap_or(0);
        let fence = "`".repeat(longest.max(2) + 1);
        text.push_str(&format!("\n\n{fence}suggestion\n{patch}\n{fence}"));
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
        assert_eq!(review_state(std::slice::from_ref(&nit), &choice(false, "")), Ok(Some(ReviewState::Comment)));
        assert_eq!(review_state(&[nit], &choice(true, "")), Ok(Some(ReviewState::Approve)));
        assert_eq!(review_state(std::slice::from_ref(&gone), &choice(false, "")), Ok(None));
        assert_eq!(review_state(std::slice::from_ref(&gone), &choice(false, "LGTM")), Ok(Some(ReviewState::Comment)));
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

    #[test]
    fn suggestion_fence_outlasts_backticks_in_the_patch() {
        let mut e = ruled("in", at(10, 10), Verdict::Comment, "Doc fix");
        if let Proposal::Agent(f) = &mut e.proposal {
            f.suggested_patch = Some("/// ```\n/// let x = 1;\n/// ```".into());
        }
        let r = compose(&[e], &diff(), &choice(false, ""), &cite_plain).unwrap();
        assert_eq!(r.inline[0].body, "Doc fix\n\n````suggestion\n/// ```\n/// let x = 1;\n/// ```\n````");
    }
}
