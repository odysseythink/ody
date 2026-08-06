use ody_tools::{JsonSchema, ResponsesApiTool, ToolSpec};
use std::collections::BTreeMap;

pub const SUBMIT_ROADMAP_TOOL_NAME: &str = "submit_roadmap";

pub fn create_submit_roadmap_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        ("roadmap".to_string(), JsonSchema::string(Some("Markdown roadmap with a ## Phases table. Each phase row must have an ID and a status; exactly one row must be active.".to_string()))),
        ("slug".to_string(), JsonSchema::string(Some("Filesystem-safe roadmap topic slug, without date or extension.".to_string()))),
    ]);
    ToolSpec::Function(ResponsesApiTool {
        name: SUBMIT_ROADMAP_TOOL_NAME.to_string(),
        description: "Submit a proposed multi-phase roadmap in Design mode. The host validates phase rows and asks the user to confirm the split before persisting it. After confirmation, only the active phase may be designed.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, Some(vec!["roadmap".to_string(), "slug".to_string()]), Some(false.into())),
        output_schema: None,
    })
}
