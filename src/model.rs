//! The findings file an Agent hands to `prr review`.

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
