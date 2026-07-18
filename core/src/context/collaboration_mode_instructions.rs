use super::ContextualUserFragment;
use crate::plan_artifact::PlanArtifact;
use crate::plan_mode_tier_selector::PlanModeTierSelector;
use ody_config::config_toml::{PlanModeConfigToml, PlanModeTier};
use ody_protocol::config_types::CollaborationMode;
use ody_protocol::config_types::DesignAuditLevel;
use ody_protocol::config_types::ModeKind;
use ody_protocol::protocol::COLLABORATION_MODE_CLOSE_TAG;
use ody_protocol::protocol::COLLABORATION_MODE_OPEN_TAG;

const SPLIT_THRESHOLD_TEMPLATE_KEY: &str = "split_threshold";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CollaborationModeInstructions {
    instructions: String,
}

impl CollaborationModeInstructions {
    pub(crate) fn from_collaboration_mode(
        collaboration_mode: &CollaborationMode,
        split_threshold: Option<usize>,
        user_prompt: Option<&str>,
        plan_mode_config: Option<&PlanModeConfigToml>,
        plan_artifact: Option<&PlanArtifact>,
    ) -> Option<Self> {
        let instructions = collaboration_mode
            .settings
            .developer_instructions
            .as_ref()
            .filter(|instructions| !instructions.is_empty())?;

        let mut this = Self {
            instructions: instructions.clone(),
        };

        if collaboration_mode.mode == ModeKind::Plan {
            let tier = resolve_plan_mode_tier(user_prompt, plan_mode_config, plan_artifact);
            this = match tier {
                PlanModeTier::Concise => this.with_concise_contract(),
                // Auto means "decide for me", not a renderable contract; `resolve_plan_mode_tier`
                // normalizes it away. Falling through to the rigor contract rather than composing
                // nothing means a future regression degrades to a verbose plan, not a tierless one.
                PlanModeTier::Rigor | PlanModeTier::Auto => {
                    debug_assert!(
                        tier == PlanModeTier::Rigor,
                        "resolve_plan_mode_tier must never return Auto"
                    );
                    this.with_rigor_contract()
                }
            };
            // Prepended last so it lands at the very top: the tier is a host decision, and every
            // tier-shaped rule below is scoped by it. Without this the model had to infer its own
            // tier from indirect cues (prompt length, which sections happened to render) and the
            // cues disagreed.
            this = this.with_plan_tier_declaration(tier);
        }

        if collaboration_mode.mode == ModeKind::Design {
            let level = plan_artifact.and_then(|a| a.design_audit_level());
            this = this.with_design_audit_level(level);
        }

        // Render AFTER all fragments are appended: PLAN_RIGOR_SPLIT carries its
        // own {{ split_threshold }} placeholders, so rendering the base
        // instructions first leaves literal placeholders in the final text.
        if matches!(collaboration_mode.mode, ModeKind::Plan | ModeKind::Design)
            && this.instructions.contains(SPLIT_THRESHOLD_TEMPLATE_KEY)
        {
            this.instructions = render_plan_instructions(&this.instructions, split_threshold);
        }

        Some(this)
    }

    /// The full Rigor tier fragment chain. Order is load-bearing: several fragments open with
    /// "In addition to ... above" and read as a non-sequitur if their dependency has not rendered
    /// yet. `context::rigor_fragment_graph` pins this order against the declared DAG.
    fn with_rigor_contract(self) -> Self {
        self.with_rigor_workflow()
            .with_rigor_coverage()
            .with_rigor_task_skeleton()
            .with_rigor_selfreview()
            .with_rigor_invariants()
            .with_rigor_grounding()
            .with_rigor_scope()
            .with_rigor_rename()
            .with_rigor_risks()
            .with_rigor_split()
            .with_rigor_turn_discipline()
    }

