use ody_tools::JsonSchema;
use ody_tools::ResponsesApiTool;
use ody_tools::ToolSpec;
use std::collections::BTreeMap;

pub fn create_review_tests_tool() -> ToolSpec {
    let properties = BTreeMap::from([(
        "project_root".to_string(),
        JsonSchema::string(Some(
            "Optional project root; defaults to the agent workspace root.".to_string(),
        )),
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: "review_tests".to_string(),
        description: "Independently review the changed test files. The reviewer is a separate model alias (or the current model if none is configured) so it can adversarially check whether the tests are meaningful, non-vacuous, and match the implementation.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            /*required*/ None,
            Some(false.into()),
        ),
        output_schema: None,
    })
}
