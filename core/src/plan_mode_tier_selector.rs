use ody_config::config_toml::{PlanModeConfigToml, PlanModeTier};
use regex_lite::Regex;
use std::sync::OnceLock;

/// Threshold above which the heuristic selects the rigor tier.
const RISK_THRESHOLD: usize = 60;

const HIGH_IMPACT_VERBS: &[&str] = &[
    "refactor",
    "migrate",
    "remove concept",
    "delete",
    "rename",
    "redesign",
    "extract",
    "merge",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanModeTierSelection {
    pub tier: PlanModeTier,
    pub rationale: String,
}

pub struct PlanModeTierSelector<'a> {
    config: Option<&'a PlanModeConfigToml>,
}

impl<'a> PlanModeTierSelector<'a> {
    pub fn new(config: Option<&'a PlanModeConfigToml>) -> Self {
        Self { config }
    }

    pub fn select_tier(&self, user_prompt: &str) -> PlanModeTierSelection {
        // Config override takes precedence.
        if let Some(tier) = self.config.and_then(|c| c.tier) {
            return PlanModeTierSelection {
                tier,
                rationale: format!("config override: {}", tier_name(tier)),
            };
        }

        let threshold = self
            .config
            .and_then(|c| c.split_threshold)
            .unwrap_or(8);

        match std::panic::catch_unwind(|| heuristic_score(user_prompt, threshold)) {
            Ok((score, factors)) => {
                if score >= RISK_THRESHOLD {
                    PlanModeTierSelection {
                        tier: PlanModeTier::Rigor,
                        rationale: format!(
                            "auto: score={} >= {} ({})",
                            score, RISK_THRESHOLD, factors
                        ),
                    }
                } else {
                    PlanModeTierSelection {
                        tier: PlanModeTier::Concise,
                        rationale: format!(
                            "auto: score={} < {} ({})",
                            score, RISK_THRESHOLD, factors
                        ),
                    }
                }
            }
            Err(_) => PlanModeTierSelection {
                tier: PlanModeTier::Concise,
                rationale: "scorer failed; falling back to concise".to_string(),
            },
        }
    }
}

fn tier_name(tier: PlanModeTier) -> &'static str {
    match tier {
        PlanModeTier::Auto => "auto",
        PlanModeTier::Concise => "concise",
        PlanModeTier::Rigor => "rigor",
    }
}

fn heuristic_score(prompt: &str, threshold: usize) -> (usize, String) {
    let mut score = 0usize;
    let mut factors: Vec<String> = Vec::new();

    // F1 — explicit task count.
    let task_count = estimate_task_count(prompt);
    if task_count > threshold {
        score += 40;
        factors.push(format!("tasks={}>threshold={}", task_count, threshold));
    } else if task_count > threshold / 2 {
        score += 20;
        factors.push(format!("tasks={}>threshold/2", task_count));
    }

    // F2 — subsystem/crate breadth.
    let crate_count = count_distinct_crates_or_modules(prompt);
    if crate_count >= 3 {
        score += 25;
        factors.push(format!("crates={}>=3", crate_count));
    } else if crate_count == 2 {
        score += 10;
        factors.push("crates=2".to_string());
    }

    // F3 — high-impact action verbs.
    let high_impact_matches = count_high_impact_verbs(prompt);
    if high_impact_matches >= 2 {
        score += 40;
        factors.push(format!("high_impact_matches={}", high_impact_matches));
    } else if high_impact_matches == 1 {
        score += 25;
        factors.push("high_impact_match=1".to_string());
    }

    // F4 — concept removal/rename is high-risk and needs rigor even for short prompts.
    let lower = prompt.to_lowercase();
    let concept_words = ["concept", "概念"];
    let has_concept = concept_words.iter().any(|w| lower.contains(w));
    let removal_words = [
        "remove", "delete", "rename", "refactor",
        "拿掉", "移除", "删除", "删掉", "重命名", "重构",
    ];
    let has_removal = removal_words.iter().any(|w| lower.contains(w));
    if has_concept && has_removal {
        score += 60;
        factors.push("concept_remove_or_rename".to_string());
    }

    // F5 — prompt length (cheap byte-length proxy).
    let token_estimate = prompt.len() / 4;
    if token_estimate > 512 {
        score += 15;
        factors.push(format!("tokens~{}>512", token_estimate));
    } else if token_estimate > 256 {
        score += 5;
        factors.push(format!("tokens~{}>256", token_estimate));
    }

    let factors = if factors.is_empty() {
        "no risk factors".to_string()
    } else {
        factors.join(", ")
    };

    (score, factors)
}

fn estimate_task_count(prompt: &str) -> usize {
    static NUMBERED_RE: OnceLock<Regex> = OnceLock::new();
    static SECTION_RE: OnceLock<Regex> = OnceLock::new();

    let numbered_re = NUMBERED_RE.get_or_init(|| {
        Regex::new(r"(?m)^\s*(\d+[.):-]|\*|\-)\s+\S+").unwrap()
    });
    let section_re = SECTION_RE.get_or_init(|| {
        Regex::new(r"(?i)(phase|step|part|section|task|goal)").unwrap()
    });

    let numbered = numbered_re.find_iter(prompt).count();
    let sections = section_re.find_iter(prompt).count();
    numbered.max(sections).max(1)
}

