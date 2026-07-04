//! Plan option parser for the Plan-mode approval popup.

use std::sync::OnceLock;

/// A candidate option extracted from a plan markdown document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanOption {
    /// Reassigned label letter in document order (`A`, `B`, ...).
    pub label: char,
    /// Summary text taken from the heading after the colon.
    pub summary: String,
}

/// All actions a user may take in the plan approval popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanApprovalChoice {
    /// Approve a specific candidate option and implement it.
    ApproveOption {
        label: char,
        summary: String,
        clear_context: bool,
    },
    /// Approve the whole plan (fallback when no candidate options are present).
    Implement { clear_context: bool },
    /// Return feedback and keep planning.
    Revise { feedback: String },
    /// Reject the plan and switch back to Default mode.
    Reject,
    /// Dismiss the popup and continue planning.
    ContinuePlanning,
}

/// Parse candidate options from plan markdown.
///
/// Recognises `## Option <letter>` headings case-insensitively. Original
/// letters are ignored; labels are reassigned as `A`, `B`, ... in document
/// order. The summary is the text after the first colon, trimmed; if there is
/// no colon the summary is empty.
pub fn parse_plan_options(markdown: &str) -> Vec<PlanOption> {
    let mut options = Vec::new();
    for line in markdown.lines() {
        if options.len() >= 26 {
            break;
        }
        if let Some((_original_letter, rest)) = parse_option_heading(line) {
            let summary = if let Some(pos) = rest.find(':') {
                rest[pos + 1..].trim()
            } else {
                rest.trim()
            };
            let label = char::from_u32('A' as u32 + options.len() as u32).unwrap_or('A');
            options.push(PlanOption {
                label,
                summary: summary.to_string(),
            });
        }
    }
    options
}

fn parse_option_heading(line: &str) -> Option<(char, &str)> {
    static RE: OnceLock<regex_lite::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex_lite::Regex::new(r"(?i)^##\s+option\s+([a-z])\b(.*)$").unwrap()
    });
    let caps = re.captures(line.trim_end())?;
    let letter = caps.get(1)?.as_str().chars().next()?;
    let rest = caps.get(2)?.as_str();
    Some((letter, rest))
}

/// Convert an approval choice into a handoff-message suffix.
///
/// Returns `None` for choices that do not need an extra suffix.
pub fn plan_choice_handoff_suffix(choice: &PlanApprovalChoice) -> Option<String> {
    match choice {
        PlanApprovalChoice::ApproveOption { label, summary, .. } => {
            if summary.is_empty() {
                Some(format!("Execute Option {label} only."))
            } else {
                Some(format!("Execute Option {label} only: {summary}."))
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_option_extracts_label_and_summary() {
        let opts = parse_plan_options("## Option A: Refactor incrementally\n- step 1");
        assert_eq!(opts, vec![PlanOption {
            label: 'A',
            summary: "Refactor incrementally".to_string(),
        }]);
    }

    #[test]
    fn parse_reassigns_labels_by_document_order() {
        let opts = parse_plan_options("## Option C: First\n## Option A: Second");
        assert_eq!(opts[0].label, 'A');
        assert_eq!(opts[0].summary, "First");
        assert_eq!(opts[1].label, 'B');
        assert_eq!(opts[1].summary, "Second");
    }

    #[test]
    fn parse_is_case_insensitive() {
        let opts = parse_plan_options("## option b: Lowercase\n## OPTION A: Uppercase");
        assert_eq!(opts[0].summary, "Lowercase");
        assert_eq!(opts[1].summary, "Uppercase");
    }

    #[test]
    fn parse_ignores_non_option_headings() {
        let opts = parse_plan_options("# Plan\n## Summary\n## Option A: Real");
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].summary, "Real");
    }

    #[test]
    fn parse_returns_empty_for_plain_plan() {
        assert!(parse_plan_options("- Step 1\n- Step 2").is_empty());
    }

    #[test]
    fn parse_respects_word_boundary() {
        // must-reject: "Alpha" is not a single-letter option label
        assert!(parse_plan_options("## Option Alpha: Not an option").is_empty());
    }

    #[test]
    fn parse_ignores_plural_options_heading() {
        // must-reject: no letter after "Options"
        assert!(parse_plan_options("## Options\n- item").is_empty());
    }

    #[test]
    fn parse_keeps_colons_inside_summary() {
        let opts = parse_plan_options("## Option A: Refactor : with colon");
        assert_eq!(opts[0].summary, "Refactor : with colon");
    }

    #[test]
    fn parse_caps_at_26_labels() {
        let markdown: String = (0..30)
            .map(|i| format!("## Option {}: item {i}\n", char::from_u32('A' as u32 + (i % 26)).unwrap()))
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(parse_plan_options(&markdown).len(), 26);
    }

    #[test]
    fn handoff_suffix_for_approve_option_includes_label_and_summary() {
        let choice = PlanApprovalChoice::ApproveOption {
            label: 'B',
            summary: "Migrate DB".to_string(),
            clear_context: false,
        };
        assert_eq!(
            plan_choice_handoff_suffix(&choice),
            Some("Execute Option B only: Migrate DB.".to_string())
        );
    }

    #[test]
    fn handoff_suffix_for_approve_option_without_summary_omits_colon() {
        let choice = PlanApprovalChoice::ApproveOption {
            label: 'A',
            summary: "".to_string(),
            clear_context: false,
        };
        assert_eq!(
            plan_choice_handoff_suffix(&choice),
            Some("Execute Option A only.".to_string())
        );
    }

    #[test]
    fn handoff_suffix_for_implement_returns_none() {
        assert_eq!(
            plan_choice_handoff_suffix(&PlanApprovalChoice::Implement { clear_context: false }),
            None
        );
    }
}
