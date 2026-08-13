use ody_tools::JsonSchema;
use ody_tools::ResponsesApiTool;
use ody_tools::ToolSpec;
use std::collections::BTreeMap;

pub const VALIDATE_PLAN_PART_TOOL_NAME: &str = "validate_plan_part";

pub fn create_validate_plan_part_tool() -> ToolSpec {
    let properties = BTreeMap::from([(
        "task_id".to_string(),
        JsonSchema::string(Some(
            "Task ID from the persisted frozen `## Parts` manifest.".to_string(),
        )),
    )]);
    ToolSpec::Function(ResponsesApiTool {
        name: VALIDATE_PLAN_PART_TOOL_NAME.to_string(),
        description: "Validate the current split-plan part locally with the exact completion, source-evidence, size, and dependency checks used by submit_plan. Call this after writing a task part and before advancing its manifest status. This tool never changes the plan or part files.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, Some(vec!["task_id".to_string()]), Some(false.into())),
        output_schema: None,
    })
}
