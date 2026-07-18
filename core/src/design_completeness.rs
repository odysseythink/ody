//! Design-mode C1–C8 completeness gate.
//!
//! Ports ody-code `findMissingDesignSections` (exit-design-mode.ts) so that the
//! Design→Plan handoff (D6) can verify a design artifact contains every required
//! section before allowing the switch. This module is a pure function set: it
//! owns no session state and performs no I/O. Wiring into the handoff lives in D6.
//!
use regex_lite::Regex;
use std::sync::OnceLock;

/// Minimum character length for a design document to be considered non-empty.
const MIN_CONTENT_LEN: usize = 300;

/// Minimum number of `## ` (level-2) sections expected in a complete design.
const MIN_HEADING_COUNT: usize = 3;

fn heading_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?m)^## ").expect("valid heading regex"))
}

// Every section check anchors its keyword to the start of a heading line
// (`^#{1,3}\s+`), so a keyword merely appearing in prose (or in a skeleton
// section's placeholder body) no longer satisfies the section. An optional
// enumerator prefix (`C1.`, `C2)`, `C3：` …) is tolerated so the template's own
// `## C2. Architecture / Design` heading style passes without forcing the model
// to rename to a bare `## Architecture` — the un-tolerated prefix previously
// caused spurious "missing section" rejections. `(?im)` keeps `^` per-line and
// matching case-insensitive.
const HEADING_PREFIX: &str = r"(?im)^#{1,3}\s+(?:c\d+[.):：]?\s+)?";

fn c1_scope() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r"{HEADING_PREFIX}(scope|in/out|范围)")).expect("valid C1 regex")
    })
}

fn c2_architecture() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"{HEADING_PREFIX}(architecture|design|approach|overview|架构|设计方案)"
        ))
        .expect("valid C2 regex")
    })
}

fn c3_data_models() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"{HEADING_PREFIX}(data\s*models?|数据模型|models?|data\s+&?\s*state)"
        ))
        .expect("valid C3 regex")
    })
}

fn c4_algorithms() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"{HEADING_PREFIX}(algorithms?|算法|pseudocode|implementation\s+notes?)"
        ))
        .expect("valid C4 regex")
    })
}

fn c5_error_handling() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"{HEADING_PREFIX}(error\s*handling|错误处理|errors?|degradation|failure\s+scenarios?)"
        ))
        .expect("valid C5 regex")
    })
}

fn c6_self_review() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Deliberately narrow: a bare `review` / `audit` heading is NOT accepted,
    // because a section merely named "Design Review" is not the adversarial
    // self-review this gate exists to force (mirrors ody-code's C6).
    RE.get_or_init(|| {
        Regex::new(&format!(r"{HEADING_PREFIX}(self[-\s]?review|自检)")).expect("valid C6 regex")
    })
}

fn c7_user_approval() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"{HEADING_PREFIX}(user\s+(?:final\s+)?approval|用户批准|批准状态|approved?)"
        ))
        .expect("valid C7 regex")
    })
}

fn c8_reuse_analysis() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"{HEADING_PREFIX}(reuse\s+analysis|复用分析|component\s+reuse|existing\s+components?)"
        ))
        .expect("valid C8 regex")
    })
}

/// Returns the human-readable names of every required C1–C8 design section that
/// is missing from `content`. An empty `Vec` means the design is complete.
///
/// The first two entries (when present) are structural preconditions — minimum
/// length and minimum `##` heading count — followed by any of the eight named
/// sections whose regex did not match. Order matches the upstream tool.
pub fn find_missing_design_sections(content: &str) -> Vec<String> {
    let mut missing = Vec::new();

    if content.len() < MIN_CONTENT_LEN {
        missing.push("sufficient content (design appears incomplete or empty)".to_string());
    }

    let heading_count = heading_regex().find_iter(content).count();
    if heading_count < MIN_HEADING_COUNT {
        missing.push(format!(
            "at least {MIN_HEADING_COUNT} design sections (found {heading_count})"
        ));
    }

    let checks: [(&Regex, &str); 8] = [
        (c1_scope(), "Scope or Scope In/Out section"),
        (c2_architecture(), "Architecture or Design section"),
        (c3_data_models(), "Data Models section"),
        (
            c4_algorithms(),
            "Algorithms or Implementation Notes section",
        ),
        (c5_error_handling(), "Error Handling section"),
        (c6_self_review(), "Self-Review section"),
        (c7_user_approval(), "User Approval section"),
        (c8_reuse_analysis(), "Reuse Analysis section"),
    ];

    for (re, name) in checks {
        if !re.is_match(content) {
            missing.push(name.to_string());
        }
    }

    missing
}

