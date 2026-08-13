use ody_tools::JsonSchema;
use ody_tools::ResponsesApiTool;
use ody_tools::ToolSpec;
use std::collections::BTreeMap;

pub const SUBMIT_PLAN_TOOL_NAME: &str = "submit_plan";

pub fn create_submit_plan_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "plan".to_string(),
            JsonSchema::string(Some(
            "The initial plan markdown or a complete non-frozen replacement. For a frozen split manifest, do not resend it; use task_id/status. If all fields are omitted, the host reads the persisted plan markdown.".to_string(),
            )),
        ),
        (
            "task_id".to_string(),
            JsonSchema::string(Some(
                "For a split task plan, advance this frozen manifest task without resending the index. Use together with status after validate_plan_part passes.".to_string(),
            )),
        ),
        (
            "status".to_string(),
            JsonSchema::string(Some(
                "Status for task_id. Use `done` after validate_plan_part passes.".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: SUBMIT_PLAN_TOOL_NAME.to_string(),
        description: r#"Submit or checkpoint the plan in Plan mode.
Call this as the only action in your response to persist the plan markdown to `.ody-code/plans/`.
If the plan has no `## Parts` manifest, or the manifest has no `pending` rows, this call is terminal and cleanly ends the turn.
If the manifest still has a `pending` row, first call `validate_plan_part`, then call this tool with only `task_id` and `status: "done"`. The host edits only that status cell in its persisted frozen index; do not regenerate or resend the full index.
You may omit `plan` on the final call; the host will read the persisted plan from the current artifact file.
Do not send a `<proposed_plan>` block and do not call `update_plan` for finalization.
"#
        .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            None,
            Some(false.into()),
        ),
        output_schema: None,
    })
}