    /// The Concise tier counterpart to [`Self::with_rigor_contract`]. These rules used to be baked
    /// into the base `plan.md`, which renders for every tier — so rigor prompts received the
    /// concise autonomy rules and compactness guidance too, contradicting their own addenda. A
    /// single fragment per tier keeps the base template tier-neutral.
    fn with_concise_contract(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_CONCISE;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    /// States the host-resolved tier at the top of the prompt.
    ///
    /// The tier is a host decision, but nothing used to say so out loud. The model had to infer it
    /// from indirect cues that disagreed with each other: the base template's concise autonomy
    /// section said to self-classify by how brief the request was (a heuristic the host had already
    /// abandoned — `select_tier` ignores the prompt), while the rigor fragments implied the tier by
    /// merely being present. A terse request could therefore talk a rigor-tier session into writing
    /// a concise plan.
    fn with_plan_tier_declaration(self, tier: PlanModeTier) -> Self {
        let name = match tier {
            PlanModeTier::Concise => "concise",
            PlanModeTier::Rigor | PlanModeTier::Auto => "rigor",
        };
        let fragment = format!(
            "## Plan tier: {name} (host-selected)\n\n\
             You are writing a **{name}-tier** plan. The host decided this before your turn began. \
             Do not infer your tier from the length of the user's request, the size of the task, or \
             which sections happen to appear below — a one-line request can still be a rigor-tier \
             plan. The `## {Name} tier addendum:` sections below are binding; no other tier's rules \
             apply to you.",
            name = name,
            Name = capitalize(name),
        );
        Self {
            instructions: format!("{}\n\n{}", fragment, self.instructions),
        }
    }

    pub(crate) fn with_rigor_coverage(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_COVERAGE;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_selfreview(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_SELFREVIEW;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_invariants(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_INVARIANTS;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_grounding(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_GROUNDING;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_rename(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_RENAME;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_scope(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_SCOPE;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_workflow(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_WORKFLOW;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_task_skeleton(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_TASK_SKELETON;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_risks(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_RISKS;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_split(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_SPLIT;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_turn_discipline(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_TURN_DISCIPLINE;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    fn with_design_audit_level(self, level: Option<DesignAuditLevel>) -> Self {
        let fragment = match level {
            Some(level) => format!(
                "## Step 0 — Audit level (host-selected)\n\
                 The user has selected audit level: **{level}** [C:USER]. It governs two things.\n\n\
                 (1) How hard you self-verify assumptions:\n\
                 - **Basic** — verify only load-bearing assumptions (architecture, security, data, ops).\n\
                 - **Standard** — verify every assumption that would be expensive if wrong; record the rest in `## Assumptions & Unverified Items`.\n\
                 - **Deep** — verify nearly everything against sources; treat the repo and upstream as the only ground truth.\n\n\
                 (2) What the host escalates to the user for sign-off when you finalize — ONE merged prompt covering both (a) inferred assumptions from your `## Assumptions & Unverified Items` table (**Basic** surfaces low-confidence rows, **Standard** adds medium, **Deep** all) and (b) adversarial-review findings (**Basic** Critical/High, **Standard** += Medium, **Deep** += Low). You do NOT run this yourself and do NOT separately ask the user to confirm inferred decisions — after `submit_design` with `final: true`, the host presents all level-appropriate items in a single prompt (accept/defer all, or revise) and only finalizes once they are resolved. A revise request keeps you in Design mode to fix them.\n\n\
                 Do NOT ask the user to choose the audit level again."
            ),
            None => format!(
                "## Step 0 — Audit level (host-managed, not selected)\n\
                 No audit level was selected by the host. Default to **Basic** and record `Assumption: audit tier = Basic (auto mode)` in the design's Assumptions section. At Basic, the host escalates Critical/High adversarial-review findings to the user for sign-off when you finalize. Do NOT ask the user to choose the level unless these instructions explicitly say no level was selected."
            ),
        };
        Self {
            instructions: format!("{}\n\n{}", fragment, self.instructions),
        }
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn render_plan_instructions(instructions: &str, split_threshold: Option<usize>) -> String {
    let template = match ody_utils_template::Template::parse(instructions) {
        Ok(template) => template,
        Err(err) => {
            tracing::warn!("plan mode instructions template parse error: {err}");
            return instructions.to_string();
        }
    };

    if !template
        .placeholders()
        .any(|name| name == SPLIT_THRESHOLD_TEMPLATE_KEY)
    {
        return instructions.to_string();
    }

    let value = split_threshold.map_or_else(|| "8".to_string(), |v| v.to_string());
    match template.render([(SPLIT_THRESHOLD_TEMPLATE_KEY, value.as_str())]) {
        Ok(rendered) => rendered,
        Err(err) => {
            tracing::warn!("plan mode instructions template render error: {err}");
            instructions.to_string()
        }
    }
}

fn resolve_plan_mode_tier(
    user_prompt: Option<&str>,
    plan_mode_config: Option<&PlanModeConfigToml>,
    plan_artifact: Option<&PlanArtifact>,
) -> PlanModeTier {
    // `/plan-tier auto` writes Auto straight to the artifact, so filter it here too: Auto means
    // "decide for me" and must be re-resolved, not returned. Returning it verbatim matched neither
    // the Rigor nor the Concise arm of the composition match, yielding a base prompt with no tier
    // contract at all.
    if let Some(tier) = plan_artifact
        .and_then(|a| a.plan_mode_tier())
        .filter(|tier| *tier != PlanModeTier::Auto)
    {
        return tier;
    }

    // The tier does not depend on the prompt: `select_tier` ignores it and always returns Rigor
    // unless config overrides. So a caller that has no prompt to pass MUST resolve to the same
    // tier as one that does. This previously branched on `user_prompt`, sending the promptless
    // caller (`context_manager::updates`, which builds the instructions when the user switches
    // collaboration mode mid-session) to a `Concise` fallback. That silently dropped every rigor
    // fragment, and — because only the prompt branch persisted the tier — also left the artifact
    // tier unset, disabling the `rigor_structure_gap` check in `submit_plan`. `/writing-plan`
    // switches into Plan mode, so it hit this path every time and could never produce a rigor plan.
    let selection =
        PlanModeTierSelector::new(plan_mode_config).select_tier(user_prompt.unwrap_or_default());
    if let Some(artifact) = plan_artifact {
        artifact.set_plan_mode_tier(selection.tier);
    }
    selection.tier
}

impl ContextualUserFragment for CollaborationModeInstructions {
    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (COLLABORATION_MODE_OPEN_TAG, COLLABORATION_MODE_CLOSE_TAG)
    }

    fn body(&self) -> String {
        self.instructions.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_protocol::config_types::CollaborationMode;
    use ody_protocol::config_types::ModeKind;
    use ody_protocol::config_types::Settings;

    fn plan_mode_with_instructions(instructions: &str) -> CollaborationMode {
        CollaborationMode {
            mode: ModeKind::Plan,
            settings: Settings {
                model: "test-model".to_string(),
                reasoning_effort: None,
                developer_instructions: Some(instructions.to_string()),
                design_audit_level: None,
            },
        }
    }

    #[test]
    fn renders_split_threshold_placeholder() {
        let mode =
            plan_mode_with_instructions("Split plans larger than {{ split_threshold }} tasks.");
        // Asserts placeholder rendering only. Every Plan-mode tier composes a contract fragment and
        // a tier declaration around the base instructions, so the body is never bare — `contains`
        // is the assertion this test actually wants.
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            None,
        )
        .expect("should produce instructions");
        assert!(
            instructions
                .body()
                .contains("Split plans larger than 8 tasks.")
        );
        assert!(!instructions.body().contains("{{ split_threshold }}"));
    }

    #[test]
    fn rigor_fragments_have_no_unrendered_split_threshold_placeholder() {
        // user_prompt = Some(..) forces tier resolution through the selector,
        // which always returns Rigor for Plan mode (see
        // plan_mode_tier_selector.rs: select_tier), so all rigor fragments —
        // including PLAN_RIGOR_SPLIT with its own {{ split_threshold }} — are
        // appended.
        let mode =
            plan_mode_with_instructions("Split plans larger than {{ split_threshold }} tasks.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            Some("write the migration plan"),
            None,
            None,
        )
        .expect("should produce instructions");
        let body = instructions.body();
        assert!(
            !body.contains("{{"),
            "rigor-tier instructions still contain an unrendered placeholder:\n{}",
            body.lines()
                .filter(|line| line.contains("{{"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn renders_split_threshold_placeholder_for_design_mode() {
        let mode = CollaborationMode {
            mode: ModeKind::Design,
            settings: Settings {
                model: "test-model".to_string(),
                reasoning_effort: None,
                developer_instructions: Some(
                    "Split designs larger than {{ split_threshold }} subsystems.".to_string(),
                ),
                design_audit_level: None,
            },
        };
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            None,
        )
        .expect("should produce instructions");
        let body = instructions.body();
        assert!(
            body.contains("Split designs larger than 8 subsystems."),
            "body should contain rendered design instructions:\n{body}"
        );
        assert!(
            body.contains("## Step 0 — Audit level"),
            "body should include the host-managed audit level fragment:\n{body}"
        );
    }

    #[test]
    fn design_split_threshold_uses_default_when_config_absent() {
        let mode = CollaborationMode {
            mode: ModeKind::Design,
            settings: Settings {
                model: "test-model".to_string(),
                reasoning_effort: None,
                developer_instructions: Some(
                    "Split designs larger than {{ split_threshold }} subsystems.".to_string(),
                ),
                design_audit_level: None,
            },
        };
        let instructions =
            CollaborationModeInstructions::from_collaboration_mode(&mode, None, None, None, None)
                .expect("should produce instructions");
        let body = instructions.body();
        assert!(
            body.contains("Split designs larger than 8 subsystems."),
            "when split_threshold is None, Design mode should fall back to the same default as Plan mode:\n{body}"
        );
        assert!(
            body.contains("## Step 0 — Audit level"),
            "body should include the host-managed audit level fragment:\n{body}"
        );
    }

    #[test]
    fn design_mode_does_not_compose_plan_rigor_fragments() {
        let mode = CollaborationMode {
            mode: ModeKind::Design,
            settings: Settings {
                model: "test-model".to_string(),
                reasoning_effort: None,
                developer_instructions: Some("Design the work.".to_string()),
                design_audit_level: None,
            },
        };
        let config = PlanModeConfigToml {
            tier: Some(PlanModeTier::Rigor),
            ..Default::default()
        };
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            Some(&config),
            None,
        )
        .expect("should produce instructions");
        let body = instructions.body();
        assert!(
            body.contains("Design the work."),
            "design mode should preserve the base instructions:\n{body}"
        );
        assert!(
            body.contains("## Step 0 — Audit level"),
            "body should include the host-managed audit level fragment:\n{body}"
        );
        assert!(
            !body.contains("## Dependency Overview"),
            "design mode must not compose plan rigor fragments:\n{body}"
        );
    }

    #[test]
    fn leaves_non_plan_instructions_unrendered() {
        let mode = CollaborationMode {
            mode: ModeKind::Default,
            settings: Settings {
                model: "test-model".to_string(),
                reasoning_effort: None,
                developer_instructions: Some("Hello {{ split_threshold }}".to_string()),
                design_audit_level: None,
            },
        };
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            None,
        )
        .expect("should produce instructions");
        // Default mode should not attempt to render the plan placeholder.
        assert_eq!(instructions.body(), "Hello {{ split_threshold }}");
    }

    #[test]
    fn no_placeholder_passes_through_unchanged() {
        let mode = plan_mode_with_instructions("Stay focused.");
        // Same reasoning as `renders_split_threshold_placeholder`: this asserts the no-placeholder
        // passthrough, not which fragments a tier composes.
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            None,
        )
        .expect("should produce instructions");
        assert!(instructions.body().contains("Stay focused."));
    }

    #[test]
    fn composes_rigor_coverage_fragment() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            None,
        )
        .expect("should produce instructions")
        .with_rigor_coverage();
        let body = instructions.body();
        assert!(
            body.contains("## Dependency Overview"),
            "body should contain Dependency Overview section:\n{body}"
        );
        assert!(
            body.contains("## Spec-coverage table"),
            "body should contain Spec-coverage table section:\n{body}"
        );
    }

    #[test]
    fn composes_rigor_selfreview_fragment() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            None,
        )
        .expect("should produce instructions")
        .with_rigor_selfreview();
        let body = instructions.body();

        for (n, label) in [
            (1, "Spec-coverage table"),
            (2, "Placeholder scan"),
            (3, "No phantom tasks"),
            (4, "Dependency soundness"),
            (5, "Caller & build soundness"),
            (6, "Test-the-risk"),
            (7, "Type consistency"),
        ] {
            assert!(
                body.contains(&format!("{n}. {label}")),
                "body should contain self-review item {n} ({label}):\n{body}"
            );
        }

        assert!(
            body.contains("trace one concrete value"),
            "body should contain the end-to-end trace requirement:\n{body}"
        );
    }

    #[test]
    fn composes_rigor_invariants_fragment() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            None,
        )
        .expect("should produce instructions")
        .with_rigor_invariants();
        let body = instructions.body();

        assert!(
            body.contains("## Shared-signature build-green invariant"),
            "body should contain Shared-signature section:\n{body}"
        );
        assert!(
            body.contains("## No-placeholders rule"),
            "body should contain No-placeholders section:\n{body}"
        );
        assert!(
            body.contains("cargo check --workspace --all-targets"),
            "body should contain whole-tree typecheck command:\n{body}"
        );
        assert!(
            body.contains("TODO"),
            "body should list TODO as a forbidden placeholder:\n{body}"
        );
        assert!(
            body.contains("TBD"),
            "body should list TBD as a forbidden placeholder:\n{body}"
        );
    }

    #[test]
    fn composes_rigor_grounding_fragment() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            None,
        )
        .expect("should produce instructions")
        .with_rigor_grounding();
        let body = instructions.body();

        assert!(
            body.contains("## Source-grounding mandate"),
            "body should contain Source-grounding mandate section:\n{body}"
        );
        assert!(
            body.contains("Read") && body.contains("Grep"),
            "body should mandate Read and Grep verification:\n{body}"
        );
        assert!(
            body.contains("Never infer scope from names"),
            "body should contain the anti-name-inference rule:\n{body}"
        );
        assert!(
            body.contains("[C:INFERRED]"),
            "body should contain the inferred-assumption label:\n{body}"
        );
        assert!(
            body.contains("creator_account_user_id"),
            "body should include the external-schema carve-out example:\n{body}"
        );
    }

    #[test]
    fn composes_rigor_rename_fragment() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            None,
        )
        .expect("should produce instructions")
        .with_rigor_rename();
        let body = instructions.body();

        assert!(
            body.contains("## Rename-vs-delete decision prompt"),
            "body should contain Rename-vs-delete section:\n{body}"
        );
        assert!(
            body.contains("Delete") && body.contains("Rename") && body.contains("Carve out"),
            "body should list all three per-hit actions:\n{body}"
        );
        assert!(
            body.contains("one-line reason"),
            "body should require a one-line reason:\n{body}"
        );
        assert!(
            body.contains("No silent survivors"),
            "body should contain the no-silent-survivors rule:\n{body}"
        );
    }

    #[test]
    fn composes_rigor_scope_fragment() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            None,
        )
        .expect("should produce instructions")
        .with_rigor_scope();
        let body = instructions.body();

        assert!(
            body.contains("## Out-of-scope / false-positive discipline"),
            "body should contain Out-of-scope / false-positive discipline section:\n{body}"
        );
        assert!(
            body.contains("## Out-of-scope"),
            "body should contain the mandatory Out-of-scope section heading:\n{body}"
        );
        assert!(
            body.contains("false-positive"),
            "body should mention false-positive discipline:\n{body}"
        );
        assert!(
            body.contains("windows-sandbox-rs"),
            "body should include the windows-sandbox carve-out example:\n{body}"
        );
        assert!(
            body.contains("ext/goal/accounting.rs"),
            "body should include the token-accounting carve-out example:\n{body}"
        );
        assert!(
            body.contains("creator_account_user_id"),
            "body should include the external-schema carve-out example:\n{body}"
        );
    }

    #[test]
    fn rigor_tier_composes_all_fragments() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let prompt = r#"
1. Refactor auth
2. Migrate payments
3. Remove account concept
4. Delete legacy schema
5. Rename user_id
6. Extract helpers
7. Merge account types
8. Redesign session
9. Update connectors/ody-mcp
10. Sweep insta snapshots
"#;
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            Some(prompt),
            None,
            None,
        )
        .expect("should produce instructions");
        let body = instructions.body();
        assert!(body.contains("## Dependency Overview"));
        assert!(body.contains("## Spec-coverage table"));
        assert!(body.contains("## Shared-signature build-green invariant"));
        assert!(body.contains("## Source-grounding mandate"));
        assert!(body.contains("## Out-of-scope / false-positive discipline"));
        assert!(body.contains("## Rename-vs-delete decision prompt"));
        assert!(body.contains("## Rigor tier addendum: Large plan splitting & Parts manifest"));
        assert!(body.contains("## Rigor tier addendum: Turn discipline (when to submit the plan)"));
    }

    #[test]
    fn all_prompts_now_generate_rigor_tier_with_workflow() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            Some("fix typo in README"),
            None,
            None,
        )
        .expect("should produce instructions");
        let body = instructions.body();
        // After removing heuristic tier selector, all prompts generate Rigor tier
        assert!(body.contains("## Rigor tier addendum: Structured workflow"));
        assert!(body.contains("## Dependency Overview"));
    }

    #[test]
    fn split_threshold_rendered_with_rigor_fragments() {
        let mode =
            plan_mode_with_instructions("Split plans larger than {{ split_threshold }} tasks.");

        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(12),
            Some("fix typo"),
            None,
            None,
        )
        .expect("should produce instructions");
        let body = instructions.body();
        // Both cases now include split_threshold rendering + rigor fragments
        assert!(body.contains("Split plans larger than 12 tasks."));
        assert!(body.contains("## Rigor tier addendum: Structured workflow"));
    }

    #[test]
    fn config_override_rigor_applies_without_prompt() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let config = PlanModeConfigToml {
            tier: Some(PlanModeTier::Rigor),
            ..Default::default()
        };
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            Some(&config),
            None,
        )
        .expect("should produce instructions");
        assert!(instructions.body().contains("## Dependency Overview"));
    }

    /// The base `plan.md` used to inline a verbatim copy of PLAN_RIGOR_WORKFLOW as "PHASE 3A",
    /// minus its `(see Self-review addendum)` cross-reference. A concise-tier plan therefore
    /// carried a dangling "verify all seven items" instruction with the seven items defined
    /// nowhere, and an Execution note promising `- [ ]` checkboxes for tasks it never asked for.
    /// Models filled the gap by inventing a plausible seven-item checklist.
    #[test]
    fn concise_tier_has_no_dangling_rigor_references() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let config = PlanModeConfigToml {
            tier: Some(PlanModeTier::Concise),
            ..Default::default()
        };
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            Some(&config),
            None,
        )
        .expect("should produce instructions");
        let body = instructions.body();

        assert!(
            !body.contains("Rigor tier addendum"),
            "concise tier must not receive rigor fragments"
        );
        assert!(
            !body.contains("seven items") && !body.contains("seven verification items"),
            "concise tier must not reference a seven-item checklist it never defines"
        );
        assert!(
            !body.contains("checkboxes for tracking"),
            "concise tier has no tasks, so it must not promise task checkboxes"
        );
    }

    /// Every tier-shaped rule must arrive scoped by a tier the host named. Concise-only rules
    /// reaching a rigor prompt is the same defect class as rigor-only rules reaching a concise one.
    #[test]
    fn rigor_tier_does_not_receive_concise_rules() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            None,
        )
        .expect("should produce instructions");
        let body = instructions.body();

        assert!(
            !body.contains("Concise tier addendum"),
            "rigor tier must not receive the concise contract"
        );
        assert!(
            !body.contains("prefer a compact structure with 3-5 short sections"),
            "rigor tier must not be told to compress to 3-5 sections"
        );
        assert!(
            !body.contains("bias heavily toward moving forward without clarification"),
            "the concise autonomy rule must not reach a rigor prompt"
        );
    }

    /// The base template renders for every tier, so it must not carry rules that only one tier
    /// obeys. This is the invariant behind both leak directions above.
    #[test]
    fn base_template_is_tier_neutral() {
        let plan = ody_collaboration_mode_templates::PLAN;
        for marker in [
            "Concise-tier autonomy",
            "prefer a compact structure with 3-5 short sections",
            "bias heavily toward moving forward without clarification",
            "### Task N",
            "Spec-coverage table",
        ] {
            assert!(
                !plan.contains(marker),
                "base plan.md must stay tier-neutral, but carries tier-specific text: {marker:?}"
            );
        }
        // "standard tier" never existed in `PlanModeTier` (Auto/Concise/Rigor only).
        assert!(
            !plan.contains("standard and rigorous tiers"),
            "base plan.md references a phantom tier"
        );
    }

    /// The host knows the tier; the prompt must say so rather than leave the model to infer it.
    #[test]
    fn tier_is_declared_explicitly_in_the_prompt() {
        let mode = plan_mode_with_instructions("Plan the work.");

        let rigor = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            None,
        )
        .expect("should produce instructions");
        assert!(
            rigor
                .body()
                .starts_with("## Plan tier: rigor (host-selected)"),
            "the tier declaration must lead the prompt; got: {:?}",
            &rigor.body()[..rigor.body().len().min(80)]
        );
        assert!(
            rigor
                .body()
                .contains("You are writing a **rigor-tier** plan.")
        );

        let config = PlanModeConfigToml {
            tier: Some(PlanModeTier::Concise),
            ..Default::default()
        };
        let concise = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            Some(&config),
            None,
        )
        .expect("should produce instructions");
        assert!(
            concise
                .body()
                .starts_with("## Plan tier: concise (host-selected)")
        );
        assert!(
            concise
                .body()
                .contains("You are writing a **concise-tier** plan.")
        );
    }

    /// Regression: `/plan-tier auto` persists `Auto` to the artifact. `Auto` matched neither
    /// composition arm, so the session got a base prompt with no tier contract at all.
    #[test]
    fn artifact_auto_tier_resolves_to_a_real_contract() {
        use ody_protocol::ThreadId;
        use ody_utils_absolute_path::AbsolutePathBuf;

        let mode = plan_mode_with_instructions("Plan the work.");
        let tmp = tempfile::tempdir().unwrap();
        let plans_base_dir = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
        let thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000003").unwrap();
        let artifact =
            crate::plan_artifact::PlanArtifact::new_temp(plans_base_dir, thread_id, "2026-07-16");
        artifact.set_plan_mode_tier(PlanModeTier::Auto);

        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            Some(&artifact),
        )
        .expect("should produce instructions");
        let body = instructions.body();

        assert!(
            body.contains("## Rigor tier addendum: Structured workflow"),
            "auto must re-resolve to a real tier contract, not compose nothing"
        );
        assert!(
            !body.contains("**auto-tier**"),
            "auto is not a renderable tier"
        );
    }

    /// The rigor workflow must live in exactly one place. It was duplicated into `plan.md`, so
    /// rigor-tier prompts rendered it twice and concise-tier prompts rendered it without the
    /// addenda that define what its steps mean.
    #[test]
    fn rigor_workflow_is_not_duplicated_in_base_template() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            None,
        )
        .expect("should produce instructions");
        let body = instructions.body();

        assert_eq!(
            body.matches("**Understand** — explore the codebase")
                .count(),
            1,
            "the five-step workflow must render exactly once, from PLAN_RIGOR_WORKFLOW only"
        );
        assert_eq!(
            body.matches("### Plan document header (top of every plan)")
                .count(),
            1,
            "the plan document header spec must render exactly once"
        );
    }

    /// Regression: the mid-session mode-switch path (`context_manager::updates`) passes
    /// `user_prompt: None`, and a user config with a `[plan_mode]` section but no `tier` key
    /// yields `config: Some(_)` with `tier: None`. That combination used to fall through to a
    /// `Concise` default and drop every rigor fragment — this is the exact shape `/writing-plan`
    /// produced, and no test covered it because they all passed a prompt.
    #[test]
    fn rigor_applies_without_prompt_when_config_sets_no_tier() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let config = PlanModeConfigToml {
            tier: None,
            ..Default::default()
        };
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            Some(&config),
            None,
        )
        .expect("should produce instructions");
        let body = instructions.body();
        assert!(body.contains("## Rigor tier addendum: Structured workflow"));
        assert!(
            body.contains("## Rigor tier addendum: Task skeleton and test-first implementation")
        );
        assert!(body.contains("## Dependency Overview"));
    }

    /// The promptless caller must resolve to the same tier as the prompted one, since
    /// `select_tier` ignores the prompt entirely.
    #[test]
    fn tier_resolution_is_independent_of_prompt_presence() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let with_prompt = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            Some("write the migration plan"),
            None,
            None,
        )
        .expect("should produce instructions");
        let without_prompt = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            None,
        )
        .expect("should produce instructions");
        assert_eq!(with_prompt.body(), without_prompt.body());
    }

    /// `Auto` means "let the selector decide", not a resolvable tier. Returning it verbatim read
    /// as "not Rigor" at every `tier == Rigor` check and dropped the fragments.
    #[test]
    fn config_auto_tier_resolves_to_rigor() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let config = PlanModeConfigToml {
            tier: Some(PlanModeTier::Auto),
            ..Default::default()
        };
        for prompt in [Some("write the migration plan"), None] {
            let instructions = CollaborationModeInstructions::from_collaboration_mode(
                &mode,
                Some(8),
                prompt,
                Some(&config),
                None,
            )
            .expect("should produce instructions");
            assert!(
                instructions
                    .body()
                    .contains("## Rigor tier addendum: Structured workflow"),
                "auto tier should resolve to rigor (prompt: {prompt:?})"
            );
        }
    }

    /// The resolved tier must be persisted to the artifact even when no prompt is present:
    /// `submit_plan` gates its `rigor_structure_gap` check on `artifact.plan_mode_tier()`, so an
    /// unset tier silently skips rigor structure validation.
    #[test]
    fn resolved_tier_persists_to_artifact_without_prompt() {
        use ody_protocol::ThreadId;
        use ody_utils_absolute_path::AbsolutePathBuf;

        let mode = plan_mode_with_instructions("Plan the work.");
        let tmp = tempfile::tempdir().unwrap();
        let plans_base_dir = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
        let thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000002").unwrap();
        let artifact =
            crate::plan_artifact::PlanArtifact::new_temp(plans_base_dir, thread_id, "2026-07-16");
        assert_eq!(artifact.plan_mode_tier(), None);

        CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            Some(&artifact),
        )
        .expect("should produce instructions");

        assert_eq!(artifact.plan_mode_tier(), Some(PlanModeTier::Rigor));
    }

    #[test]
    fn artifact_tier_reused_without_prompt() {
        use ody_protocol::ThreadId;
        use ody_utils_absolute_path::AbsolutePathBuf;

        let mode = plan_mode_with_instructions("Plan the work.");
        let tmp = tempfile::tempdir().unwrap();
        let plans_base_dir = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
        let thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001").unwrap();
        let artifact =
            crate::plan_artifact::PlanArtifact::new_temp(plans_base_dir, thread_id, "2026-07-04");
        artifact.set_plan_mode_tier(PlanModeTier::Rigor);

        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            Some(&artifact),
        )
        .expect("should produce instructions");
        assert!(instructions.body().contains("## Dependency Overview"));
    }

    #[test]
    fn design_mode_injects_selected_audit_level() {
        use ody_protocol::ThreadId;
        use ody_utils_absolute_path::AbsolutePathBuf;

        let mode = CollaborationMode {
            mode: ModeKind::Design,
            settings: Settings {
                model: "test-model".to_string(),
                reasoning_effort: None,
                developer_instructions: Some("Design the work.".to_string()),
                design_audit_level: None,
            },
        };
        let tmp = tempfile::tempdir().unwrap();
        let plans_base_dir = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
        let thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001").unwrap();
        let artifact =
            crate::plan_artifact::PlanArtifact::new_temp(plans_base_dir, thread_id, "2026-07-04");
        artifact.set_design_audit_level(DesignAuditLevel::Deep);

        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            Some(&artifact),
        )
        .expect("should produce instructions");
        let body = instructions.body();
        assert!(
            body.contains("**Deep**"),
            "body should contain the selected audit level:\n{body}"
        );
        assert!(
            body.contains("Do NOT ask the user to choose the audit level again"),
            "body should tell the model not to re-prompt for the audit level:\n{body}"
        );
        assert!(
            body.contains("escalates to the user for sign-off"),
            "body should explain the level drives which review findings the host escalates:\n{body}"
        );
    }

    #[test]
    fn design_mode_without_level_keeps_fallback() {
        let mode = CollaborationMode {
            mode: ModeKind::Design,
            settings: Settings {
                model: "test-model".to_string(),
                reasoning_effort: None,
                developer_instructions: Some("Design the work.".to_string()),
                design_audit_level: None,
            },
        };
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            None,
        )
        .expect("should produce instructions");
        let body = instructions.body();
        assert!(
            body.contains("Default to **Basic**"),
            "body should default to Basic when no audit level is selected:\n{body}"
        );
        assert!(
            body.contains("Assumption: audit tier = Basic (auto mode)"),
            "body should record the auto-mode assumption:\n{body}"
        );
    }
}
