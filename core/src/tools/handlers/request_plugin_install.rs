use std::collections::HashSet;

use ody_app_server_protocol::AppInfo;
use ody_config::types::ToolSuggestDisabledTool;
use ody_mcp::ODY_APPS_MCP_SERVER_NAME;
use ody_rmcp_client::ElicitationAction;
use ody_rmcp_client::ElicitationResponse;
use ody_tools::DiscoverableTool;
use ody_tools::DiscoverableToolAction;
use ody_tools::DiscoverableToolType;
use ody_tools::LIST_AVAILABLE_PLUGINS_TO_INSTALL_TOOL_NAME;
use ody_tools::REQUEST_PLUGIN_INSTALL_PERSIST_ALWAYS_VALUE;
use ody_tools::REQUEST_PLUGIN_INSTALL_PERSIST_KEY;
use ody_tools::REQUEST_PLUGIN_INSTALL_TOOL_NAME;
use ody_tools::RequestPluginInstallArgs;
use ody_tools::RequestPluginInstallResult;
use ody_tools::ToolName;
use ody_tools::ToolSpec;
use ody_tools::all_requested_connectors_picked_up;
use ody_tools::build_request_plugin_install_elicitation_request;
use ody_tools::filter_request_plugin_install_discoverable_tools_for_client;
use ody_tools::verified_connector_install_completed;
use rmcp::model::RequestId;
use serde::Deserialize;
use serde_json::Value;
use tracing::warn;

use crate::config::edit::ConfigEdit;
use crate::config::edit::ConfigEditsBuilder;
use crate::connectors;
use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::request_plugin_install_spec::create_request_plugin_install_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use crate::tools::router::ToolSuggestPresentation;

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct RecommendedPluginInstallArgs {
    #[serde(alias = "tool_id")]
    plugin_id: String,
    suggest_reason: String,
}

pub struct RequestPluginInstallHandler {
    discoverable_tools: Vec<DiscoverableTool>,
    presentation: ToolSuggestPresentation,
}

impl RequestPluginInstallHandler {
    pub(crate) fn new(
        discoverable_tools: Vec<DiscoverableTool>,
        presentation: ToolSuggestPresentation,
    ) -> Self {
        Self {
            discoverable_tools,
            presentation,
        }
    }
}

impl ToolExecutor<ToolInvocation> for RequestPluginInstallHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(REQUEST_PLUGIN_INSTALL_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_request_plugin_install_tool(self.presentation)
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(invocation))
    }
}

