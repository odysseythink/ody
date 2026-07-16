use ody_tools::ToolSpec;

pub const SUBMIT_DESIGN_TOOL_NAME: &str = "submit_design";

pub fn create_submit_design_tool() -> ToolSpec {
    use ody_tools::{JsonSchema, ResponsesApiTool};
    use std::collections::BTreeMap;

    let properties = BTreeMap::from([
        (
            "design".to_string(),
            JsonSchema::string(Some(
                "The design index markdown to persist. For single-file designs this is the complete design document. For split designs (## Parts table present), pass the index markdown on every call — the turn only ends once no part row is `pending`; intermediate calls return the stem directory path for part-file writes.".to_string(),
            )),
        ),
        (
            "final".to_string(),
            JsonSchema::boolean(Some(
                "Whether this submission is the final design ready to exit Design mode. Defaults to false. Pass `false` (or omit) to checkpoint an in-progress/skeleton design: it is persisted and shown, but the turn stays in Design mode so you can keep building. Pass `true` ONLY when the design is complete and you want to exit — this runs the C1–C8 completeness gate and, if it passes, ends the turn.".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: SUBMIT_DESIGN_TOOL_NAME.to_string(),
        description: format!(
            "Persist the current design in Design mode. The host derives the filename from the design's # Title and atomically writes it to {} — do not use a shell command or Write tool for the index file; use only this tool. Use `final: false` (the default) to checkpoint a partial/skeleton design each turn without exiting; use `final: true` only when the design is complete and you intend to exit. Supports single-file designs and split designs (## Parts manifest). For split designs, call this tool with the index markdown after writing each part; the tool will report pending parts and keep the session in Design mode until all parts are done. A `final: true` call then performs a completeness check (C1–C8) before finalizing.",
            ".ody-code/designs/"
        ),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["design".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_has_correct_tool_name() {
        let spec = create_submit_design_tool();
        match spec {
            ToolSpec::Function(tool) => {
                assert_eq!(tool.name, "submit_design");
                let props = tool.parameters.properties.as_ref().expect("must have properties");
                assert!(
                    props.contains_key("design"),
                    "must have 'design' field"
                );
                assert!(
                    !props.contains_key("plan"),
                    "must NOT have 'plan' field (that's submit_plan)"
                );
                let req = tool.parameters.required.clone().unwrap_or_default();
                assert!(req.iter().any(|r| r == "design"), "design must be required");
                assert!(!req.iter().any(|r| r == "plan"), "plan must not be required");
            }
            _ => panic!("expected Function variant"),
        }
    }

    #[test]
    fn spec_description_mentions_designs_directory() {
        let spec = create_submit_design_tool();
        match spec {
            ToolSpec::Function(tool) => {
                assert!(
                    tool.description.contains(".ody-code/designs/"),
                    "description must mention .ody-code/designs/: {}",
                    tool.description
                );
                assert!(
                    !tool.description.contains(".ody-code/plans/"),
                    "description must NOT mention .ody-code/plans/"
                );
            }
            _ => panic!("expected Function variant"),
        }
    }
}
