use crate::bespoke_event_handling::apply_bespoke_event_handling;
use crate::bespoke_event_handling::maybe_emit_hook_prompt_item_completed;
use crate::command_exec::CommandExecManager;
use crate::command_exec::StartCommandExecParams;
use crate::config_manager::ConfigManager;
use crate::error_code::INPUT_TOO_LARGE_ERROR_CODE;
use crate::error_code::invalid_params;
use crate::models::supported_models;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::ConnectionRequestId;
use crate::outgoing_message::OutgoingMessageSender;
use crate::outgoing_message::RequestContext;
use crate::outgoing_message::ThreadScopedOutgoingMessageSender;
use crate::skills_watcher::SkillsWatcher;
use crate::thread_status::ThreadWatchManager;
use crate::thread_status::resolve_thread_status;
use chrono::Duration as ChronoDuration;
use chrono::SecondsFormat;
use ody_analytics::AnalyticsEventsClient;
use ody_analytics::AnalyticsJsonRpcError;
use ody_analytics::InputError;
use ody_analytics::TurnSteerRequestError;
use ody_app_server_protocol::AuthState;
use ody_app_server_protocol::LoginCompletedNotification;
use ody_app_server_protocol::AuthUpdatedNotification;
use ody_app_server_protocol::AdditionalContextEntry;
use ody_app_server_protocol::AdditionalContextKind;
use ody_app_server_protocol::AppInfo;
use ody_app_server_protocol::AppListUpdatedNotification;
use ody_app_server_protocol::AppSummary;
use ody_app_server_protocol::AppsListParams;
use ody_app_server_protocol::AppsListResponse;
use ody_app_server_protocol::AskForApproval;
use ody_app_server_protocol::AuthMode;
use ody_app_server_protocol::ClientInfo;
use ody_app_server_protocol::ClientRequest;
use ody_app_server_protocol::ClientResponsePayload;
use ody_app_server_protocol::OdyErrorInfo;
use ody_app_server_protocol::CollaborationModeListParams;
use ody_app_server_protocol::CollaborationModeListResponse;
use ody_app_server_protocol::CommandExecParams;
use ody_app_server_protocol::CommandExecResizeParams;
use ody_app_server_protocol::CommandExecTerminateParams;
use ody_app_server_protocol::CommandExecWriteParams;
use ody_app_server_protocol::ConfigWarningNotification;
use ody_app_server_protocol::ConsumeRateLimitResetCreditParams;
use ody_app_server_protocol::ConversationGitInfo;
use ody_app_server_protocol::ConversationSummary;
use ody_app_server_protocol::DynamicToolFunctionSpec;
use ody_app_server_protocol::DynamicToolNamespaceTool;
use ody_app_server_protocol::DynamicToolSpec;
use ody_app_server_protocol::EnvironmentAddParams;
use ody_app_server_protocol::EnvironmentAddResponse;
use ody_app_server_protocol::ExperimentalFeature as ApiExperimentalFeature;
use ody_app_server_protocol::ExperimentalFeatureListParams;
use ody_app_server_protocol::ExperimentalFeatureListResponse;
use ody_app_server_protocol::ExperimentalFeatureStage as ApiExperimentalFeatureStage;
use ody_app_server_protocol::FeedbackUploadParams;
use ody_app_server_protocol::FeedbackUploadResponse;
use ody_app_server_protocol::GetAuthStateParams;
use ody_app_server_protocol::GetAuthStateResponse;
use ody_app_server_protocol::GetAuthStatusParams;
use ody_app_server_protocol::GetAuthStatusResponse;
use ody_app_server_protocol::GetConversationSummaryParams;
use ody_app_server_protocol::GetConversationSummaryResponse;
use ody_app_server_protocol::GitDiffToRemoteParams;
use ody_app_server_protocol::GitDiffToRemoteResponse;
use ody_app_server_protocol::GitInfo as ApiGitInfo;
use ody_app_server_protocol::HookMetadata;
use ody_app_server_protocol::HooksListParams;
use ody_app_server_protocol::HooksListResponse;
use ody_app_server_protocol::InitializeParams;
use ody_app_server_protocol::InitializeResponse;
use ody_app_server_protocol::JSONRPCErrorError;
use ody_app_server_protocol::ListMcpServerStatusParams;
use ody_app_server_protocol::ListMcpServerStatusResponse;
use ody_app_server_protocol::LoginParams;
use ody_app_server_protocol::LoginResponse;
use ody_app_server_protocol::LoginApiKeyParams;
use ody_app_server_protocol::LogoutResponse;
use ody_app_server_protocol::MarketplaceAddParams;
use ody_app_server_protocol::MarketplaceAddResponse;
use ody_app_server_protocol::MarketplaceInterface;
use ody_app_server_protocol::MarketplaceRemoveParams;
use ody_app_server_protocol::MarketplaceRemoveResponse;
use ody_app_server_protocol::MarketplaceUpgradeErrorInfo;
use ody_app_server_protocol::MarketplaceUpgradeParams;
use ody_app_server_protocol::MarketplaceUpgradeResponse;
use ody_app_server_protocol::McpResourceReadParams;
use ody_app_server_protocol::McpResourceReadResponse;
use ody_app_server_protocol::McpServerOauthLoginCompletedNotification;
use ody_app_server_protocol::McpServerOauthLoginParams;
use ody_app_server_protocol::McpServerOauthLoginResponse;
use ody_app_server_protocol::McpServerRefreshResponse;
use ody_app_server_protocol::McpServerStatus;
use ody_app_server_protocol::McpServerStatusDetail;
use ody_app_server_protocol::McpServerToolCallParams;
use ody_app_server_protocol::McpServerToolCallResponse;
use ody_app_server_protocol::MemoryResetResponse;
use ody_app_server_protocol::MockExperimentalMethodParams;
use ody_app_server_protocol::MockExperimentalMethodResponse;
use ody_app_server_protocol::ModelListParams;
use ody_app_server_protocol::ModelListResponse;
use ody_app_server_protocol::PermissionProfileListParams;
use ody_app_server_protocol::PermissionProfileListResponse;
use ody_app_server_protocol::PermissionProfileSummary;
use ody_app_server_protocol::PluginDetail;
use ody_app_server_protocol::PluginInstallParams;
use ody_app_server_protocol::PluginInstallResponse;
use ody_app_server_protocol::PluginInstalledParams;
use ody_app_server_protocol::PluginInstalledResponse;
use ody_app_server_protocol::PluginInterface;
use ody_app_server_protocol::PluginListMarketplaceKind;
use ody_app_server_protocol::PluginListParams;
use ody_app_server_protocol::PluginListResponse;
use ody_app_server_protocol::PluginMarketplaceEntry;
use ody_app_server_protocol::PluginReadParams;
use ody_app_server_protocol::PluginReadResponse;
use ody_app_server_protocol::PluginShareCheckoutParams;
use ody_app_server_protocol::PluginShareCheckoutResponse;
use ody_app_server_protocol::PluginShareContext;
use ody_app_server_protocol::PluginShareDeleteParams;
use ody_app_server_protocol::PluginShareDeleteResponse;
use ody_app_server_protocol::PluginShareListParams;
use ody_app_server_protocol::PluginShareListResponse;
use ody_app_server_protocol::PluginShareSaveParams;
use ody_app_server_protocol::PluginShareSaveResponse;
use ody_app_server_protocol::PluginShareUpdateTargetsParams;
use ody_app_server_protocol::PluginShareUpdateTargetsResponse;
use ody_app_server_protocol::PluginSkillReadParams;
use ody_app_server_protocol::PluginSkillReadResponse;
use ody_app_server_protocol::PluginSource;
use ody_app_server_protocol::PluginSummary;
use ody_app_server_protocol::PluginUninstallParams;
use ody_app_server_protocol::PluginUninstallResponse;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::ReviewDelivery as ApiReviewDelivery;
use ody_app_server_protocol::ReviewStartParams;
use ody_app_server_protocol::ReviewStartResponse;
use ody_app_server_protocol::ReviewTarget as ApiReviewTarget;
use ody_app_server_protocol::SandboxMode;
use ody_app_server_protocol::SendAddCreditsNudgeEmailParams;
use ody_app_server_protocol::ServerNotification;
use ody_app_server_protocol::ServerRequestResolvedNotification;
use ody_app_server_protocol::SkillSummary;
use ody_app_server_protocol::SkillsConfigWriteParams;
use ody_app_server_protocol::SkillsConfigWriteResponse;
use ody_app_server_protocol::SkillsExtraRootsSetParams;
use ody_app_server_protocol::SkillsExtraRootsSetResponse;
use ody_app_server_protocol::SkillsListParams;
use ody_app_server_protocol::SkillsListResponse;
use ody_app_server_protocol::SortDirection;
use ody_app_server_protocol::Thread;
use ody_app_server_protocol::ThreadApproveGuardianDeniedActionParams;
use ody_app_server_protocol::ThreadApproveGuardianDeniedActionResponse;
use ody_app_server_protocol::ThreadArchiveParams;
use ody_app_server_protocol::ThreadArchiveResponse;
use ody_app_server_protocol::ThreadArchivedNotification;
use ody_app_server_protocol::ThreadBackgroundTerminal;
use ody_app_server_protocol::ThreadBackgroundTerminalsCleanParams;
use ody_app_server_protocol::ThreadBackgroundTerminalsCleanResponse;
use ody_app_server_protocol::ThreadBackgroundTerminalsListParams;
use ody_app_server_protocol::ThreadBackgroundTerminalsListResponse;
use ody_app_server_protocol::ThreadBackgroundTerminalsTerminateParams;
use ody_app_server_protocol::ThreadBackgroundTerminalsTerminateResponse;
use ody_app_server_protocol::ThreadClosedNotification;
use ody_app_server_protocol::ThreadCompactStartParams;
use ody_app_server_protocol::ThreadCompactStartResponse;
use ody_app_server_protocol::ThreadDecrementElicitationParams;
use ody_app_server_protocol::ThreadDecrementElicitationResponse;
use ody_app_server_protocol::ThreadDeleteParams;
use ody_app_server_protocol::ThreadDeleteResponse;
use ody_app_server_protocol::ThreadDeletedNotification;
use ody_app_server_protocol::ThreadForkParams;
use ody_app_server_protocol::ThreadForkResponse;
use ody_app_server_protocol::ThreadGoal;
use ody_app_server_protocol::ThreadGoalClearParams;
use ody_app_server_protocol::ThreadGoalClearResponse;
use ody_app_server_protocol::ThreadGoalClearedNotification;
use ody_app_server_protocol::ThreadGoalGetParams;
use ody_app_server_protocol::ThreadGoalGetResponse;
use ody_app_server_protocol::ThreadGoalSetParams;
use ody_app_server_protocol::ThreadGoalSetResponse;
use ody_app_server_protocol::ThreadGoalStatus;
use ody_app_server_protocol::ThreadGoalUpdatedNotification;
use ody_app_server_protocol::ThreadHistoryBuilder;
use ody_app_server_protocol::ThreadIncrementElicitationParams;
use ody_app_server_protocol::ThreadIncrementElicitationResponse;
use ody_app_server_protocol::ThreadInjectItemsParams;
use ody_app_server_protocol::ThreadInjectItemsResponse;
use ody_app_server_protocol::ThreadItem;
use ody_app_server_protocol::ThreadListCwdFilter;
use ody_app_server_protocol::ThreadListParams;
use ody_app_server_protocol::ThreadListResponse;
use ody_app_server_protocol::ThreadLoadedListParams;
use ody_app_server_protocol::ThreadLoadedListResponse;
use ody_app_server_protocol::ThreadMemoryModeSetParams;
use ody_app_server_protocol::ThreadMemoryModeSetResponse;
use ody_app_server_protocol::ThreadMetadataGitInfoUpdateParams;
use ody_app_server_protocol::ThreadMetadataUpdateParams;
use ody_app_server_protocol::ThreadMetadataUpdateResponse;
use ody_app_server_protocol::ThreadNameUpdatedNotification;
use ody_app_server_protocol::ThreadReadParams;
use ody_app_server_protocol::ThreadReadResponse;
use ody_app_server_protocol::ThreadRealtimeAppendAudioParams;
use ody_app_server_protocol::ThreadRealtimeAppendAudioResponse;
use ody_app_server_protocol::ThreadRealtimeAppendSpeechParams;
use ody_app_server_protocol::ThreadRealtimeAppendSpeechResponse;
use ody_app_server_protocol::ThreadRealtimeAppendTextParams;
use ody_app_server_protocol::ThreadRealtimeAppendTextResponse;
use ody_app_server_protocol::ThreadRealtimeListVoicesResponse;
use ody_app_server_protocol::ThreadRealtimeStartParams;
use ody_app_server_protocol::ThreadRealtimeStartResponse;
use ody_app_server_protocol::ThreadRealtimeStartTransport;
use ody_app_server_protocol::ThreadRealtimeStopParams;
use ody_app_server_protocol::ThreadRealtimeStopResponse;
use ody_app_server_protocol::ThreadResumeInitialTurnsPageParams;
use ody_app_server_protocol::ThreadResumeParams;
use ody_app_server_protocol::ThreadResumeResponse;
use ody_app_server_protocol::ThreadRollbackParams;
use ody_app_server_protocol::ThreadSearchParams;
use ody_app_server_protocol::ThreadSearchResponse;
use ody_app_server_protocol::ThreadSearchResult;
use ody_app_server_protocol::ThreadSetNameParams;
use ody_app_server_protocol::ThreadSetNameResponse;
use ody_app_server_protocol::ThreadSettings;
use ody_app_server_protocol::ThreadSettingsUpdateParams;
use ody_app_server_protocol::ThreadSettingsUpdateResponse;
use ody_app_server_protocol::ThreadShellCommandParams;
use ody_app_server_protocol::ThreadShellCommandResponse;
use ody_app_server_protocol::ThreadSortKey;
use ody_app_server_protocol::ThreadSourceKind;
use ody_app_server_protocol::ThreadStartParams;
use ody_app_server_protocol::ThreadStartResponse;
use ody_app_server_protocol::ThreadStartedNotification;
use ody_app_server_protocol::ThreadStatus;
use ody_app_server_protocol::ThreadTurnsItemsListParams;
use ody_app_server_protocol::ThreadTurnsListParams;
use ody_app_server_protocol::ThreadTurnsListResponse;
use ody_app_server_protocol::ThreadUnarchiveParams;
use ody_app_server_protocol::ThreadUnarchiveResponse;
use ody_app_server_protocol::ThreadUnarchivedNotification;
use ody_app_server_protocol::ThreadUnsubscribeParams;
use ody_app_server_protocol::ThreadUnsubscribeResponse;
use ody_app_server_protocol::ThreadUnsubscribeStatus;
use ody_app_server_protocol::Turn;
use ody_app_server_protocol::TurnEnvironmentParams;
use ody_app_server_protocol::TurnError;
use ody_app_server_protocol::TurnInterruptParams;
use ody_app_server_protocol::TurnInterruptResponse;
use ody_app_server_protocol::TurnItemsView;
use ody_app_server_protocol::TurnStartParams;
use ody_app_server_protocol::TurnStartResponse;
use ody_app_server_protocol::TurnStatus;
use ody_app_server_protocol::TurnSteerParams;
use ody_app_server_protocol::TurnSteerResponse;
use ody_app_server_protocol::UserInput as V2UserInput;
use ody_app_server_protocol::WindowsSandboxReadiness;
use ody_app_server_protocol::WindowsSandboxReadinessResponse;
use ody_app_server_protocol::WindowsSandboxSetupCompletedNotification;
use ody_app_server_protocol::WindowsSandboxSetupMode;
use ody_app_server_protocol::WindowsSandboxSetupStartParams;
use ody_app_server_protocol::WindowsSandboxSetupStartResponse;
use ody_arg0::Arg0DispatchPaths;
use ody_core::connectors;
use ody_core::workspace_settings;
use ody_config::CloudConfigBundleLoadError;
use ody_config::CloudConfigBundleLoadErrorCode;
use ody_config::ConfigLayerStack;
use ody_config::loader::project_trust_key;
use ody_config::types::McpServerTransportConfig;
use ody_core::OdyThread;
use ody_core::OdyThreadSettingsOverrides;
use ody_core::ForkSnapshot;
use ody_core::McpManager;
use ody_core::NewThread;
#[cfg(test)]
use ody_core::SessionMeta;
use ody_core::StartThreadOptions;
use ody_core::SteerInputError;
use ody_core::ThreadConfigSnapshot;
use ody_core::ThreadManager;
use ody_core::config::Config;
use ody_core::config::ConfigOverrides;
use ody_core::config::NetworkProxyAuditMetadata;
use ody_core::config::edit::ConfigEdit;
use ody_core::config::edit::ConfigEditsBuilder;
use ody_core::connectors::AccessibleConnectorsStatus;
use ody_core::exec::ExecCapturePolicy;
use ody_core::exec::ExecExpiration;
use ody_core::exec::ExecParams;
use ody_core::exec_env::create_env;
use ody_core::path_utils;
#[cfg(test)]
use ody_core::read_head_for_summary;
use ody_core::sandboxing::SandboxPermissions;
use ody_core::windows_sandbox::WindowsSandboxLevelExt;
use ody_core::windows_sandbox::WindowsSandboxSetupMode as CoreWindowsSandboxSetupMode;
use ody_core::windows_sandbox::WindowsSandboxSetupRequest;
use ody_core::windows_sandbox::sandbox_setup_is_complete;
use ody_core_plugins::PluginInstallError as CorePluginInstallError;
use ody_core_plugins::PluginInstallRequest;
use ody_core_plugins::PluginReadRequest;
use ody_core_plugins::PluginUninstallError as CorePluginUninstallError;
use ody_core_plugins::PluginsManager;
use ody_core_plugins::loader::load_plugin_apps;
use ody_core_plugins::loader::load_plugin_mcp_servers;
use ody_core_plugins::manifest::PluginManifestInterface;
use ody_core_plugins::marketplace::MarketplaceError;
use ody_core_plugins::marketplace::MarketplacePluginSource;
use ody_core_plugins::marketplace_add::MarketplaceAddError;
use ody_core_plugins::marketplace_add::MarketplaceAddRequest;
use ody_core_plugins::marketplace_add::add_marketplace as add_marketplace_to_ody_home;
use ody_core_plugins::marketplace_remove::MarketplaceRemoveError;
use ody_core_plugins::marketplace_remove::MarketplaceRemoveRequest as CoreMarketplaceRemoveRequest;
use ody_core_plugins::marketplace_remove::remove_marketplace;
use ody_exec_server::EnvironmentManager;
use ody_exec_server::LOCAL_ENVIRONMENT_ID;
use ody_exec_server::LOCAL_FS;
use ody_features::FEATURES;
use ody_features::Feature;
use ody_features::Stage;
use ody_feedback::OdyFeedback;
use ody_feedback::FeedbackAttachmentPath;
use ody_feedback::FeedbackUploadOptions;
use ody_git_utils::git_diff_to_remote;
use ody_git_utils::resolve_root_git_project_for_trust;
use ody_mcp::McpRuntimeContext;
use ody_mcp::McpServerStatusSnapshot;
use ody_mcp::McpSnapshotDetail;
use ody_mcp::collect_mcp_server_status_snapshot_with_detail;
use ody_mcp::discover_supported_scopes;
use ody_mcp::read_mcp_resource as read_mcp_resource_without_thread;
use ody_mcp::resolve_oauth_scopes;
use ody_memories_write::clear_memory_roots_contents;
use ody_model_provider::create_model_provider;
use ody_models_manager::collaboration_mode_presets::builtin_collaboration_mode_presets;
use ody_protocol::ThreadId;
use ody_protocol::config_types::CollaborationMode;
use ody_protocol::config_types::Personality;
use ody_protocol::config_types::ReasoningSummary;
use ody_protocol::config_types::TrustLevel;
use ody_protocol::config_types::WindowsSandboxLevel;
use ody_protocol::error::OdyErr;
use ody_protocol::error::Result as OdyResult;
#[cfg(test)]
use ody_protocol::items::TurnItem;
use ody_protocol::models::ResponseItem;
use ody_protocol::odysseythink_models::ReasoningEffort;
#[cfg(test)]
use ody_protocol::permissions::FileSystemSandboxPolicy;
use ody_protocol::protocol::AgentStatus;
use ody_protocol::protocol::ConversationAudioParams;
use ody_protocol::protocol::ConversationSpeechParams;
use ody_protocol::protocol::ConversationStartParams;
use ody_protocol::protocol::ConversationStartTransport;
use ody_protocol::protocol::ConversationTextParams;
use ody_protocol::protocol::EventMsg;
#[cfg(test)]
use ody_protocol::protocol::GitInfo as CoreGitInfo;
use ody_protocol::protocol::InitialHistory;
use ody_protocol::protocol::McpAuthStatus as CoreMcpAuthStatus;
use ody_protocol::protocol::Op;
use ody_protocol::protocol::RealtimeVoicesList;
use ody_protocol::protocol::ResumedHistory;
use ody_protocol::protocol::ReviewDelivery as CoreReviewDelivery;
use ody_protocol::protocol::ReviewRequest;
use ody_protocol::protocol::ReviewTarget as CoreReviewTarget;
use ody_protocol::protocol::RolloutItem;
use ody_protocol::protocol::SessionConfiguredEvent;
#[cfg(test)]
use ody_protocol::protocol::SessionMetaLine;
use ody_protocol::protocol::TurnEnvironmentSelection;
use ody_protocol::protocol::TurnEnvironmentSelections;
use ody_protocol::protocol::USER_MESSAGE_BEGIN;
use ody_protocol::protocol::W3cTraceContext;
use ody_protocol::user_input::MAX_USER_INPUT_TEXT_CHARS;
use ody_protocol::user_input::UserInput as CoreInputItem;
use ody_rmcp_client::perform_oauth_login_return_url;
use ody_rollout::is_persisted_rollout_item;
use ody_rollout::state_db::StateDbHandle;
use ody_rollout::state_db::reconcile_rollout;
use ody_state::ThreadMetadata;
use ody_state::log_db::LogDbLayer;
use ody_thread_store::ArchiveThreadParams as StoreArchiveThreadParams;
use ody_thread_store::DeleteThreadParams as StoreDeleteThreadParams;
use ody_thread_store::GitInfoPatch as StoreGitInfoPatch;
use ody_thread_store::ListThreadsParams as StoreListThreadsParams;
use ody_thread_store::LocalThreadStore;
use ody_thread_store::ReadThreadByRolloutPathParams as StoreReadThreadByRolloutPathParams;
use ody_thread_store::ReadThreadParams as StoreReadThreadParams;
use ody_thread_store::SearchThreadsParams as StoreSearchThreadsParams;
use ody_thread_store::SortDirection as StoreSortDirection;
use ody_thread_store::StoredThread;
use ody_thread_store::ThreadMetadataPatch as StoreThreadMetadataPatch;
use ody_thread_store::ThreadSortKey as StoreThreadSortKey;
use ody_thread_store::ThreadStore;
use ody_thread_store::ThreadStoreError;
use ody_utils_absolute_path::AbsolutePathBuf;
use ody_utils_pty::DEFAULT_OUTPUT_BYTES_CAP;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::result::Result;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio::sync::Semaphore;
use tokio::sync::SemaphorePermit;
use tokio::sync::broadcast;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::sync::DropGuard;
use tokio_util::task::TaskTracker;
use toml::Value as TomlValue;
use tracing::Instrument;
use tracing::error;
use tracing::info;
use tracing::warn;

