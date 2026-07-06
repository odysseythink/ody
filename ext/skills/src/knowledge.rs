use ody_core_skills::SkillType;
use ody_protocol::config_types::ModeKind;
use regex::Regex;

use crate::catalog::SkillCatalog;
use crate::catalog::SkillCatalogEntry;

/// Selects knowledge skills whose triggers match the user text.
pub struct KnowledgeMicroagentInjector;

impl KnowledgeMicroagentInjector {
    /// Returns knowledge catalog entries whose triggers appear in `text`, up to
    /// `max_skills` entries and `max_chars` of matched trigger characters.
    ///
    /// Only enabled knowledge skills that are not hidden in `mode` are
    /// considered. Empty triggers never match (such skills behave as inline
    /// skills at load time and are not eligible here).
    pub fn select(
        text: &str,
        catalog: &SkillCatalog,
        mode: ModeKind,
        max_skills: usize,
        max_chars: usize,
    ) -> Vec<SkillCatalogEntry> {
        let mut selected = Vec::new();
        let mut consumed_chars = 0usize;

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

            let trigger_chars = entry
                .triggers
                .iter()
                .map(|trigger| trigger.chars().count())
                .sum::<usize>();
            let next_consumed = consumed_chars.saturating_add(trigger_chars);
            if next_consumed > max_chars {
                continue;
            }
            consumed_chars = next_consumed;
            selected.push(entry.clone());
        }

        selected
    }
}

fn trigger_matches(text: &str, trigger: &str) -> bool {
    if trigger.is_empty() {
        return false;
    }
    if trigger.chars().any(is_cjk) {
        text.contains(trigger)
    } else {
        let pattern = format!(r"\b{}\b", regex::escape(trigger));
        Regex::new(&pattern)
            .map(|re| re.is_match(text))
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
    fn cjk_trigger_matches_substring() {
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
            10,
            8,
        );
        assert_eq!(selected.len(), 2);
    }
}
