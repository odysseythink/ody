//! Host-driven "what next?" menu presented after a terminal `submit_design`.
//!
//! Design mode used to rely on the model calling `request_user_input` itself
//! (design.md Step 5). That is the only step in the design flow with no runtime
//! enforcement, and it silently dropped under long-session instruction drift —
//! leaving the user staring at a finished design with no next action. The host
//! now drives the menu directly so the affordance always appears, and it is
//! severity-aware: when the adversarial review found Critical/High issues, the
//! recommended choice is to revise the design rather than to start planning.

use crate::design_review::orchestrator::SeverityCounts;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use ody_protocol::request_user_input::RequestUserInputArgs;
use ody_protocol::request_user_input::RequestUserInputQuestion;
use ody_protocol::request_user_input::RequestUserInputQuestionOption;

const QUESTION_ID: &str = "design_next_step";

/// The four next-step actions offered after a design is submitted. The variant
/// order in a built menu is severity-dependent (see [`build_next_step_menu`]);
/// each variant owns both its localized label and the instruction the model
/// must relay once the user picks it, so the two can never drift apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NextStep {
    Revise,
    Plan,
    CompactPlan,
    Stay,
}

impl NextStep {
    fn label(self, chinese: bool) -> String {
        match (self, chinese) {
            (NextStep::Revise, true) => "修复严重问题后再规划".to_string(),
            (NextStep::Revise, false) => "Revise the design first".to_string(),
            (NextStep::Plan, true) => "进入 Plan 模式".to_string(),
            (NextStep::Plan, false) => "Enter Plan mode".to_string(),
            (NextStep::CompactPlan, true) => "压缩后进入 Plan 模式".to_string(),
            (NextStep::CompactPlan, false) => "Compact, then enter Plan mode".to_string(),
            (NextStep::Stay, true) => "留在 Design 模式".to_string(),
            (NextStep::Stay, false) => "Stay in Design mode".to_string(),
        }
    }

    fn description(self, chinese: bool, counts: Option<SeverityCounts>) -> String {
        match (self, chinese) {
            (NextStep::Revise, true) => {
                let (c, h) = review_gap(counts);
                format!("对抗性自审发现 {c} 个严重、{h} 个高优先级问题——建议先留在 Design 模式修复它们。")
            }
            (NextStep::Revise, false) => {
                let (c, h) = review_gap(counts);
                format!("The adversarial review found {c} critical and {h} high-severity issue(s); revise them in Design mode before planning.")
            }
            (NextStep::Plan, true) => "基于当前设计开始写实现计划（运行 /plan）。".to_string(),
            (NextStep::Plan, false) => {
                "Start the implementation plan from this design (run /plan).".to_string()
            }
            (NextStep::CompactPlan, true) => {
                "先压缩会话历史，再切到 Plan 模式（先 /compact 再 /plan）。".to_string()
            }
            (NextStep::CompactPlan, false) => {
                "Compact the conversation first, then switch to Plan mode (/compact then /plan)."
                    .to_string()
            }
            (NextStep::Stay, true) => "继续修改或讨论这个设计。".to_string(),
            (NextStep::Stay, false) => "Keep revising or discussing this design.".to_string(),
        }
    }

    /// The instruction the model must relay to the user for this choice. Kept in
    /// the tool result (fresh every turn) rather than the template so it never
    /// erodes under compaction.
    fn instruction(self, chinese: bool) -> String {
        match (self, chinese) {
            (NextStep::Revise, true) => {
                "用户选择先修复严重问题：留在 Design 模式，明确列出你将处理审查中的哪些点，不要开始实现。".to_string()
            }
            (NextStep::Revise, false) => {
                "The user chose to revise first: stay in Design mode, name which review findings you will address, and do not start implementing.".to_string()
            }
            (NextStep::Plan, true) => {
                "用户选择进入 Plan 模式：告诉他们运行 `/plan` 开始写实现计划。不要现在就开始实现。".to_string()
            }
            (NextStep::Plan, false) => {
                "The user chose Plan mode: tell them to run `/plan` to start the implementation plan. Do not start implementing now.".to_string()
            }
            (NextStep::CompactPlan, true) => {
                "用户选择压缩后进入 Plan 模式：告诉他们先运行 `/compact`，再运行 `/plan`。不要现在就开始实现。".to_string()
            }
            (NextStep::CompactPlan, false) => {
                "The user chose to compact then plan: tell them to run `/compact` first, then `/plan`. Do not start implementing now.".to_string()
            }
            (NextStep::Stay, true) => "用户选择留在 Design 模式：询问他们想修改设计的哪一部分。".to_string(),
            (NextStep::Stay, false) => {
                "The user chose to stay in Design mode: ask what they would like to revise.".to_string()
            }
        }
    }
}

fn review_gap(counts: Option<SeverityCounts>) -> (usize, usize) {
    counts.map(|c| (c.critical, c.high)).unwrap_or((0, 0))
}

/// True when the review surfaced at least one Critical or High finding — the
/// signal that "revise before planning" should be the recommended option.
fn has_blocking_findings(counts: Option<SeverityCounts>) -> bool {
    let (critical, high) = review_gap(counts);
    critical > 0 || high > 0
}

/// The ordered list of actions to offer. When the review found Critical/High
/// issues, "revise" leads (and is therefore the recommended default); otherwise
/// planning leads and "revise" is dropped (there is nothing flagged to revise).
fn ordered_steps(counts: Option<SeverityCounts>) -> Vec<NextStep> {
    if has_blocking_findings(counts) {
        vec![
            NextStep::Revise,
            NextStep::Plan,
            NextStep::CompactPlan,
            NextStep::Stay,
        ]
    } else {
        vec![NextStep::Plan, NextStep::CompactPlan, NextStep::Stay]
    }
}

