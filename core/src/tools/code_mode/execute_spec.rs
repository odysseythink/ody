use ody_code_mode::ToolDefinition as CodeModeToolDefinition;
use ody_tools::JsonSchema;
use ody_tools::ResponsesApiTool;
use ody_tools::ToolSpec;
use std::collections::BTreeMap;

pub(crate) fn create_code_mode_tool(
    enabled_tools: &[CodeModeToolDefinition],
    namespace_descriptions: &BTreeMap<String, ody_code_mode::ToolNamespaceDescription>,
    code_mode_only: bool,
    deferred_tools_available: bool,
) -> ToolSpec {
    let properties = BTreeMap::from([(
        "source".to_string(),
        JsonSchema::string(Some(
            "The JavaScript source to execute. May start with a `// @exec:` pragma line."
                .to_string(),
        )),
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: ody_code_mode::PUBLIC_TOOL_NAME.to_string(),
        description: ody_code_mode::build_exec_tool_description(
            enabled_tools,
            namespace_descriptions,
            code_mode_only,
            deferred_tools_available,
        ),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["source".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_tools::ToolName;
    use pretty_assertions::assert_eq;

    #[test]
    fn create_code_mode_tool_takes_the_source_in_a_required_string() {
        let enabled_tools = vec![ody_code_mode::ToolDefinition {
            name: "update_plan".to_string(),
            tool_name: ToolName::plain("update_plan"),
            description: "Update the plan".to_string(),
            kind: ody_code_mode::CodeModeToolKind::Function,
            input_schema: None,
            output_schema: None,
        }];

        let spec = create_code_mode_tool(
            &enabled_tools,
            &BTreeMap::new(),
            /*code_mode_only*/ true,
            /*deferred_tools_available*/ false,
        );

        let ToolSpec::Function(tool) = spec else {
            panic!("the code mode executor must be a JSON function tool");
        };
        assert_eq!(tool.name, ody_code_mode::PUBLIC_TOOL_NAME);
        assert_eq!(
            tool.description,
            ody_code_mode::build_exec_tool_description(
                &enabled_tools,
                &BTreeMap::new(),
                /*code_mode_only*/ true,
                /*deferred_tools_available*/ false
            )
        );
        assert_eq!(tool.parameters.required, Some(vec!["source".to_string()]));
        assert_eq!(
            tool.parameters
                .properties
                .expect("parameters.properties")
                .keys()
                .collect::<Vec<_>>(),
            vec!["source"]
        );
    }
}
