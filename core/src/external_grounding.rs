//! Completeness gate for external evidence in finalized Design and Plan artifacts.
//!
//! The prompt decides *when* research is required; this module ensures that the model records that
//! decision and does not finalize a recommendation with an implicit appeal to model memory.

use std::collections::BTreeSet;

const HEADING_EN: &str = "external evidence";
const HEADING_ZH: &str = "外部证据";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvidenceStatus {
    Required,
    NotRequired,
}

pub(crate) fn external_evidence_report(content: &str) -> Option<String> {
    let section_count = content
        .lines()
        .filter(|line| is_external_evidence_heading(line.trim()))
        .count();
    if section_count > 1 {
        return Some(format!(
            "Found {section_count} External Evidence sections. Keep exactly one declaration in the finalized artifact."
        ));
    }
    let Some(section) = external_evidence_section(content) else {
        return Some(
            "External evidence declaration is missing. Add `## External Evidence` with `Status: Required` or `Status: Not required` and a task-specific `Reason:`.".to_string(),
        );
    };

    let Some(status) = evidence_status(section) else {
        return Some(
            "External Evidence has no valid status. Use exactly `Status: Required` or `Status: Not required`.".to_string(),
        );
    };

    let Some(reason) = evidence_reason(section) else {
        return Some(
            "External Evidence needs a concrete task-specific `Reason:`; placeholders such as N/A, none, TBD, or 待定 are not accepted.".to_string(),
        );
    };
    if !is_concrete_reason(reason) {
        return Some(
            "External Evidence `Reason:` is empty or only a placeholder. Explain why external research is or is not needed for this task.".to_string(),
        );
    }

    if status == EvidenceStatus::NotRequired {
        return None;
    }

    let urls = external_urls(section);
    if urls.len() < 2 {
        return Some(format!(
            "External research is Required, but the evidence section contains only {} distinct external URL(s); add at least two original-source URLs.",
            urls.len()
        ));
    }

    let normalized = section.to_ascii_lowercase();
    let has_required_columns = ["claim", "source", "version/date", "decision impact"]
        .iter()
        .all(|column| normalized.contains(column))
        || ["主张", "来源", "版本/日期", "决策影响"]
            .iter()
            .all(|column| section.contains(column));
    if !has_required_columns {
        return Some(
            "External research is Required. Add an evidence table with `Claim`, `Source`, `Version/date`, and `Decision impact` columns (or their Chinese equivalents).".to_string(),
        );
    }

    None
}

fn external_evidence_section(content: &str) -> Option<&str> {
    let mut start = None;
    let mut end = content.len();
    let mut offset = 0;

    for line in content.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            if start.is_some() {
                end = offset;
                break;
            }
            if is_external_evidence_heading(trimmed) {
                start = Some(offset + line.len());
            }
        }
        offset += line.len();
    }

    start.map(|start| &content[start.min(content.len())..end])
}

fn is_external_evidence_heading(line: &str) -> bool {
    line.strip_prefix("## ").is_some_and(|title| {
        let title = title.trim().to_ascii_lowercase();
        title == HEADING_EN || title == HEADING_ZH
    })
}

fn evidence_status(section: &str) -> Option<EvidenceStatus> {
    for line in section.lines() {
        let normalized = line
            .trim()
            .trim_start_matches(['-', '*'])
            .trim()
            .to_ascii_lowercase();
        if matches!(normalized.as_str(), "status: required" | "status：required") {
            return Some(EvidenceStatus::Required);
        }
        if matches!(
            normalized.as_str(),
            "status: not required" | "status：not required"
        ) {
            return Some(EvidenceStatus::NotRequired);
        }
        let original = line.trim().trim_start_matches(['-', '*']).trim();
        if matches!(original, "状态: 需要" | "状态：需要") {
            return Some(EvidenceStatus::Required);
        }
        if matches!(original, "状态: 不需要" | "状态：不需要") {
            return Some(EvidenceStatus::NotRequired);
        }
    }
    None
}

