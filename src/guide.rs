//! The agent-facing usage guide printed by `prr guide`.

use crate::model::example_batch;

pub fn guide() -> String {
    let example = serde_json::to_string_pretty(&example_batch()).expect("example serializes");
    include_str!("guide.md").replace("{{EXAMPLE}}", &example)
}


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