#[cfg(test)]
use ody_app_server_protocol::ServerRequest;

mod account_processor;
mod apps_processor;
mod catalog_processor;
mod command_exec_processor;
mod config_processor;
mod environment_processor;
mod feedback_doctor_report;
mod feedback_processor;
mod fs_processor;
mod git_processor;
mod initialize_processor;
mod marketplace_processor;
mod mcp_processor;
mod plugins;
mod process_exec_processor;
mod search;
mod thread_processor;
mod token_usage_replay;
mod turn_processor;
mod windows_sandbox_processor;

pub(crate) use account_processor::AccountRequestProcessor;
pub(crate) use apps_processor::AppsRequestProcessor;
pub(crate) use catalog_processor::CatalogRequestProcessor;
pub(crate) use command_exec_processor::CommandExecRequestProcessor;
pub(crate) use config_processor::ConfigRequestProcessor;
pub(crate) use environment_processor::EnvironmentRequestProcessor;
pub(crate) use feedback_processor::FeedbackRequestProcessor;
pub(crate) use fs_processor::FsRequestProcessor;
pub(crate) use git_processor::GitRequestProcessor;
pub(crate) use initialize_processor::InitializeRequestProcessor;
pub(crate) use marketplace_processor::MarketplaceRequestProcessor;
pub(crate) use mcp_processor::McpRequestProcessor;
pub(crate) use plugins::PluginRequestProcessor;
pub(crate) use process_exec_processor::ProcessExecRequestProcessor;
pub(crate) use search::SearchRequestProcessor;
pub(crate) use thread_goal_processor::ThreadGoalRequestProcessor;
pub(crate) use thread_processor::ThreadRequestProcessor;
pub(crate) use turn_processor::TurnRequestProcessor;
pub(crate) use windows_sandbox_processor::WindowsSandboxRequestProcessor;

