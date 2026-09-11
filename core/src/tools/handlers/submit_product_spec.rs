use ody_tools::ToolSpec;

pub const SUBMIT_PRODUCT_TOOL_NAME: &str = "submit_product";

/// Product mode's terminal tool. Unlike `submit_plan`/`submit_design` there is
/// no checkpoint flow: the requirements document already lives on disk (written
/// via `write_file` per the product template), so this tool takes no markdown —
/// the host reads the persisted document from the current product artifact,
/// verifies it is non-empty, marks the artifact submitted, and emits the
/// finalized plan item the TUI uses to offer the handoff menu (Enter Plan /
/// Enter Design / Stay). Collaboration mode is owned by the client, so the
/// actual mode switch happens in the TUI, mirroring the post-design flow.
pub fn create_submit_product_tool() -> ToolSpec {
    use ody_tools::{JsonSchema, ResponsesApiTool};
    use std::collections::BTreeMap;

    ToolSpec::Function(ResponsesApiTool {
        name: SUBMIT_PRODUCT_TOOL_NAME.to_string(),
        description: format!(
            "Finalize the requirements document in Product mode and end the mode. Call this ONLY after the user has confirmed the document is complete (via `request_user_input`). The host reads the persisted document from the current product artifact under {} and verifies it is non-empty. After this call the client shows a handoff menu (Enter Plan mode / Enter Design mode / Stay in Product mode) — the switch itself is performed by the client, not by you.",
            ".ody-code/products/"
        ),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(BTreeMap::new(), None, Some(false.into())),
        output_schema: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_has_correct_tool_name_and_no_parameters() {
        let spec = create_submit_product_tool();
        match spec {
            ToolSpec::Function(tool) => {
                assert_eq!(tool.name, SUBMIT_PRODUCT_TOOL_NAME);
                let props = tool.parameters.properties.clone().unwrap_or_default();
                assert!(
                    props.is_empty(),
                    "submit_product takes no arguments; the document is read from disk: {props:?}"
                );
            }
            _ => panic!("expected Function variant"),
        }
    }

    #[test]
    fn spec_description_mentions_products_directory_and_handoff() {
        let spec = create_submit_product_tool();
        match spec {
            ToolSpec::Function(tool) => {
                assert!(
                    tool.description.contains(".ody-code/products/"),
                    "description must mention .ody-code/products/: {}",
                    tool.description
                );
                assert!(
                    tool.description.contains("request_user_input"),
                    "description must require prior user confirmation: {}",
                    tool.description
                );
            }
            _ => panic!("expected Function variant"),
        }
    }
}
