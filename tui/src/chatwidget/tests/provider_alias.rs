//! Provider alias echo authority and rollback tests for `ChatWidget`.

use super::*;

fn minimal_thread_session(thread_id: ThreadId) -> crate::session_state::ThreadSessionState {
    crate::session_state::ThreadSessionState {
        thread_id,
        forked_from_id: None,
        fork_parent_title: None,
        thread_name: None,
        model: "kimi-for-coding".to_string(),
        model_provider_id: "kimi".to_string(),
        service_tier: None,
        approval_policy: AskForApproval::Never,
        approvals_reviewer: ApprovalsReviewer::User,
        permission_profile: PermissionProfile::read_only(),
        active_permission_profile: None,
        cwd: test_path_buf("/tmp/thread-settings").abs(),
        runtime_workspace_roots: vec![test_path_buf("/tmp/thread-settings").abs()],
        instruction_source_paths: Vec::new(),
        reasoning_effort: None,
        collaboration_mode: None,
        personality: None,
        message_history: None,
        network_proxy: None,
        rollout_path: None,
    }
}

fn thread_settings_with_provider(
    model: &str,
    provider_alias: &str,
    thread_id: ThreadId,
) -> ody_app_server_protocol::ThreadSettingsUpdatedNotification {
    ody_app_server_protocol::ThreadSettingsUpdatedNotification {
        thread_id: thread_id.to_string(),
        thread_settings: ody_app_server_protocol::ThreadSettings {
            cwd: test_path_buf("/tmp/thread-settings").abs(),
            approval_policy: AskForApproval::OnRequest,
            approvals_reviewer: ody_app_server_protocol::ApprovalsReviewer::AutoReview,
            sandbox_policy: ody_app_server_protocol::SandboxPolicy::ReadOnly {
                network_access: false,
            },
            active_permission_profile: Some(
                ody_app_server_protocol::ActivePermissionProfile::read_only(),
            ),
            model: model.to_string(),
            model_provider_alias: provider_alias.to_string(),
            service_tier: Some(ServiceTier::Fast.request_value().to_string()),
            effort: Some(ReasoningEffortConfig::High),
            summary: None,
            collaboration_mode: CollaborationMode {
                mode: ModeKind::Plan,
                settings: ody_protocol::config_types::Settings {
                    model: model.to_string(),
                    reasoning_effort: Some(ReasoningEffortConfig::High),
                    developer_instructions: None,
                    design_audit_level: None,
                },
            },
            multi_agent_mode: Default::default(),
            personality: Some(Personality::Pragmatic),
        },
    }
}

#[tokio::test]
async fn apply_thread_settings_uses_echo_as_authority_for_provider_alias() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("kimi-for-coding")).await;
    let thread_id = ThreadId::new();
    chat.handle_thread_session(minimal_thread_session(thread_id));

    chat.handle_server_notification(
        ServerNotification::ThreadSettingsUpdated(thread_settings_with_provider(
            "k3",
            "deepseek",
            thread_id,
        )),
        /*replay_kind*/ None,
    );

    assert_eq!(chat.current_provider_alias(), "deepseek");
    assert_eq!(
        chat.config_ref().model_provider,
        chat.provider_info_for_provider_id("deepseek")
    );
}

#[tokio::test]
async fn apply_thread_settings_clears_pending_provider_change() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("kimi-for-coding")).await;
    chat.record_pending_provider_change("kimi".to_string(), "deepseek".to_string());
    assert!(chat.pending_provider_change().is_some());

    let thread_id = ThreadId::new();
    chat.handle_thread_session(minimal_thread_session(thread_id));

    chat.handle_server_notification(
        ServerNotification::ThreadSettingsUpdated(thread_settings_with_provider(
            "k3",
            "deepseek",
            thread_id,
        )),
        /*replay_kind*/ None,
    );

    assert!(chat.pending_provider_change().is_none());
    assert_eq!(chat.current_provider_alias(), "deepseek");
}

#[tokio::test]
async fn race_regression_echo_overwrites_stale_pending_alias() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("kimi-for-coding")).await;
    // User selected "deepseek" locally, but an in-flight echo arrives for "glm"
    // before the request completes. The echo must remain authoritative.
    chat.record_pending_provider_change("kimi".to_string(), "deepseek".to_string());

    let thread_id = ThreadId::new();
    chat.handle_thread_session(minimal_thread_session(thread_id));

    chat.handle_server_notification(
        ServerNotification::ThreadSettingsUpdated(thread_settings_with_provider(
            "glm-4",
            "glm",
            thread_id,
        )),
        /*replay_kind*/ None,
    );

    assert!(chat.pending_provider_change().is_none());
    assert_eq!(chat.current_provider_alias(), "glm");
}

#[tokio::test]
async fn revert_provider_alias_restores_previous_alias_and_info() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("kimi-for-coding")).await;
    let original_alias = chat.current_provider_alias();
    chat.record_pending_provider_change(original_alias.clone(), "deepseek".to_string());

    // Simulate a failed thread/settings/update by reverting to the captured alias.
    chat.revert_provider_alias(&original_alias);

    assert_eq!(chat.current_provider_alias(), original_alias);
    assert_eq!(
        chat.config_ref().model_provider,
        chat.provider_info_for_provider_id(&original_alias)
    );
}
