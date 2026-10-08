//! HTML pages, rendered with maud. Every interpolation is HTML-escaped unless wrapped in PreEscaped.

use std::collections::BTreeMap;

use maud::{html, Markup, DOCTYPE};

use super::handlers::{NewFindingForm, RulingForm, SubmitForm};
use super::highlight::code_block;
use super::url;
use crate::compose::ComposedReview;
use crate::model::{Evidence, Finding, Location};
use crate::session::{Origin, Session, Target, Verdict};

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

/// `form` carries what the Reviewer submitted when a ruling was rejected, so it is shown again instead of the saved values.
pub fn finding(s: &Session, idx: usize, error: Option<&str>, form: Option<&RulingForm>) -> Markup {
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
            (ruling_form(s, idx, form))
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

fn ruling_form(s: &Session, idx: usize, form: Option<&RulingForm>) -> Markup {
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
    let (current, comment, reason, escalate_to) = match form {
        Some(f) => (
            Some(f.verdict.as_str()).filter(|v| !v.is_empty()),
            f.comment.as_str(),
            f.reason.as_str(),
            f.escalate_to.as_str(),
        ),
        None => (current, comment, reason, escalate_to),
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

/// `form` carries what the Reviewer submitted when a submit was rejected, so it is shown again.
pub fn submit(
    s: &Session,
    preview: Option<&ComposedReview>,
    stale: bool,
    error: Option<&str>,
    form: Option<&SubmitForm>,
) -> Markup {
    let t = &s.token;
    let unruled = s.unruled();
    let has_block = s.entries.iter().any(|e| e.ruling.as_ref().is_some_and(|r| r.verdict == Verdict::Block));
    let own_pr = matches!(s.target, Target::Github { own_pr: true, .. });
    let (destination, button) = match &s.target {
        Target::Local { .. } => ("Local mode: nothing is posted.".to_string(), "Submit review"),
        Target::Github { repo, number, pending: true, .. } => {
            (format!("Creates a pending review on {repo}#{number} that you finish on GitHub."), "Create pending review")
        }
        Target::Github { repo, number, .. } => (format!("Posts the Review to {repo}#{number} on GitHub."), "Submit review to GitHub"),
    };
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
                    label { input type="checkbox" name="ack_stale" checked[form.is_some_and(|f| f.ack_stale.is_some())]; " I understand; submit anyway" }
                }
                label { "Summary (top of the Review)" textarea name="summary" rows="4" { (form.map_or("", |f| f.summary.as_str())) } }
                label {
                    input type="checkbox" name="approve" checked[form.is_some_and(|f| f.approve.is_some())] disabled[has_block || own_pr];
                    " Approve"
                    @if has_block { " (not available: something is Blocked)" }
                }
                @if own_pr {
                    p.hint { "This is your own pull request. GitHub only accepts a comment review on it, so Blocks are posted as comments marked Blocking, and Approve is not available." }
                }
                p.hint { (destination) }
                p.hint { "If every Finding is dismissed: tick Approve to approve, write a summary to post a comment, or leave both empty to post nothing." }
                @if let Some(path) = &s.export_md { p.hint { "The Review will be written to " code { (path.display()) } "." } }
                button type="submit" { (button) }
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
