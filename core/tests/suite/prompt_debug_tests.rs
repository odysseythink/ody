use std::sync::Arc;

use anyhow::Result;
use ody_core::build_prompt_input;
use ody_core::config::ConfigBuilder;
use ody_core::config::ConfigOverrides;
use ody_home::OdyHomeUserInstructionsProvider;
use ody_protocol::models::ContentItem;
use ody_protocol::models::ResponseItem;
use ody_protocol::user_input::UserInput;
use core_test_support::responses::strip_metadata;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

const TEST_INSTRUCTIONS: &str = "Global test instructions";

#[tokio::test]
async fn build_prompt_input_includes_context_and_user_message() -> Result<()> {
    let ody_home = TempDir::new()?;
    let cwd = TempDir::new()?;
    std::fs::write(ody_home.path().join("AGENTS.md"), TEST_INSTRUCTIONS)?;
    let config = ConfigBuilder::default()
        .ody_home(ody_home.path().to_path_buf())
        .harness_overrides(ConfigOverrides {
            cwd: Some(cwd.path().to_path_buf()),
            ody_self_exe: Some(std::env::current_exe()?),
            ..ConfigOverrides::default()
        })
        .build()
        .await?;
    let user_instructions_provider = Arc::new(OdyHomeUserInstructionsProvider::new(
        config.ody_home.clone(),
    ));

    let input = build_prompt_input(
        config,
        vec![UserInput::Text {
            text: "hello from debug prompt".to_string(),
            text_elements: Vec::new(),
        }],
        /*state_db*/ None,
        user_instructions_provider,
        /*plan_mode*/ false,
    )
    .await?;

    let expected_user_message = ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: "hello from debug prompt".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    };
    assert_eq!(
        input.last().cloned().map(strip_metadata),
        Some(expected_user_message)
    );
    assert!(input.iter().any(|item| {
        let ResponseItem::Message { content, .. } = item else {
            return false;
        };

        content.iter().any(|content_item| {
            let (ContentItem::InputText { text } | ContentItem::OutputText { text }) = content_item
            else {
                return false;
            };
            text.contains(TEST_INSTRUCTIONS)
        })
    }));

    Ok(())
}
