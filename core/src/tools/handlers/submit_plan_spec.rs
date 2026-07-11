use ody_tools::JsonSchema;
use ody_tools::ResponsesApiTool;
use ody_tools::ToolSpec;
use std::collections::BTreeMap;

pub const SUBMIT_PLAN_TOOL_NAME: &str = "submit_plan";

pub fn create_submit_plan_tool() -> ToolSpec {
    let properties = BTreeMap::from([(
        "plan".to_string(),
        JsonSchema::string(Some(
            "The final plan markdown to persist and submit; must be the only action in the final response.".to_string(),
        )),
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: SUBMIT_PLAN_TOOL_NAME.to_string(),
        description: r#"Submit the final plan in Plan mode.
Call this once as the only action in your final response to persist the plan markdown to `.ody-code/plans/` and cleanly end the turn.
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
