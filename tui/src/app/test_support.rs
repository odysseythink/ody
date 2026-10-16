//! Shared App fixtures for app submodule unit tests.
//!
//! This module keeps heavyweight `App` construction and config-inspection helpers available to
//! focused sibling test modules without making `app/tests.rs` the only practical place to test
//! app-owned behavior.

use super::*;
use crate::chatwidget::tests::make_chatwidget_manual_with_sender;
use ody_models_manager::test_support::construct_model_info_offline_for_tests;
use ody_models_manager::test_support::get_model_offline_for_tests;

pub(super) async fn make_test_app() -> App {
    let (chat_widget, app_event_tx, _rx, _op_rx) = make_chatwidget_manual_with_sender().await;
    let config = chat_widget.config_ref().clone();
    let file_search = FileSearchManager::new(config.cwd.to_path_buf(), app_event_tx.clone());
    let model = get_model_offline_for_tests(config.model.as_deref());
    let session_telemetry = test_session_telemetry(&config, model.as_str());

    App {
        model_catalog: chat_widget.model_catalog(),
        session_telemetry,
        app_event_tx,
        chat_widget,
        workspace_command_runner: None,
        config,
        state_db: None,
        cli_kv_overrides: Vec::new(),
        harness_overrides: ConfigOverrides::default(),
        loader_overrides: LoaderOverrides::without_managed_config_for_tests(),
        cloud_config_bundle: CloudConfigBundleLoader::default(),
        runtime_approval_policy_override: None,
        runtime_permission_profile_override: None,
        file_search,
        transcript_cells: Vec::new(),
        transcript_view: TranscriptView::default(),
        last_transcript_copy: None,
        overlay: None,
        deferred_history_lines: Vec::new(),
        has_emitted_history_lines: false,
        transcript_reflow: TranscriptReflowState::default(),
        initial_history_replay_buffer: None,
        scrollback_has_older_history: false,
        enhanced_keys_supported: false,
        keymap: crate::keymap::RuntimeKeymap::defaults(),
        commit_anim_running: Arc::new(AtomicBool::new(false)),
        status_line_invalid_items_warned: Arc::new(AtomicBool::new(false)),
        terminal_title_invalid_items_warned: Arc::new(AtomicBool::new(false)),
        skill_load_warnings: SkillLoadWarningState::default(),
        backtrack: BacktrackState::default(),
        backtrack_render_pending: false,
        feedback: ody_feedback::OdyFeedback::new(),
        feedback_audience: FeedbackAudience::External,
        environment_manager: Arc::new(EnvironmentManager::default_for_tests()),
        app_server_target: crate::AppServerTarget::Embedded,
        pending_update_action: None,
        pending_shutdown_exit_thread_id: None,
        windows_sandbox: WindowsSandboxState::default(),
        thread_event_channels: HashMap::new(),
        thread_event_listener_tasks: HashMap::new(),
        agent_navigation: AgentNavigationState::default(),
        side_threads: HashMap::new(),
        active_thread_id: None,
        active_thread_rx: None,
        primary_thread_id: None,
        last_subagent_backfill_attempt: None,
        primary_session_configured: None,
        pending_primary_events: VecDeque::new(),
        pending_app_server_requests: PendingAppServerRequests::default(),
        pending_startup_thread_start: false,
        pending_plugin_enabled_writes: HashMap::new(),
        pending_hook_enabled_writes: HashMap::new(),
    }
}

fn test_session_telemetry(config: &Config, model: &str) -> SessionTelemetry {
    let model_info =
        construct_model_info_offline_for_tests(model, &config.to_models_manager_config());
    SessionTelemetry::new(
        ThreadId::new(),
        model,
        model_info.slug.as_str(),
        /*auth_mode*/ None,
        "test_originator".to_string(),
        /*log_user_prompts*/ false,
        "test".to_string(),
        serde_json::from_value(serde_json::json!("cli"))
            .expect("cli session source should deserialize"),
    )
}

pub(super) fn app_enabled_in_effective_config(config: &Config, app_id: &str) -> Option<bool> {
    config
        .config_layer_stack
        .effective_config()
        .as_table()
        .and_then(|table| table.get("apps"))
        .and_then(TomlValue::as_table)
        .and_then(|apps| apps.get(app_id))
        .and_then(TomlValue::as_table)
        .and_then(|app| app.get("enabled"))
        .and_then(TomlValue::as_bool)
}

/// Run an async test body on a thread with an explicit large stack.
///
/// The owned-mode paging acceptance tests (`owned_paging`, `paginated_resume`) drive the real
/// embedded app server over a 200-turn rollout, so the whole future tree is polled synchronously
/// on the test thread. That poll chain slightly exceeds the default 8 MiB test-thread stack on
/// Windows MSVC debug builds (Linux fits in 8 MiB, which is why CI never saw the overflow).
/// Run these tests on a stack with real headroom instead of weakening the assertions. The future
/// is created *inside* the spawned thread, so it never has to be `Send`.
pub(super) fn run_async_test_on_big_stack<F, Fut>(test: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = color_eyre::eyre::Result<()>>,
{
    let join = std::thread::Builder::new()
        .name("big-stack-test".to_string())
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build test runtime")
                .block_on(test())
        })
        .expect("spawn big-stack test thread")
        .join();
    match join {
        Ok(result) => result.expect("async test body failed"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}
