use super::*;

use anyhow::Result;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::path::PathBuf;

#[test]
fn extract_conversation_summary_prefers_plain_user_messages() -> Result<()> {
    let conversation_id = ThreadId::from_string("3f941c35-29b3-493b-b0a4-e25800d9aeb0")?;
    let timestamp = Some("2025-09-05T16:53:11.850Z".to_string());
    let path = PathBuf::from("rollout.jsonl");

    let head = vec![
        json!({
            "session_id": conversation_id.to_string(),
            "id": conversation_id.to_string(),
            "timestamp": timestamp,
            "cwd": "/",
            "originator": "ody",
            "cli_version": "0.0.0",
            "model_provider": "test-provider"
        }),
        json!({
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": "# AGENTS.md instructions for project\n\n<INSTRUCTIONS>\n<AGENTS.md contents>\n</INSTRUCTIONS>".to_string(),
            }],
        }),
        json!({
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": format!("<prior context> {USER_MESSAGE_BEGIN}Count to 5"),
            }],
        }),
    ];

    let session_meta = serde_json::from_value::<SessionMeta>(head[0].clone())?;

    let summary = extract_conversation_summary(
        path.clone(),
        &head,
        &session_meta,
        /*git*/ None,
        "test-provider",
        timestamp.clone(),
    )
    .expect("summary");

    let expected = ConversationSummary {
        conversation_id,
        timestamp: timestamp.clone(),
        updated_at: timestamp,
        path,
        preview: "Count to 5".to_string(),
        model_provider: "test-provider".to_string(),
        cwd: PathBuf::from("/"),
        cli_version: "0.0.0".to_string(),
        source: ody_protocol::protocol::SessionSource::VSCode,
        git_info: None,
    };

    assert_eq!(summary, expected);
    Ok(())
}

#[cfg(test)]
mod alias_validation_tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ody_protocol::config_types::ApprovalsReviewer;
    use ody_protocol::config_types::ModeKind;
    use ody_protocol::config_types::Settings;
    use ody_protocol::protocol::SessionSource;
    use ody_protocol::protocol::ThreadSettingsSnapshot;

    fn test_cwd() -> AbsolutePathBuf {
        AbsolutePathBuf::from_absolute_path("/tmp").unwrap()
    }

    fn test_runtime_providers() -> HashMap<String, ModelProviderInfo> {
        let mut providers = HashMap::new();
        providers.insert("custom-provider".to_string(), ModelProviderInfo::default());
        providers.insert("123456".to_string(), ModelProviderInfo::default());
        providers
    }

    fn test_config_snapshot(model_provider_id: &str) -> ThreadConfigSnapshot {
        let cwd = test_cwd();
        ThreadConfigSnapshot {
            model: "kimi-k2.5".to_string(),
            model_provider_id: model_provider_id.to_string(),
            service_tier: None,
            approval_policy: ody_protocol::protocol::AskForApproval::OnRequest,
            approvals_reviewer: ApprovalsReviewer::User,
            permission_profile: ody_protocol::models::PermissionProfile::read_only(),
            active_permission_profile: None,
            environments: TurnEnvironmentSelections::new(cwd.clone(), Vec::new()),
            workspace_roots: Vec::new(),
            profile_workspace_roots: Vec::new(),
            ephemeral: false,
            reasoning_effort: None,
            reasoning_summary: None,
            personality: None,
            collaboration_mode: ody_protocol::config_types::CollaborationMode {
                mode: ModeKind::Default,
                settings: Settings {
                    model: "kimi-k2.5".to_string(),
                    reasoning_effort: None,
                    developer_instructions: None,
                    design_audit_level: None,
                },
            },
            session_source: SessionSource::Cli,
            forked_from_thread_id: None,
            parent_thread_id: None,
            thread_source: None,
        }
    }

    fn test_core_snapshot(model_provider_id: &str) -> ThreadSettingsSnapshot {
        ThreadSettingsSnapshot {
            model: "kimi-k2.5".to_string(),
            model_provider_id: model_provider_id.to_string(),
            service_tier: None,
            approval_policy: ody_protocol::protocol::AskForApproval::OnRequest,
            approvals_reviewer: ApprovalsReviewer::User,
            permission_profile: ody_protocol::models::PermissionProfile::read_only(),
            active_permission_profile: None,
            cwd: test_cwd(),
            reasoning_effort: None,
            reasoning_summary: None,
            personality: None,
            collaboration_mode: ody_protocol::config_types::CollaborationMode {
                mode: ModeKind::Default,
                settings: Settings {
                    model: "kimi-k2.5".to_string(),
                    reasoning_effort: None,
                    developer_instructions: None,
                    design_audit_level: None,
                },
            },
        }
    }

    #[test]
    fn resolve_validated_provider_alias_accepts_runtime_alias() {
        let providers = test_runtime_providers();
        assert_eq!(
            resolve_validated_provider_alias("custom-provider".to_string(), &providers, "kimi"),
            "custom-provider"
        );
    }

    #[test]
    fn resolve_validated_provider_alias_accepts_builtin_alias() {
        let providers = HashMap::new();
        assert_eq!(
            resolve_validated_provider_alias("kimi".to_string(), &providers, "deepseek"),
            "kimi"
        );
    }

    #[test]
    fn resolve_validated_provider_alias_accepts_numeric_runtime_alias() {
        let providers = test_runtime_providers();
        assert_eq!(
            resolve_validated_provider_alias("123456".to_string(), &providers, "kimi"),
            "123456"
        );
    }

    #[test]
    fn resolve_validated_provider_alias_falls_back_to_default_for_unknown() {
        let providers = test_runtime_providers();
        assert_eq!(
            resolve_validated_provider_alias(
                "totally-unknown-provider".to_string(),
                &providers,
                "kimi"
            ),
            "kimi"
        );
    }

    #[test]
    fn thread_settings_from_config_snapshot_validates_alias() {
        let providers = test_runtime_providers();
        let snapshot = test_config_snapshot("custom-provider");
        let settings = thread_settings_from_config_snapshot(&snapshot, &providers);
        assert_eq!(settings.model_provider_alias, "custom-provider");
    }

    #[test]
    fn thread_settings_from_config_snapshot_falls_back_to_snapshot_alias() {
        let providers = test_runtime_providers();
        let snapshot = test_config_snapshot("totally-unknown-provider");
        let settings = thread_settings_from_config_snapshot(&snapshot, &providers);
        assert_eq!(settings.model_provider_alias, "totally-unknown-provider");
    }

    #[test]
    fn thread_settings_from_core_snapshot_validates_alias() {
        let providers = test_runtime_providers();
        let snapshot = test_core_snapshot("custom-provider");
        let settings = thread_settings_from_core_snapshot(snapshot, &providers);
        assert_eq!(settings.model_provider_alias, "custom-provider");
    }

    #[test]
    fn thread_settings_from_core_snapshot_falls_back_to_snapshot_alias() {
        let providers = test_runtime_providers();
        let snapshot = test_core_snapshot("totally-unknown-provider");
        let settings = thread_settings_from_core_snapshot(snapshot, &providers);
        assert_eq!(settings.model_provider_alias, "totally-unknown-provider");
    }
}