use crate::error_code::internal_error;
use crate::error_code::invalid_request;
use crate::filters::compute_source_filters;
use crate::filters::source_kind_matches;
use crate::thread_state::ConnectionCapabilities;
use crate::thread_state::ThreadListenerCommand;
use crate::thread_state::ThreadState;
use crate::thread_state::ThreadStateManager;
use token_usage_replay::latest_token_usage_turn_id_from_rollout_items;
use token_usage_replay::send_thread_token_usage_update_to_connection;

fn resolve_request_cwd(cwd: Option<PathBuf>) -> Result<Option<AbsolutePathBuf>, JSONRPCErrorError> {
    cwd.map(|cwd| {
        AbsolutePathBuf::relative_to_current_dir(path_utils::normalize_for_native_workdir(cwd))
            .map_err(|err| invalid_request(format!("invalid cwd: {err}")))
    })
    .transpose()
}

fn resolve_turn_environment_selections(
    thread_manager: &ThreadManager,
    environments: Option<Vec<TurnEnvironmentParams>>,
) -> Result<Option<Vec<TurnEnvironmentSelection>>, JSONRPCErrorError> {
    let Some(environments) = environments else {
        return Ok(None);
    };
    let mut selections = Vec::with_capacity(environments.len());
    for environment in environments {
        let environment_id = environment.environment_id;
        let cwd = environment
            .cwd
            .to_inferred_path_uri()
            .ok_or_else(|| {
                invalid_request(format!(
                    "invalid cwd for environment `{environment_id}`: path `{}` does not use absolute POSIX or Windows path syntax",
                    environment.cwd
                ))
            })?;
        selections.push(TurnEnvironmentSelection {
            environment_id,
            cwd,
        });
    }
    thread_manager
        .validate_environment_selections(&selections)
        .map_err(environment_selection_error)?;
    Ok(Some(selections))
}

