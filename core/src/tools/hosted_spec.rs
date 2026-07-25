use ody_tools::ToolSpec;

pub fn create_image_generation_tool(output_format: &str) -> ToolSpec {
    ToolSpec::ImageGeneration {
        output_format: output_format.to_string(),
    }
}

#[cfg(test)]
#[path = "hosted_spec_tests.rs"]
mod tests;
