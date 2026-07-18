use super::augment_tool_spec_for_code_mode;
use super::tool_spec_to_code_mode_tool_definition;
use crate::AdditionalProperties;
use crate::JsonSchema;
use crate::ResponsesApiTool;
use crate::ToolName;
use crate::ToolSpec;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn augment_tool_spec_for_code_mode_augments_function_tools() {
    assert_eq!(
        augment_tool_spec_for_code_mode(ToolSpec::Function(ResponsesApiTool {
            name: "lookup_order".to_string(),
            description: "Look up an order".to_string(),
            strict: false,
            defer_loading: Some(true),
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "order_id".to_string(),
                    JsonSchema::string(/*description*/ None),
                )]),
                Some(vec!["order_id".to_string()]),
                Some(AdditionalProperties::Boolean(false))
            ),
            output_schema: Some(json!({
                "type": "object",
                "properties": {
                    "ok": {"type": "boolean"}
                },
                "required": ["ok"],
            })),
        })),
        ToolSpec::Function(ResponsesApiTool {
            name: "lookup_order".to_string(),
            description: r#"Look up an order

exec tool declaration:
```ts
declare const tools: { lookup_order(args: { order_id: string; }): Promise<{ ok: boolean; }>; };
```"#
                .to_string(),
            strict: false,
            defer_loading: Some(true),
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "order_id".to_string(),
                    JsonSchema::string(/*description*/ None),
                )]),
                Some(vec!["order_id".to_string()]),
                Some(AdditionalProperties::Boolean(false))
            ),
            output_schema: Some(json!({
                "type": "object",
                "properties": {
                    "ok": {"type": "boolean"}
                },
                "required": ["ok"],
            })),
        })
    );
}

#[test]
fn augment_tool_spec_for_code_mode_preserves_exec_tool_description() {
    let exec = ToolSpec::Function(ResponsesApiTool {
        name: ody_code_mode::PUBLIC_TOOL_NAME.to_string(),
        description: "Run code".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::from([("source".to_string(), JsonSchema::string(None))]),
            Some(vec!["source".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    });

    // The executor is not one of its own nested tools, so it is left alone.
    assert_eq!(augment_tool_spec_for_code_mode(exec.clone()), exec);
}

#[test]
fn tool_spec_to_code_mode_tool_definition_returns_augmented_nested_tools() {
    let parameters = JsonSchema::object(
        BTreeMap::from([("input".to_string(), JsonSchema::string(None))]),
        Some(vec!["input".to_string()]),
        Some(false.into()),
    );
    let spec = ToolSpec::Function(ResponsesApiTool {
        name: "apply_patch".to_string(),
        description: "Apply a patch".to_string(),
        strict: false,
        defer_loading: None,
        parameters: parameters.clone(),
        output_schema: None,
    });

    let definition =
        tool_spec_to_code_mode_tool_definition(&spec).expect("apply_patch is a nested tool");
    assert_eq!(definition.name, "apply_patch");
    assert_eq!(definition.tool_name, ToolName::plain("apply_patch"));
    assert_eq!(definition.kind, ody_code_mode::CodeModeToolKind::Function);
    assert_eq!(
        definition.input_schema,
        serde_json::to_value(&parameters).ok()
    );
    assert!(definition.description.starts_with("Apply a patch"));
    assert!(definition.description.contains("exec tool declaration:"));
}

#[test]
fn tool_spec_to_code_mode_tool_definition_skips_unsupported_variants() {
    assert_eq!(
        tool_spec_to_code_mode_tool_definition(&ToolSpec::ToolSearch {
            execution: "sync".to_string(),
            description: "Search".to_string(),
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None
            ),
        }),
        None
    );
}