fn resolve_runtime_workspace_roots(workspace_roots: Vec<AbsolutePathBuf>) -> Vec<AbsolutePathBuf> {
    let mut resolved_roots = Vec::new();
    for root in workspace_roots {
        if !resolved_roots.iter().any(|existing| existing == &root) {
            resolved_roots.push(root);
        }
    }
    resolved_roots
}

mod config_errors;
mod request_errors;
mod thread_delete;
mod thread_goal_processor;
mod thread_lifecycle;
mod thread_summary;

use self::config_errors::*;
use self::request_errors::*;
use self::thread_goal_processor::api_thread_goal_from_state;
use self::thread_lifecycle::*;
use self::thread_summary::*;

pub(crate) use self::thread_lifecycle::populate_thread_turns_from_history;
pub(crate) use self::thread_processor::thread_from_stored_thread;
#[cfg(test)]
pub(crate) use self::thread_summary::read_summary_from_rollout;
#[cfg(test)]
pub(crate) use self::thread_summary::summary_to_thread;
pub(crate) use self::thread_summary::thread_settings_from_config_snapshot;
pub(crate) use self::thread_summary::thread_settings_from_core_snapshot;

pub(crate) fn build_api_turns_from_rollout_items(items: &[RolloutItem]) -> Vec<Turn> {
    let mut builder = ThreadHistoryBuilder::new();
    for item in items {
        if is_persisted_rollout_item(item) {
            builder.handle_rollout_item(item);
        }
    }
    builder.finish()
}