fn evidence_reason(section: &str) -> Option<&str> {
    section.lines().find_map(|line| {
        let line = line.trim().trim_start_matches(['-', '*']).trim();
        [
            "Reason:",
            "Reason：",
            "reason:",
            "reason：",
            "原因:",
            "原因：",
        ]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix).map(str::trim))
    })
}

fn is_concrete_reason(reason: &str) -> bool {
    let normalized = reason.trim().trim_matches(['.', '。']).to_ascii_lowercase();
    if normalized.chars().count() < 12 {
        return false;
    }
    !matches!(
        normalized.as_str(),
        "n/a" | "na" | "none" | "not required" | "not needed" | "tbd" | "todo" | "待定" | "不需要"
    )
}

fn external_urls(section: &str) -> BTreeSet<String> {
    let mut urls = BTreeSet::new();
    for scheme in ["https://", "http://"] {
        let mut remainder = section;
        while let Some(index) = remainder.find(scheme) {
            let candidate = &remainder[index..];
            let end = candidate
                .find(|ch: char| {
                    ch.is_whitespace() || matches!(ch, '|' | ')' | ']' | '>' | ',' | '，')
                })
                .unwrap_or(candidate.len());
            let url = candidate[..end]
                .trim_end_matches(['.', ';', ':', '。', '；'])
                .to_string();
            if url.len() > scheme.len() {
                urls.insert(url);
            }
            remainder = &candidate[end..];
            if end == 0 {
                break;
            }
        }
    }
    urls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_section_is_rejected() {
        assert!(external_evidence_report("## Scope\nLocal change").is_some());
    }

    #[test]
    fn duplicate_sections_are_rejected() {
        let declaration = "## External Evidence\nStatus: Not required\nReason: This repository-local task is fully specified by checked-in source and tests.\n";
        assert!(external_evidence_report(&format!("{declaration}\n{declaration}")).is_some());
    }

    #[test]
    fn concrete_not_required_declaration_passes() {
        let artifact = "## External Evidence\nStatus: Not required\nReason: This is a repository-local rename whose callers and behavior are fully defined by checked-in source and tests.\n";
        assert_eq!(external_evidence_report(artifact), None);
    }

    #[test]
    fn repository_local_plan_fixture_passes() {
        let artifact = "# Test Plan\n- Step 1\n- Step 2\n\n## External Evidence\nStatus: Not required\nReason: This test plan exercises repository-local behavior fully defined by checked-in code and test fixtures.\n";
        assert_eq!(external_evidence_report(artifact), None);
    }

    #[test]
    fn placeholder_not_required_reason_is_rejected() {
        let artifact = "## External Evidence\nStatus: Not required\nReason: not required\n";
        assert!(external_evidence_report(artifact).is_some());
    }

    #[test]
    fn required_research_needs_two_urls_and_table_contract() {
        let one_source = "## External Evidence\nStatus: Required\nReason: The recommendation compares current third-party APIs and therefore depends on external versioned facts.\n| Claim | Source | Version/date | Decision impact |\n|---|---|---|---|\n| API | https://example.com/docs | 2026 | choose A |\n";
        assert!(external_evidence_report(one_source).is_some());

        let two_sources = format!(
            "{one_source}| Practice | https://github.com/example/project | v2 | add fallback |\n"
        );
        assert_eq!(external_evidence_report(&two_sources), None);
    }

    #[test]
    fn chinese_declaration_and_columns_pass() {
        let artifact = "## 外部证据\n状态：需要\n原因：该技术选型依赖当前版本的官方接口以及成熟开源项目的实际做法。\n| 主张 | 来源 | 版本/日期 | 决策影响 |\n|---|---|---|---|\n| 官方接口 | https://example.com/docs | 2026 | 采用接口 |\n| 开源实践 | https://github.com/example/project | v2 | 增加回退 |\n";
        assert_eq!(external_evidence_report(artifact), None);
    }
}