fn count_distinct_crates_or_modules(prompt: &str) -> usize {
    static CRATE_RE: OnceLock<Regex> = OnceLock::new();
    let crate_re = CRATE_RE.get_or_init(|| {
        Regex::new(r"(?i)([a-z0-9][a-z0-9_-]*)(?:/|::)").unwrap()
    });

    let mut crates = std::collections::HashSet::new();
    for cap in crate_re.captures_iter(prompt) {
        let name = cap[1].to_lowercase();
        // Exclude common prose words that happen to be followed by punctuation we can't anchor.
        if !is_common_non_crate_word(&name) {
            crates.insert(name);
        }
    }
    crates.len()
}

fn is_common_non_crate_word(word: &str) -> bool {
    const COMMON: &[&str] = &[
        "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for", "of", "with",
        "from", "as", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "can", "shall",
        "this", "that", "these", "those", "i", "you", "he", "she", "it", "we", "they",
        "my", "your", "his", "her", "its", "our", "their", "what", "which", "who", "when",
        "where", "why", "how", "all", "each", "every", "both", "few", "more", "most", "other",
        "some", "such", "no", "not", "only", "own", "same", "so", "than", "too", "very",
    ];
    COMMON.binary_search(&word).is_ok()
}

fn count_high_impact_verbs(prompt: &str) -> usize {
    let lower = prompt.to_lowercase();
    HIGH_IMPACT_VERBS
        .iter()
        .filter(|verb| lower.contains(*verb))
        .count()
}

/// Parses an explicit tier-switch command from the start of a user message.
///
/// Only exact commands anchored to the whole message are recognized, preventing
/// accidental triggers inside plan prose.
pub fn parse_tier_switch_command(user_message: &str) -> Option<PlanModeTier> {
    static SWITCH_RE: OnceLock<Regex> = OnceLock::new();
    let switch_re = SWITCH_RE.get_or_init(|| {
        Regex::new(r"(?i)^\s*/plan-tier\s+(rigor|concise|auto)\s*$").unwrap()
    });

    switch_re.captures(user_message).and_then(|cap| {
        match &cap[1].to_lowercase()[..] {
            "rigor" => Some(PlanModeTier::Rigor),
            "concise" => Some(PlanModeTier::Concise),
            "auto" => Some(PlanModeTier::Auto),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_override_concise_ignores_heuristic() {
        let config = PlanModeConfigToml {
            tier: Some(PlanModeTier::Concise),
            ..Default::default()
        };
        let selector = PlanModeTierSelector::new(Some(&config));
        let selection = selector.select_tier(
            "Refactor auth, migrate payments, remove concept of accounts, delete legacy schema"
        );
        assert_eq!(selection.tier, PlanModeTier::Concise);
        assert!(selection.rationale.contains("config override"));
    }

    #[test]
    fn config_override_rigor_ignores_heuristic() {
        let config = PlanModeConfigToml {
            tier: Some(PlanModeTier::Rigor),
            ..Default::default()
        };
        let selector = PlanModeTierSelector::new(Some(&config));
        let selection = selector.select_tier("fix typo");
        assert_eq!(selection.tier, PlanModeTier::Rigor);
        assert!(selection.rationale.contains("config override"));
    }

    #[test]
    fn auto_selects_rigor_for_many_tasks() {
        let selector = PlanModeTierSelector::new(None);
        let prompt = r#"
1. Refactor auth module
2. Migrate payments
3. Remove account concept from analytics
4. Delete legacy schema
5. Rename user_id fields
6. Extract account helpers
7. Merge account types
8. Redesign session flow
9. Update connectors/ody-mcp
10. Sweep insta snapshots
"#;
        let selection = selector.select_tier(prompt);
        assert_eq!(selection.tier, PlanModeTier::Rigor);
        assert!(selection.rationale.contains("score="));
    }

    #[test]
    fn auto_selects_concise_for_small_prompt() {
        let selector = PlanModeTierSelector::new(None);
        let selection = selector.select_tier("fix typo in README");
        assert_eq!(selection.tier, PlanModeTier::Concise);
    }

    #[test]
    fn high_impact_verb_boosts_score() {
        let selector = PlanModeTierSelector::new(None);
        let prompt = r#"
1. Rename foo
2. Extract bar
3. Merge baz
4. Update docs
5. Add tests
Also refactor and migrate the subsystem.
"#;
        let selection = selector.select_tier(prompt);
        assert_eq!(selection.tier, PlanModeTier::Rigor);
        assert!(selection.rationale.contains("high_impact"));
    }

    #[test]
    fn scorer_panic_falls_back_to_concise() {
        let selector = PlanModeTierSelector::new(None);
        let selection = selector.select_tier("");
        assert_eq!(selection.tier, PlanModeTier::Concise);
    }

    #[test]
    fn parse_tier_switch_command_matches_valid_commands() {
        assert_eq!(parse_tier_switch_command("/plan-tier rigor"), Some(PlanModeTier::Rigor));
        assert_eq!(
            parse_tier_switch_command("  /plan-tier  concise  "),
            Some(PlanModeTier::Concise)
        );
        assert_eq!(parse_tier_switch_command("/plan-tier auto"), Some(PlanModeTier::Auto));
    }

    #[test]
    fn parse_tier_switch_command_ignores_non_commands() {
        assert_eq!(parse_tier_switch_command("Please /plan-tier rigor now"), None);
        assert_eq!(parse_tier_switch_command("/plan-tier unknown"), None);
        assert_eq!(parse_tier_switch_command("/plan-tier rigor extra"), None);
        assert_eq!(parse_tier_switch_command(""), None);
        assert_eq!(parse_tier_switch_command("/plan-tier rigor
more text"), None);
    }
}
