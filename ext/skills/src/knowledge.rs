use ody_core_skills::SkillType;
use ody_protocol::config_types::ModeKind;
use regex::Regex;

use crate::catalog::SkillCatalog;
use crate::catalog::SkillCatalogEntry;

/// Selects knowledge skills whose triggers match the user text.
pub(crate) struct KnowledgeMicroagentInjector;

impl KnowledgeMicroagentInjector {
    /// Returns knowledge catalog entries whose triggers appear in `text`, up to
    /// `max_skills` entries and `max_contents_bytes` of selected skill contents
    /// (name + description bytes).
    ///
    /// Only enabled knowledge skills that are not hidden in `mode` are
    /// considered. Empty triggers never match (such skills behave as inline
    /// skills at load time and are not eligible here).
    pub(crate) fn select(
        text: &str,
        catalog: &SkillCatalog,
        mode: ModeKind,
        max_skills: usize,
        max_contents_bytes: usize,
    ) -> Vec<SkillCatalogEntry> {
        if max_skills == 0 || text.is_empty() || max_contents_bytes == 0 {
            return Vec::new();
        }

        let mut selected = Vec::new();
        let mut total_bytes = 0usize;

        for entry in catalog.entries.iter().filter(|entry| {
            entry.enabled
                && entry.skill_type == SkillType::Knowledge
                && !entry.hidden_in_modes.contains(&mode)
        }) {
            if selected.len() >= max_skills {
                break;
            }
            if entry.triggers.is_empty() {
                continue;
            }
            if !entry
                .triggers
                .iter()
                .any(|trigger| trigger_matches(text, trigger))
            {
                continue;
            }

            let entry_bytes = entry.name.len() + entry.description.len();
            let next_bytes = total_bytes.saturating_add(entry_bytes);
            if next_bytes > max_contents_bytes {
                continue;
            }
            total_bytes = next_bytes;
            selected.push(entry.clone());
        }

        selected
    }
}

fn trigger_matches(text: &str, trigger: &str) -> bool {
    if trigger.is_empty() {
        return false;
    }

    let text = text.to_lowercase();
    let trigger = trigger.to_lowercase();

    if trigger.chars().any(is_cjk) {
        text.contains(&trigger)
    } else {
        let pattern = format!(r"(?i)\b{}\b", regex::escape(&trigger));
        Regex::new(&pattern)
            .map(|re| re.is_match(&text))
            .unwrap_or(false)
    }
}

fn is_cjk(c: char) -> bool {
    matches!(
        c,
        '\u{4E00}'..='\u{9FFF}'     // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}'     // CJK Unified Ideographs Extension A
        | '\u{AC00}'..='\u{D7AF}'     // Hangul Syllables
        | '\u{3040}'..='\u{309F}'     // Hiragana
        | '\u{30A0}'..='\u{30FF}'     // Katakana
        | '\u{FF00}'..='\u{FFEF}'     // Halfwidth and Fullwidth Forms
        | '\u{3100}'..='\u{312F}'     // Bopomofo
        | '\u{31A0}'..='\u{31BF}'     // Bopomofo Extended
        | '\u{3000}'..='\u{303F}'     // CJK Symbols and Punctuation
    )
}

#[cfg(test)]
mod tests {
    use ody_core_skills::SkillType;
    use ody_protocol::config_types::ModeKind;

    use super::KnowledgeMicroagentInjector;
    use crate::catalog::SkillAuthority;
    use crate::catalog::SkillCatalog;
    use crate::catalog::SkillPackageId;
    use crate::catalog::SkillResourceId;
    use crate::catalog::SkillSourceKind;

    fn knowledge_entry(name: &str, triggers: &[&str]) -> crate::catalog::SkillCatalogEntry {
        crate::catalog::SkillCatalogEntry::new(
            SkillPackageId(name.to_string()),
            SkillAuthority::new(SkillSourceKind::Host, "host"),
            name,
            format!("{name} description"),
            SkillResourceId::new(format!("{name}/SKILL.md")),
        )
        .with_skill_type(SkillType::Knowledge)
        .with_triggers(triggers.iter().map(|&s| s.to_string()).collect())
    }

    fn catalog(entries: Vec<crate::catalog::SkillCatalogEntry>) -> SkillCatalog {
        SkillCatalog {
            entries,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn ascii_trigger_matches_whole_word() {
        let entry = knowledge_entry("review", &["review"]);
        let selected = KnowledgeMicroagentInjector::select(
            "please review this code",
            &catalog(vec![entry]),
            ModeKind::Default,
            3,
            8_000,
        );
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "review");
    }

    #[test]
    fn ascii_trigger_matches_case_insensitively() {
        let entry = knowledge_entry("review", &["review"]);
        let selected = KnowledgeMicroagentInjector::select(
            "Please Review this code",
            &catalog(vec![entry]),
            ModeKind::Default,
            3,
            8_000,
        );
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "review");
    }

    #[test]
    fn ascii_trigger_does_not_match_subword() {
        let entry = knowledge_entry("review", &["review"]);
        let selected = KnowledgeMicroagentInjector::select(
            "code reviewer",
            &catalog(vec![entry]),
            ModeKind::Default,
            3,
            8_000,
        );
        assert!(selected.is_empty());
    }

    #[test]
    fn cjk_trigger_matches_substring_case_insensitively() {
        let entry = knowledge_entry("review", &["审查"]);
        let selected = KnowledgeMicroagentInjector::select(
            "请审查这段代码",
            &catalog(vec![entry]),
            ModeKind::Default,
            3,
            8_000,
        );
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "review");
    }

    #[test]
    fn empty_trigger_downgrades_to_inline_and_does_not_match() {
        let entry = knowledge_entry("inline", &[]);
        let selected = KnowledgeMicroagentInjector::select(
            "inline trigger",
            &catalog(vec![entry]),
            ModeKind::Default,
            3,
            8_000,
        );
        assert!(selected.is_empty());
    }

    #[test]
    fn hidden_in_mode_excludes_knowledge_skill() {
        let entry =
            knowledge_entry("plan-skill", &["plan"]).with_hidden_in_modes(vec![ModeKind::Plan]);
        let selected = KnowledgeMicroagentInjector::select(
            "use plan skill",
            &catalog(vec![entry]),
            ModeKind::Plan,
            3,
            8_000,
        );
        assert!(selected.is_empty());
    }

    #[test]
    fn budget_caps_number_of_selected_skills() {
        let entries = vec![
            knowledge_entry("one", &["one"]),
            knowledge_entry("two", &["two"]),
            knowledge_entry("three", &["three"]),
        ];
        let selected = KnowledgeMicroagentInjector::select(
            "one two three",
            &catalog(entries),
            ModeKind::Default,
            1,
            8_000,
        );
        assert_eq!(selected.len(), 1);
    }

    #[test]
    fn budget_caps_contents_bytes() {
        let mut large = knowledge_entry("large", &["large"]);
        large.description = "x".repeat(1_000);
        let entries = vec![large.clone(), knowledge_entry("small", &["small"])];
        // The large entry consumes more than half the budget, so only it fits.
        let selected = KnowledgeMicroagentInjector::select(
            "large small",
            &catalog(entries),
            ModeKind::Default,
            10,
            1_010,
        );
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "large");
    }
}
