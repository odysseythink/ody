use ody_tools::JsonSchema;
use ody_tools::ResponsesApiTool;
use ody_tools::ToolSpec;
use std::collections::BTreeMap;

pub const APPLY_PATCH_TOOL_NAME: &str = "apply_patch";

const PATCH_FORMAT: &str = r#"The patch text must be exactly:

*** Begin Patch
[one or more file sections]
*** End Patch

A file section is one of:

*** Add File: path/to/file
+every line of the new file, each prefixed with +

*** Delete File: path/to/file

*** Update File: path/to/file
*** Move to: path/to/new/file      (optional, only to rename)
@@ optional context line to locate the hunk
 unchanged line (leading space)
-removed line
+added line
*** End of File                    (optional, only when the hunk reaches EOF)

Paths are relative to the working directory. Update hunks need at least one
line of surrounding context so the hunk can be located unambiguously."#;

const ENVIRONMENT_ID_FORMAT: &str = r#"

To target a specific environment, put its id on a line directly after
`*** Begin Patch`:

*** Environment ID: <environment_id>"#;

/// The `apply_patch` file-edit tool, as a plain JSON-schema function tool so
/// every model can call it regardless of wire protocol. The patch text is
/// carried in a single `input` string and parsed by `ody_apply_patch::parse_patch`.
pub fn create_apply_patch_tool(include_environment_id: bool) -> ToolSpec {
    let mut format = PATCH_FORMAT.to_string();
    if include_environment_id {
        format.push_str(ENVIRONMENT_ID_FORMAT);
    }

    let properties = BTreeMap::from([(
        "input".to_string(),
        JsonSchema::string(Some(format!(
            "The complete patch text, starting with `*** Begin Patch` and ending with `*** End Patch`.\n\n{format}"
        ))),
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: APPLY_PATCH_TOOL_NAME.to_string(),
        description:
            "Edit files by applying a patch. Use this to create, update, delete, or move files."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["input".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

#[cfg(test)]
#[path = "apply_patch_spec_tests.rs"]
mod tests;
