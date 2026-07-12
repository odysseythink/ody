use std::sync::Arc;

use ody_exec_server::EnvironmentManager;
use ody_exec_server::ExecServerRuntimePaths;
use ody_extension_api::UserInstructionsProvider;
use ody_protocol::config_types::CollaborationMode;
use ody_protocol::config_types::ModeKind;
use ody_protocol::config_types::Settings;
use ody_protocol::error::OdyErr;
use ody_protocol::error::Result as OdyResult;
use ody_protocol::models::ResponseItem;
use ody_protocol::protocol::SessionSource;
use ody_protocol::user_input::UserInput;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::resolve_installation_id;
use crate::session::session::Session;
use crate::session::turn::build_prompt;
use crate::session::turn::built_tools;
use crate::state_db_bridge::StateDbHandle;
use crate::thread_manager::ThreadManager;
use crate::thread_manager::thread_store_from_config;
use ody_extension_api::empty_extension_registry;

/// Build the model-visible `input` list for a single debug turn.
#[doc(hidden)]
pub async fn build_prompt_input(
    mut config: Config,
    input: Vec<UserInput>,
    state_db: Option<StateDbHandle>,
    user_instructions_provider: Arc<dyn UserInstructionsProvider>,
    plan_mode: bool,
) -> OdyResult<Vec<ResponseItem>> {
    config.ephemeral = true;

    let local_runtime_paths =
        ExecServerRuntimePaths::from_optional_paths(config.ody_self_exe.clone(), None)?;

    let thread_store = thread_store_from_config(&config, state_db.clone());
    let installation_id = resolve_installation_id(&config.ody_home).await?;
    let thread_manager = ThreadManager::new(
        &config,
        SessionSource::Exec,
        Arc::new(
            EnvironmentManager::from_ody_home(config.ody_home.clone(), Some(local_runtime_paths))
                .await
                .map_err(|err| OdyErr::Fatal(err.to_string()))?,
        ),
        empty_extension_registry(),
        user_instructions_provider,
        /*analytics_events_client*/ None,
        thread_store,
        state_db.clone(),
        installation_id,
        /*attestation_provider*/ None,
        /*external_time_provider*/ None,
    );
    let thread = thread_manager.start_thread(config.clone()).await?;

    if plan_mode {
        let plan_mode_settings = Settings {
            model: config.model.clone().unwrap_or_default(),
            reasoning_effort: config.model_reasoning_effort.clone(),
            developer_instructions: Some(ody_collaboration_mode_templates::PLAN.to_string()),
            design_audit_level: None,
        };
        let plan_collaboration_mode = CollaborationMode {
            mode: ModeKind::Plan,
            settings: plan_mode_settings,
        };
        let updates = crate::session::session::SessionSettingsUpdate {
            collaboration_mode: Some(plan_collaboration_mode.clone()),
            ..Default::default()
        };
        let _ = thread
            .thread
            .ody
            .session
            .apply_debug_collaboration_mode(plan_collaboration_mode)
            .await;
    }

    let output = build_prompt_input_from_session(thread.thread.ody.session.as_ref(), input).await;
    let shutdown = thread.thread.shutdown_and_wait().await;
    let _removed = thread_manager.remove_thread(&thread.thread_id).await;

    shutdown?;
    output
}

pub(crate) async fn build_prompt_input_from_session(
    sess: &Session,
    input: Vec<UserInput>,
) -> OdyResult<Vec<ResponseItem>> {
    let user_prompt = input.iter().find_map(|u| match u {
        UserInput::Text { text, .. } => Some(text.as_str()),
        _ => None,
    });
    let turn_context = sess.new_default_turn().await;
    sess.record_context_updates_and_set_reference_context_item(turn_context.as_ref(), user_prompt)
        .await;

    if !input.is_empty() {
        let response_item = sess.response_item_from_user_input(input);
        sess.record_conversation_items(turn_context.as_ref(), std::slice::from_ref(&response_item))
            .await;
    }

    let prompt_input = sess
        .clone_history()
        .await
        .for_prompt(&turn_context.model_info.input_modalities);
    let router = built_tools(sess, turn_context.as_ref(), &CancellationToken::new()).await?;
    let base_instructions = sess.get_base_instructions().await;
    let prompt = build_prompt(
        prompt_input,
        router.as_ref(),
        turn_context.as_ref(),
        base_instructions,
    );

    Ok(prompt.input)
}
