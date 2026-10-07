//! The JSON printed on stdout when a Review session ends — the Agent's view of the Reviewer's decisions.

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
