use std::sync::Arc;

use ody_core_skills::HostSkillsSnapshot;
use ody_extension_api::FunctionCallError;
use ody_extension_api::JsonToolOutput;
use ody_extension_api::ResponsesApiTool;
use ody_extension_api::ToolCall;
use ody_extension_api::ToolExecutor;
use ody_extension_api::ToolName;
use ody_extension_api::ToolOutput;
use ody_extension_api::ToolSpec;
use ody_extension_api::parse_tool_input_schema;
use ody_mcp::McpResourceClient;
use ody_mcp::ODY_APPS_MCP_SERVER_NAME;
use ody_tools::ResponsesApiNamespace;
use ody_tools::ResponsesApiNamespaceTool;
use ody_tools::default_namespace_description;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::catalog::SkillAuthority;
use crate::catalog::SkillCatalog;
use crate::catalog::SkillSourceKind;
use crate::provider::SkillListQuery;
use crate::sources::SkillProviders;
use crate::state::SkillsThreadState;

mod list;
mod read;
mod schema;

const SKILLS_NAMESPACE: &str = "skills";
const MAX_HANDLE_BYTES: usize = 2_048;

pub(crate) fn skill_tools(
    providers: SkillProviders,
    mcp_resources: Option<Arc<McpResourceClient>>,
    host_snapshot: Option<Arc<HostSkillsSnapshot>>,
    thread_state: Arc<SkillsThreadState>,
    host_enabled: bool,
    executor_enabled: bool,
    orchestrator_enabled: bool,
) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
    let context = SkillToolContext {
        providers,
        mcp_resources,
        host_snapshot,
        thread_state,
        host_enabled,
        executor_enabled,
        orchestrator_enabled,
    };
    vec![
        Arc::new(list::ListTool {
            context: context.clone(),
        }),
        Arc::new(read::ReadTool { context }),
    ]
}

#[derive(Clone)]
struct SkillToolContext {
    providers: SkillProviders,
    mcp_resources: Option<Arc<McpResourceClient>>,
    host_snapshot: Option<Arc<HostSkillsSnapshot>>,
    thread_state: Arc<SkillsThreadState>,
    host_enabled: bool,
    executor_enabled: bool,
    orchestrator_enabled: bool,
}

impl SkillToolContext {
    async fn catalog(&self, turn_id: &str, authority: SkillToolAuthority) -> SkillCatalog {
        let mode = self.thread_state.mode();
        match authority {
            SkillToolAuthority::Host if !self.host_enabled => SkillCatalog::default(),
            SkillToolAuthority::Executor { .. } if !self.executor_enabled => {
                SkillCatalog::default()
            }
            SkillToolAuthority::Orchestrator if !self.orchestrator_enabled => {
                SkillCatalog::default()
            }
            SkillToolAuthority::Host => {
                self.providers
                    .list_for_turn(SkillListQuery {
                        turn_id: turn_id.to_string(),
                        executor_roots: Vec::new(),
                        host_snapshot: self.host_snapshot.clone(),
                        include_host_skills: true,
                        include_bundled_skills: false,
                        include_orchestrator_skills: false,
                        mode,
                        mcp_resources: self.mcp_resources.clone(),
                    })
                    .await
            }
            SkillToolAuthority::Executor { root_id } => {
                let executor_roots: Vec<_> = self
                    .thread_state
                    .selected_roots()
                    .iter()
                    .filter(|root| root.id == root_id)
                    .cloned()
                    .collect();
                self.providers
                    .list_for_turn(SkillListQuery {
                        turn_id: turn_id.to_string(),
                        executor_roots,
                        host_snapshot: None,
                        include_host_skills: false,
                        include_bundled_skills: false,
                        include_orchestrator_skills: false,
                        mode,
                        mcp_resources: self.mcp_resources.clone(),
                    })
                    .await
            }
            SkillToolAuthority::Orchestrator => {
                self.thread_state
                    .orchestrator_catalog_snapshot(
                        self.mcp_resources.as_deref(),
                        self.providers.list_orchestrator_for_turn(SkillListQuery {
                            turn_id: turn_id.to_string(),
                            executor_roots: Vec::new(),
                            host_snapshot: None,
                            include_host_skills: false,
                            include_bundled_skills: false,
                            include_orchestrator_skills: true,
                            mode,
                            mcp_resources: self.mcp_resources.clone(),
                        }),
                    )
                    .await
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SkillToolAuthority {
    Host,
    Executor { root_id: String },
    Orchestrator,
}

impl SkillToolAuthority {
    fn from_authority(authority: &SkillAuthority) -> Option<Self> {
        match &authority.kind {
            SkillSourceKind::Host if authority.id == "host" => Some(Self::Host),
            SkillSourceKind::Executor => Some(Self::Executor {
                root_id: authority.id.clone(),
            }),
            SkillSourceKind::Orchestrator if authority.id == ODY_APPS_MCP_SERVER_NAME => {
                Some(Self::Orchestrator)
            }
            _ => None,
        }
    }

    fn into_authority(self) -> SkillAuthority {
        match self {
            Self::Host => SkillAuthority::new(SkillSourceKind::Host, "host"),
            Self::Executor { root_id } => SkillAuthority::new(SkillSourceKind::Executor, root_id),
            Self::Orchestrator => {
                SkillAuthority::new(SkillSourceKind::Orchestrator, ODY_APPS_MCP_SERVER_NAME)
            }
        }
    }
}

fn skill_tool_name(name: &str) -> ToolName {
    ToolName::namespaced(SKILLS_NAMESPACE, name)
}

fn skill_function_tool<I: JsonSchema, O: JsonSchema>(name: &str, description: &str) -> ToolSpec {
    let tool = ResponsesApiTool {
        name: name.to_string(),
        description: description.to_string(),
        strict: false,
        defer_loading: None,
        parameters: parse_tool_input_schema(&schema::input_schema_for::<I>())
            .unwrap_or_else(|err| panic!("generated input schema for {name} should parse: {err}")),
        output_schema: Some(schema::output_schema_for::<O>()),
    };

    ToolSpec::Namespace(ResponsesApiNamespace {
        name: SKILLS_NAMESPACE.to_string(),
        description: default_namespace_description(SKILLS_NAMESPACE),
        tools: vec![ResponsesApiNamespaceTool::Function(tool)],
    })
}

fn parse_args<T: for<'de> Deserialize<'de>>(call: &ToolCall) -> Result<T, FunctionCallError> {
    let arguments = call.function_arguments()?;
    let value = if arguments.trim().is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(arguments)
            .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?
    };
    serde_json::from_value(value).map_err(|err| FunctionCallError::RespondToModel(err.to_string()))
}

fn validate_handle(name: &str, value: &str, max_bytes: usize) -> Result<(), FunctionCallError> {
    if is_bounded_handle(value, max_bytes) {
        return Ok(());
    }

    Err(FunctionCallError::RespondToModel(format!(
        "{name} must be non-empty, contain no control characters, and be at most {max_bytes} bytes"
    )))
}

fn is_bounded_handle(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn external_json_output<T: Serialize>(value: &T) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
    let value = serde_json::to_value(value).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize tool output: {err}"))
    })?;
    Ok(Box::new(JsonToolOutput::new(value).with_external_context()))
}
