use super::*;
use pretty_assertions::assert_eq;

#[test]
fn image_generation_tool_matches_expected_spec() {
    assert_eq!(
        create_image_generation_tool("png"),
        ToolSpec::ImageGeneration {
            output_format: "png".to_string(),
        }
    );
}