/// Convenience wrapper that produces a single user-facing message when the
/// design is incomplete, mirroring upstream `checkDesignCompleteness` copy.
///
/// Returns `None` when the design is complete; otherwise a message of the form
/// `"Design is incomplete. Missing:\n- …\n\nPlease add the missing sections before exiting Design mode."`.
pub fn design_completeness_report(content: &str) -> Option<String> {
    let missing = find_missing_design_sections(content);
    if missing.is_empty() {
        return None;
    }
    let bullets = missing
        .iter()
        .map(|name| format!("- {name}"))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "Design is incomplete. Missing:\n{bullets}\n\nPlease add the missing sections before exiting Design mode."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_document_reports_many_missing_sections() {
        let missing = find_missing_design_sections("");
        assert!(missing.iter().any(|m| m.contains("sufficient content")));
        assert!(
            missing
                .iter()
                .any(|m| m.contains("at least 3 design sections"))
        );
        for expected in [
            "Scope or Scope In/Out section",
            "Architecture or Design section",
            "Data Models section",
            "Algorithms or Implementation Notes section",
            "Error Handling section",
            "Self-Review section",
            "User Approval section",
            "Reuse Analysis section",
        ] {
            assert!(
                missing.iter().any(|m| m == expected),
                "expected missing entry {expected:?}; got {missing:?}"
            );
        }
    }

    #[test]
    fn complete_english_design_returns_no_missing_sections() {
        let design = concat!(
            "# Feature Design\n\n",
            "## Scope\n",
            "In scope: the core behaviour. Out of scope: the UI polish. ",
            "This line pads the document beyond the minimum content length so ",
            "the structural gate does not trip on an otherwise complete design.\n\n",
            "## Architecture\n",
            "The approach is to reuse the existing pipeline and add a stage.\n\n",
            "## Data Models\n",
            "struct DesignState {{ sections: Vec<String> }}\n\n",
            "## Algorithms\n",
            "implementation notes: walk the sections and tally coverage.\n\n",
            "## Error Handling\n",
            "failure scenarios and graceful degradation are handled inline.\n\n",
            "## Self-Review\n",
            "audit checklist reviewed against the rubric.\n\n",
            "## User Approval\n",
            "user final approval captured before handoff.\n\n",
            "## Reuse Analysis\n",
            "component reuse survey of existing components follows.\n",
        );
        assert!(
            find_missing_design_sections(design).is_empty(),
            "complete design must report no missing sections; got {:?}",
            find_missing_design_sections(design)
        );
    }

    #[test]
    fn complete_chinese_headings_pass() {
        let design = concat!(
            "# 功能设计\n\n",
            "## 范围\n",
            "范围内：核心行为。范围外：界面打磨。这一段用于把文档填充到最小内容长度以上，",
            "从而避免结构门在一个其它维度都完整的设计上误报。\n\n",
            "## 架构\n",
            "设计方案：复用现有流水线并新增一个阶段。\n\n",
            "## 数据模型\n",
            "struct DesignState {{ sections: Vec<String> }}\n\n",
            "## 算法\n",
            "implementation notes：遍历所有章节并统计覆盖。\n\n",
            "## 错误处理\n",
            "failure scenarios 与降级路径就地处理。\n\n",
            "## 自检\n",
            "audit 清单按 rubric 复核。\n\n",
            "## 用户批准\n",
            "切换前完成用户批准。\n\n",
            "## 复用分析\n",
            "existing components 复用盘点如下。\n",
        );
        assert!(
            find_missing_design_sections(design).is_empty(),
            "Chinese-heading design must pass; got {:?}",
            find_missing_design_sections(design)
        );
    }

    #[test]
    fn only_two_headings_reports_heading_count_gate() {
        let design = concat!(
            "# Feature Design\n\n",
            "## Scope\n",
            "This document has only two level-2 headings and otherwise enough ",
            "content to clear the length gate by a comfortable margin. Padding ",
            "padding padding padding padding padding padding padding padding.\n\n",
            "## Architecture\n",
            "approach overview design reuse pipeline.\n",
        );
        let missing = find_missing_design_sections(design);
        assert!(
            missing
                .iter()
                .any(|m| m.contains("at least 3 design sections (found 2)")),
            "expected heading-count gate with found=2; got {missing:?}"
        );
    }

    #[test]
    fn design_completeness_report_returns_none_when_complete() {
        let design = concat!(
            "# Feature Design\n\n",
            "## Scope\n",
            "Padding to clear the minimum content length gate for the report ",
            "wrapper test. padding padding padding padding padding padding ",
            "padding padding padding padding padding padding padding padding.\n\n",
            "## Architecture\n",
            "approach overview design reuse pipeline.\n\n",
            "## Data Models\n",
            "models data & state.\n\n",
            "## Algorithms\n",
            "pseudocode and implementation notes.\n\n",
            "## Error Handling\n",
            "errors degradation failure scenarios.\n\n",
            "## Self-Review\n",
            "review audit.\n\n",
            "## User Approval\n",
            "approved user final approval.\n\n",
            "## Reuse Analysis\n",
            "reuse analysis component reuse.\n",
        );
        assert!(design_completeness_report(design).is_none());
    }

    #[test]
    fn design_completeness_report_renders_bulleted_missing_list() {
        let report = design_completeness_report("").expect("empty doc must produce a report");
        assert!(report.starts_with("Design is incomplete. Missing:\n"));
        assert!(report.contains("- sufficient content (design appears incomplete or empty)"));
        assert!(report.contains("- Scope or Scope In/Out section"));
        assert!(report.ends_with("Please add the missing sections before exiting Design mode."));
    }

    #[test]
    fn enumerator_prefixed_headings_pass() {
        // The template's own `## C1. Scope`, `## C2. Architecture / Design` …
        // heading style must satisfy the gate without renaming to bare headings.
        let design = concat!(
            "# Widget Fix Design\n\n",
            "## C1. Scope In / Out\n",
            "In: the widget. Out: the polish. Padding padding padding padding ",
            "padding padding padding padding padding padding padding padding.\n\n",
            "## C2. Architecture / Design\n",
            "approach overview.\n\n",
            "## C3. Data Models\n",
            "the new fields.\n\n",
            "## C4. Algorithms\n",
            "pseudocode.\n\n",
            "## C5. Error Handling\n",
            "degradation.\n\n",
            "## C6. Self-Review\n",
            "four-lens sweep.\n\n",
            "## C7. User Approval\n",
            "approved.\n\n",
            "## C8. Reuse Analysis\n",
            "existing components.\n",
        );
        assert!(
            find_missing_design_sections(design).is_empty(),
            "C-enumerated headings must pass; got {:?}",
            find_missing_design_sections(design)
        );
    }

    #[test]
    fn keyword_in_prose_does_not_satisfy_section() {
        // "architecture" only in prose (never a heading) must NOT satisfy C2 —
        // this is the un-anchored-regex bug that let skeleton drafts pass.
        let design = concat!(
            "# A Design\n\n",
            "## Scope\n",
            "We will change the architecture and the data models and the ",
            "algorithms. Padding padding padding padding padding padding padding ",
            "padding padding padding padding padding padding padding padding.\n\n",
            "## Notes\n",
            "Some error handling review approval reuse prose, all inline.\n",
        );
        let missing = find_missing_design_sections(design);
        for expected in [
            "Architecture or Design section",
            "Data Models section",
            "Algorithms or Implementation Notes section",
            "Error Handling section",
            "Self-Review section",
            "User Approval section",
            "Reuse Analysis section",
        ] {
            assert!(
                missing.iter().any(|m| m == expected),
                "prose keyword must not satisfy {expected:?}; missing = {missing:?}"
            );
        }
    }

    #[test]
    fn bare_review_heading_does_not_satisfy_self_review() {
        // A heading merely named "Design Review" is not the adversarial
        // self-review C6 exists to force.
        let design = concat!(
            "# A Design\n\n",
            "## Scope\n",
            "padding padding padding padding padding padding padding padding ",
            "padding padding padding padding padding padding padding padding.\n\n",
            "## Design Review\n",
            "a review and an audit were done.\n",
        );
        let missing = find_missing_design_sections(design);
        assert!(
            missing.iter().any(|m| m == "Self-Review section"),
            "bare 'Design Review'/'audit' must not satisfy C6; missing = {missing:?}"
        );
    }
}