impl RequestPluginInstallHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            payload,
            session,
            turn,
            call_id,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::Fatal(format!(
                    "{REQUEST_PLUGIN_INSTALL_TOOL_NAME} handler received unsupported payload"
                )));
            }
        };

        let (requested_tool_id, requested_tool_type, suggest_reason) = match self.presentation {
            ToolSuggestPresentation::ListTool => {
                let args: RequestPluginInstallArgs = parse_arguments(&arguments)?;
                if args.action_type != DiscoverableToolAction::Install {
                    return Err(FunctionCallError::RespondToModel(
                        "plugin install requests currently support only action_type=\"install\""
                            .to_string(),
                    ));
                }
                (args.tool_id, Some(args.tool_type), args.suggest_reason)
            }
            ToolSuggestPresentation::RecommendationContext => {
                let args: RecommendedPluginInstallArgs = parse_arguments(&arguments)?;
                (args.plugin_id, None, args.suggest_reason)
            }
        };
        let suggest_reason = suggest_reason.trim();
        if suggest_reason.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "suggest_reason must not be empty".to_string(),
            ));
        }
        if (requested_tool_type == Some(DiscoverableToolType::Plugin)
            || self.presentation == ToolSuggestPresentation::RecommendationContext)
            && turn.app_server_client_name.as_deref() == Some("ody-tui")
        {
            return Err(FunctionCallError::RespondToModel(
                "plugin install requests are not available in ody-tui yet".to_string(),
            ));
        }

        let discoverable_tools = filter_request_plugin_install_discoverable_tools_for_client(
            self.discoverable_tools.clone(),
            turn.app_server_client_name.as_deref(),
        );

        let tool = discoverable_tools
            .into_iter()
            .find(|tool| {
                tool.id() == requested_tool_id
                    && match self.presentation {
                        ToolSuggestPresentation::ListTool => {
                            Some(tool.tool_type()) == requested_tool_type
                        }
                        ToolSuggestPresentation::RecommendationContext => {
                            matches!(tool, DiscoverableTool::Plugin(_))
                        }
                    }
            })
            .ok_or_else(|| {
                let (argument_name, source) = match self.presentation {
                    ToolSuggestPresentation::ListTool => (
                        "tool_id",
                        format!(
                            "the discoverable tools returned by {LIST_AVAILABLE_PLUGINS_TO_INSTALL_TOOL_NAME}"
                        ),
                    ),
                    ToolSuggestPresentation::RecommendationContext => (
                        "plugin_id",
                        "the entries in the <recommended_plugins> list".to_string(),
                    ),
                };
                FunctionCallError::RespondToModel(format!(
                    "{argument_name} must match one of {source}"
                ))
            })?;
        let tool_type = tool.tool_type();

        let request_id = RequestId::String(format!("request_plugin_install_{call_id}").into());
        let params = build_request_plugin_install_elicitation_request(
            ODY_APPS_MCP_SERVER_NAME,
            session.thread_id.to_string(),
            turn.sub_id.clone(),
            suggest_reason,
            &tool,
        );
        let elicitation = session
            .request_mcp_server_elicitation(turn.as_ref(), request_id, params)
            .await;
        let response = elicitation.response;
        if let Some(response) = response.as_ref() {
            maybe_persist_disabled_install_request(&session, &turn, &tool, response).await;
        }
        let user_confirmed = response
            .as_ref()
            .is_some_and(|response| response.action == ElicitationAction::Accept);

        let completed = if user_confirmed {
            verify_request_plugin_install_completed(&session, &turn, &tool).await
        } else {
            false
        };

        if completed && let DiscoverableTool::Connector(connector) = &tool {
            session
                .merge_connector_selection(HashSet::from([connector.id.clone()]))
                .await;
        }

        if elicitation.sent {
            let tool_type = match tool_type {
                DiscoverableToolType::Connector => "connector",
                DiscoverableToolType::Plugin => "plugin",
            };
            let response_action = match response.as_ref().map(|response| &response.action) {
                Some(ElicitationAction::Accept) => "accept",
                Some(ElicitationAction::Decline) => "decline",
                Some(ElicitationAction::Cancel) => "cancel",
                None => "unavailable",
            };
            turn.session_telemetry.record_plugin_install_suggestion(
                tool_type,
                tool.id(),
                tool.name(),
                response_action,
                user_confirmed,
                completed,
            );
        }

        let content = serde_json::to_string(&RequestPluginInstallResult {
            completed,
            user_confirmed,
            tool_type,
            action_type: DiscoverableToolAction::Install,
            tool_id: tool.id().to_string(),
            tool_name: tool.name().to_string(),
            suggest_reason: suggest_reason.to_string(),
        })
        .map_err(|err| {
            FunctionCallError::Fatal(format!(
                "failed to serialize {REQUEST_PLUGIN_INSTALL_TOOL_NAME} response: {err}"
            ))
        })?;

        Ok(boxed_tool_output(FunctionToolOutput::from_text(
            content,
            Some(true),
        )))
    }
}

impl CoreToolRuntime for RequestPluginInstallHandler {}

async fn maybe_persist_disabled_install_request(
    session: &crate::session::session::Session,
    turn: &crate::session::turn_context::TurnContext,
    tool: &DiscoverableTool,
    response: &ElicitationResponse,
) {
    if !request_plugin_install_response_requests_persistent_disable(response) {
        return;
    }

    if let Err(err) = persist_disabled_install_request(&turn.config.ody_home, tool).await {
        warn!(
            error = %err,
            tool_id = tool.id(),
            "failed to persist disabled tool suggestion"
        );
        return;
    }

    session.reload_user_config_layer().await;
}

