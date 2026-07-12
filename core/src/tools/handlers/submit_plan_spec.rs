use ody_tools::JsonSchema;
use ody_tools::ResponsesApiTool;
use ody_tools::ToolSpec;
use std::collections::BTreeMap;

pub const SUBMIT_PLAN_TOOL_NAME: &str = "submit_plan";

pub fn create_submit_plan_tool() -> ToolSpec {
    let properties = BTreeMap::from([(
        "plan".to_string(),
        JsonSchema::string(Some(
            "The plan markdown to persist; must be the only action in this response. For a single-file plan, this is the final submission. For a split plan (index + parts, see the `## Parts` manifest), pass the index markdown on every call — the turn only ends once no row is `pending`.".to_string(),
        )),
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: SUBMIT_PLAN_TOOL_NAME.to_string(),
        description: r#"Submit or checkpoint the plan in Plan mode.
Call this as the only action in your response to persist the plan markdown to `.ody-code/plans/`.
If the plan has no `## Parts` manifest, or the manifest has no `pending` rows, this call is terminal and cleanly ends the turn.
If the manifest still has a `pending` row (a split plan mid-progress), this call only saves the index and keeps Plan mode active — call it again after each part is written, and keep going until every row is `done`.
Do not send a `<proposed_plan>` block and do not call `update_plan` for finalization.
"#
        .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["plan".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}
