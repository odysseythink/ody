use super::*;
use ody_tools::JsonSchemaPrimitiveType;
use ody_tools::JsonSchemaType;
use pretty_assertions::assert_eq;

fn function_tool(include_environment_id: bool) -> ResponsesApiTool {
    let ToolSpec::Function(tool) = create_apply_patch_tool(include_environment_id) else {
        panic!("apply_patch must be a JSON function tool so every model can call it");
    };
    tool
}

#[test]
fn create_apply_patch_tool_takes_the_patch_in_a_required_input_string() {
    let tool = function_tool(/*include_environment_id*/ false);

    assert_eq!(tool.name, APPLY_PATCH_TOOL_NAME);
    assert_eq!(tool.parameters.required, Some(vec!["input".to_string()]));

    let properties = tool.parameters.properties.expect("parameters.properties");
    assert_eq!(properties.keys().collect::<Vec<_>>(), vec!["input"]);
    let input = &properties["input"];
    assert_eq!(
        input.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::String))
    );

    let description = input.description.as_deref().expect("input.description");
    assert!(description.contains("*** Begin Patch"));
    assert!(description.contains("*** End Patch"));
}

#[test]
fn create_apply_patch_tool_documents_environment_id_only_when_requested() {
    let without = function_tool(/*include_environment_id*/ false);
    let with = function_tool(/*include_environment_id*/ true);

    let description_of = |tool: ResponsesApiTool| {
        tool.parameters.properties.expect("parameters.properties")["input"]
            .description
            .clone()
            .expect("input.description")
    };

    assert!(!description_of(without).contains("*** Environment ID:"));
    assert!(description_of(with).contains("*** Environment ID:"));
}

#[test]
fn create_apply_patch_tool_warns_against_prefixed_markers() {
    let tool = function_tool(/*include_environment_id*/ false);
    let description = tool.description;
    assert!(
        description.contains("do not prefix"),
        "apply_patch description should warn models not to prefix markers: {description}"
    );
    assert!(
        description.contains("*** Begin Patch"),
        "apply_patch description should name the begin marker: {description}"
    );
    assert!(
        description.contains("*** End Patch"),
        "apply_patch description should name the end marker: {description}"
    );
}