/// Build the severity-aware next-step menu in the user's language.
pub(crate) fn build_next_step_menu(
    chinese: bool,
    counts: Option<SeverityCounts>,
) -> RequestUserInputArgs {
    let steps = ordered_steps(counts);
    let options = steps
        .iter()
        .map(|step| RequestUserInputQuestionOption {
            label: step.label(chinese),
            description: step.description(chinese, counts),
        })
        .collect();

    let question = if chinese {
        "设计已提交。接下来做什么？"
    } else {
        "The design is submitted. What next?"
    };
    let header = if chinese { "下一步" } else { "Next step" };

    RequestUserInputArgs {
        questions: vec![RequestUserInputQuestion {
            id: QUESTION_ID.to_string(),
            header: header.to_string(),
            question: question.to_string(),
            is_other: false,
            is_secret: false,
            options: Some(options),
        }],
        auto_resolution_ms: None,
    }
}

/// Map a chosen label back to the instruction the model should relay. Falls
/// back to a generic instruction for free-text ("Other") answers.
fn instruction_for_answer(chinese: bool, counts: Option<SeverityCounts>, answer: &str) -> String {
    for step in ordered_steps(counts) {
        if step.label(chinese) == answer {
            return step.instruction(chinese);
        }
    }
    if chinese {
        format!("用户对下一步的回答是“{answer}”。据此行动，不要现在就开始实现。")
    } else {
        format!("The user's next-step answer was \"{answer}\". Act on it and do not start implementing now.")
    }
}

/// Determine whether the transcript language is Chinese, mirroring how core
/// derives the effective language for model instructions (explicit config
/// value, else the system locale).
pub(crate) fn transcript_is_chinese(language: Option<&str>) -> bool {
    let resolved = match language.map(str::trim).filter(|l| !l.is_empty()) {
        Some(l) if l.eq_ignore_ascii_case("auto") => ody_config::locale::detect_system_locale_code(),
        Some(l) => ody_config::locale::parse_locale_code(l),
        None => ody_config::locale::detect_system_locale_code(),
    };
    resolved.as_deref() == Some("zh")
}

/// Present the next-step menu to the user after a terminal design submit and
/// return the instruction the model should relay. Returns `None` if the request
/// was cancelled or produced no answer, in which case the caller keeps the plain
/// "design submitted" message.
pub(crate) async fn drive_post_design_next_step(
    session: &Session,
    turn: &TurnContext,
    call_id: String,
    counts: Option<SeverityCounts>,
) -> Option<String> {
    let chinese = transcript_is_chinese(turn.config.language.as_deref());
    let args = build_next_step_menu(chinese, counts);
    let response = session.request_user_input(turn, call_id, args).await?;
    let answer = response
        .answers
        .get(QUESTION_ID)
        .and_then(|a| a.answers.first())
        .cloned()?;
    Some(instruction_for_answer(chinese, counts, &answer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(critical: usize, high: usize) -> SeverityCounts {
        SeverityCounts {
            critical,
            high,
            medium: 0,
            low: 0,
        }
    }

    #[test]
    fn menu_leads_with_revise_when_criticals_present() {
        let menu = build_next_step_menu(false, Some(counts(3, 2)));
        let options = menu.questions[0].options.as_ref().unwrap();
        assert_eq!(options.len(), 4);
        assert_eq!(options[0].label, "Revise the design first");
        assert!(options[0].description.contains('3') && options[0].description.contains('2'));
    }

    #[test]
    fn menu_leads_with_plan_when_no_blocking_findings() {
        let menu = build_next_step_menu(false, Some(counts(0, 0)));
        let options = menu.questions[0].options.as_ref().unwrap();
        assert_eq!(options.len(), 3, "revise is dropped when nothing is flagged");
        assert_eq!(options[0].label, "Enter Plan mode");
    }

    #[test]
    fn menu_with_no_review_still_offers_plan_first() {
        // design_review_model unset → counts None → menu still appears.
        let menu = build_next_step_menu(true, None);
        let options = menu.questions[0].options.as_ref().unwrap();
        assert_eq!(options.len(), 3);
        assert_eq!(options[0].label, "进入 Plan 模式");
    }

    #[test]
    fn chinese_menu_uses_chinese_labels() {
        let menu = build_next_step_menu(true, Some(counts(1, 0)));
        assert_eq!(menu.questions[0].header, "下一步");
        let options = menu.questions[0].options.as_ref().unwrap();
        assert_eq!(options[0].label, "修复严重问题后再规划");
    }

    #[test]
    fn instruction_maps_each_label_to_its_action() {
        let c = Some(counts(2, 1));
        assert!(
            instruction_for_answer(false, c, "Enter Plan mode").contains("/plan"),
            "plan choice must tell the user to run /plan"
        );
        assert!(
            instruction_for_answer(false, c, "Compact, then enter Plan mode").contains("/compact")
        );
        assert!(
            instruction_for_answer(false, c, "Revise the design first")
                .contains("do not start implementing")
        );
    }

    #[test]
    fn instruction_falls_back_for_free_text_answer() {
        let instruction = instruction_for_answer(false, None, "something custom");
        assert!(instruction.contains("something custom"));
        assert!(instruction.contains("do not start implementing"));
    }

    #[test]
    fn transcript_language_detection() {
        assert!(transcript_is_chinese(Some("zh-CN")));
        assert!(transcript_is_chinese(Some("Chinese")));
        assert!(!transcript_is_chinese(Some("en-US")));
    }
}