fn request_plugin_install_response_requests_persistent_disable(
    response: &ElicitationResponse,
) -> bool {
    if response.action != ElicitationAction::Decline {
        return false;
    }

    response
        .meta
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|meta| meta.get(REQUEST_PLUGIN_INSTALL_PERSIST_KEY))
        .and_then(Value::as_str)
        == Some(REQUEST_PLUGIN_INSTALL_PERSIST_ALWAYS_VALUE)
}

async fn persist_disabled_install_request(
    ody_home: &ody_utils_absolute_path::AbsolutePathBuf,
    tool: &DiscoverableTool,
) -> anyhow::Result<()> {
    ConfigEditsBuilder::new(ody_home)
        .with_edits([ConfigEdit::AddToolSuggestDisabledTool(
            disabled_install_request(tool),
        )])
        .apply()
        .await
}

fn disabled_install_request(tool: &DiscoverableTool) -> ToolSuggestDisabledTool {
    match tool {
        DiscoverableTool::Connector(connector) => {
            ToolSuggestDisabledTool::connector(connector.id.as_str())
        }
        DiscoverableTool::Plugin(plugin) => ToolSuggestDisabledTool::plugin(plugin.id.as_str()),
    }
}

async fn verify_request_plugin_install_completed(
    session: &crate::session::session::Session,
    turn: &crate::session::turn_context::TurnContext,
    tool: &DiscoverableTool,
) -> bool {
    match tool {
        DiscoverableTool::Connector(connector) => refresh_missing_requested_connectors(
            session,
            turn,
            std::slice::from_ref(&connector.id),
            connector.id.as_str(),
        )
        .await
        .is_some_and(|accessible_connectors| {
            verified_connector_install_completed(connector.id.as_str(), &accessible_connectors)
        }),
        DiscoverableTool::Plugin(plugin) => {
            session.reload_user_config_layer().await;
            let config = session.get_config().await;
            let completed = verified_plugin_install_completed(
                plugin.id.as_str(),
                config.as_ref(),
                session.services.plugins_manager.as_ref(),
            );
            let _ = refresh_missing_requested_connectors(
                session,
                turn,
                &plugin.app_connector_ids,
                plugin.id.as_str(),
            )
            .await;
            completed
        }
    }
}

async fn refresh_missing_requested_connectors(
    session: &crate::session::session::Session,
    turn: &crate::session::turn_context::TurnContext,
    expected_connector_ids: &[String],
    tool_id: &str,
) -> Option<Vec<AppInfo>> {
    if expected_connector_ids.is_empty() {
        return Some(Vec::new());
    }

    let manager = session.services.mcp_connection_manager.load_full();
    let mcp_tools = manager.list_all_tools().await;
    let accessible_connectors = connectors::with_app_enabled_state(
        connectors::accessible_connectors_from_mcp_tools(&mcp_tools),
        &turn.config,
    );
    if all_requested_connectors_picked_up(expected_connector_ids, &accessible_connectors) {
        return Some(accessible_connectors);
    }

    match manager.hard_refresh_ody_apps_tools_cache().await {
        Ok(mcp_tools) => {
            let accessible_connectors = connectors::with_app_enabled_state(
                connectors::accessible_connectors_from_mcp_tools(&mcp_tools),
                &turn.config,
            );
            connectors::refresh_accessible_connectors_cache_from_mcp_tools(
                &turn.config,
                &mcp_tools,
            );
            Some(accessible_connectors)
        }
        Err(err) => {
            warn!(
                "failed to refresh ody apps tools cache after plugin install request for {tool_id}: {err:#}"
            );
            None
        }
    }
}

fn verified_plugin_install_completed(
    tool_id: &str,
    config: &crate::config::Config,
    plugins_manager: &ody_core_plugins::PluginsManager,
) -> bool {
    let plugins_input = config.plugins_config_input();
    plugins_manager
        .list_marketplaces_for_config(
            &plugins_input,
            &[],
            /*include_odysseythink_curated*/ true,
        )
        .ok()
        .into_iter()
        .flat_map(|outcome| outcome.marketplaces)
        .flat_map(|marketplace| marketplace.plugins.into_iter())
        .any(|plugin| plugin.id == tool_id && plugin.installed)
}
