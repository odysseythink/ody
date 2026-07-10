use ody_config::config_toml::{PlanModeConfigToml, PlanModeTier};
use regex_lite::Regex;
use std::sync::OnceLock;

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

    pub fn select_tier(&self, _user_prompt: &str) -> PlanModeTierSelection {
        // Config override takes precedence.
        if let Some(tier) = self.config.and_then(|c| c.tier) {
            return PlanModeTierSelection {
                tier,
                rationale: format!("config override: {}", tier_name(tier)),
            };
        }

        // Plan mode always generates detailed plans (no heuristic "small task exception").
        // This ensures: (1) predictability — users know plans are always generated,
        // (2) traceability — plan files serve as audit records, (3) consistency with
        // ody-code's plan-mode semantics. Config can still override via tier setting.
        PlanModeTierSelection {
            tier: PlanModeTier::Rigor,
            rationale: "plan mode: always generate detailed plan (heuristic tier selection removed)"
                .to_string(),
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
    fn config_override_concise_still_respected() {
        let config = PlanModeConfigToml {
            tier: Some(PlanModeTier::Concise),
            ..Default::default()
        };
        let selector = PlanModeTierSelector::new(Some(&config));
        let selection = selector.select_tier("any prompt");
        assert_eq!(selection.tier, PlanModeTier::Concise);
        assert!(selection.rationale.contains("config override"));
    }

    #[test]
    fn config_override_rigor_still_respected() {
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
    fn no_config_always_returns_rigor() {
        let selector = PlanModeTierSelector::new(None);
        // Test with small prompt that would have triggered Concise before.
        let selection = selector.select_tier("fix typo");
        assert_eq!(selection.tier, PlanModeTier::Rigor);
        assert!(selection.rationale.contains("always generate detailed plan"));
    }

    #[test]
    fn no_config_large_prompt_still_rigor() {
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
        assert!(selection.rationale.contains("always generate detailed plan"));
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
        assert_eq!(parse_tier_switch_command("/plan-tier rigor\nmore text"), None);
    }
}
