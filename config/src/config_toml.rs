//! Schema-heavy configuration TOML types used by Ody.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;

use crate::HooksToml;
use crate::permissions_toml::PermissionsToml;
use crate::profile_toml::ConfigProfile;
use crate::types::AnalyticsConfigToml;
use crate::types::ApprovalsReviewer;
use crate::types::AppsConfigToml;
use crate::types::AuthCredentialsStoreMode;
use crate::types::FeedbackConfigToml;
use crate::types::History;
use crate::types::MarketplaceConfig;
use crate::types::McpServerConfig;
use crate::types::MemoriesToml;
use crate::types::Notice;
use crate::types::OAuthCredentialsStoreMode;
use crate::types::OtelConfigToml;
use crate::types::PluginConfig;
use crate::types::SandboxWorkspaceWrite;
use crate::types::ShellEnvironmentPolicyToml;
use crate::types::SkillsConfig;
use crate::types::ToolSuggestConfig;
use crate::types::Tui;
use crate::types::UriBasedFileOpener;
use crate::types::WindowsToml;
use ody_features::FeaturesToml;
use ody_model_provider_info::DEEPSEEK_PROVIDER_ID;
use ody_model_provider_info::GLM_PROVIDER_ID;
use ody_model_provider_info::KIMI_PROVIDER_ID;
use ody_model_provider_info::ModelProviderInfo;
use ody_model_provider_info::ProviderCapabilities;
use ody_protocol::config_types::AutoCompactTokenLimitScope;
use ody_protocol::config_types::ForcedLoginMethod;
use ody_protocol::config_types::Personality;
use ody_protocol::config_types::ReasoningSummary;
use ody_protocol::config_types::SandboxMode;
use ody_protocol::config_types::TrustLevel;
use ody_protocol::config_types::Verbosity;
use ody_protocol::config_types::WebSearchMode;
use ody_protocol::config_types::WebSearchToolConfig;
use ody_protocol::config_types::WindowsSandboxLevel;
use ody_protocol::models::PermissionProfile;
use ody_protocol::model_metadata::ReasoningEffort;
use ody_protocol::permissions::NetworkSandboxPolicy;
use ody_protocol::protocol::AskForApproval;
use ody_utils_absolute_path::AbsolutePathBuf;
use ody_utils_path::normalize_for_path_comparison;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;

use serde_json::Value as JsonValue;

const RESERVED_MODEL_PROVIDER_IDS: [&str; 4] = [
    KIMI_PROVIDER_ID,
    KIMI_PROVIDER_ID,
    DEEPSEEK_PROVIDER_ID,
    GLM_PROVIDER_ID,
];

pub const DEFAULT_PROJECT_DOC_MAX_BYTES: usize = 32 * 1024;

const fn default_allow_login_shell() -> Option<bool> {
    Some(true)
}

fn default_history() -> Option<History> {
    Some(History::default())
}

const fn default_project_doc_max_bytes() -> Option<usize> {
    Some(DEFAULT_PROJECT_DOC_MAX_BYTES)
}

fn default_project_doc_fallback_filenames() -> Option<Vec<String>> {
    Some(Vec::new())
}

const fn default_hide_agent_reasoning() -> Option<bool> {
    Some(false)
}

const fn default_true() -> bool {
    true
}

/// Orchestrator-owned feature settings.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct OrchestratorToml {
    pub skills: Option<OrchestratorFeatureToml>,
    pub mcp: Option<OrchestratorFeatureToml>,
}

/// Settings for a feature owned by the orchestrator.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct OrchestratorFeatureToml {
    pub enabled: Option<bool>,
}

/// Enforcement level for Plan mode write protections.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(deny_unknown_fields)]
pub enum PlanEnforcement {
    /// Truly read-only planning: writes are denied with feedback to the model.
    #[default]
    Strict,
    /// Every write operation is forced through user approval.
    Ask,
    /// Equivalent to the legacy prompt-only Plan behavior.
    Advisory,
}

/// Whether Plan mode conversations are isolated from the main thread context.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(deny_unknown_fields)]
pub enum PlanContextIsolation {
    /// Plan conversations stay in the main thread (legacy behavior).
    #[default]
    Off,
    /// Plan conversations are routed to a separate partition.
    On,
}


/// Plan-mode contract tier: concise conversational contract vs. full rigor contract.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(deny_unknown_fields)]
pub enum PlanModeTier {
    /// Let the heuristic scorer decide.
    #[default]
    Auto,
    /// Always use the concise conversational contract.
    Concise,
    /// Always use the full rigor contract with reminders.
    Rigor,
}

const fn default_plan_enforcement() -> Option<PlanEnforcement> {
    Some(PlanEnforcement::Strict)
}

const fn default_persist_plan_file() -> Option<bool> {
    Some(true)
}

const fn default_plan_context_isolation() -> Option<PlanContextIsolation> {
    Some(PlanContextIsolation::Off)
}

const fn default_split_threshold() -> Option<usize> {
    Some(8)
}

const fn default_split_plan_compaction_ratio() -> Option<f64> {
    Some(0.5)
}

const fn default_full_refresh_turns() -> Option<usize> {
    Some(5)
}

const fn default_dedup_min_turns() -> Option<usize> {
    Some(2)
}

fn default_plan_mode_config() -> Option<PlanModeConfigToml> {
    Some(PlanModeConfigToml::default())
}

/// Settings scoped to Plan mode.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct PlanModeConfigToml {
    /// Enforcement level for Plan mode write protections.
    #[serde(default = "default_plan_enforcement")]
    pub enforcement: Option<PlanEnforcement>,
    /// Whether to persist the plan to a file on disk.
    #[serde(default = "default_persist_plan_file")]
    pub persist_plan_file: Option<bool>,
    /// Whether to isolate Plan mode conversations from the main thread context.
    #[serde(default = "default_plan_context_isolation")]
    pub context_isolation: Option<PlanContextIsolation>,
    /// Optional model alias to use exclusively in Plan mode.
    pub model: Option<String>,
    /// Reasoning effort override used by the Plan preset.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Number of tasks that triggers splitting a large plan into multiple files.
    /// 0 disables splitting.
    #[serde(default = "default_split_threshold")]
    pub split_threshold: Option<usize>,
    /// Ratio of max context usage above which a synchronous compaction is
    /// triggered when a plan part boundary is crossed. 0.0 disables.
    #[serde(default = "default_split_plan_compaction_ratio")]
    pub split_plan_compaction_ratio: Option<f64>,
    /// Number of plan-mode turns between full rigor-contract reinjections.
    /// 0 disables full reinjection.
    #[serde(default = "default_full_refresh_turns")]
    pub full_refresh_turns: Option<usize>,
    /// Minimum number of turns between any two rigor reminders (full or sparse).
    /// 0 disables sparse reminders.
    #[serde(default = "default_dedup_min_turns")]
    pub dedup_min_turns: Option<usize>,
    /// Explicit plan-mode tier override.  means use the heuristic scorer (Auto).
    #[serde(default)]
    pub tier: Option<PlanModeTier>,
}

impl Default for PlanModeConfigToml {
    fn default() -> Self {
        Self {
            enforcement: default_plan_enforcement(),
            persist_plan_file: default_persist_plan_file(),
            context_isolation: default_plan_context_isolation(),
            model: None,
            reasoning_effort: None,
            split_threshold: default_split_threshold(),
            split_plan_compaction_ratio: default_split_plan_compaction_ratio(),
            full_refresh_turns: default_full_refresh_turns(),
            dedup_min_turns: default_dedup_min_turns(),
            tier: None,
        }
    }
}

/// Base config deserialized from ~/.ody-code/config.toml.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ConfigToml {
    /// Optional override of model selection.
    pub model: Option<String>,
    /// Review model override used by the `/review` feature.
    pub review_model: Option<String>,

    /// Provider to use from the model_providers map.
    pub model_provider: Option<String>,

    /// Size of the context window for the model, in tokens.
    pub model_context_window: Option<i64>,

    /// Token usage threshold triggering auto-compaction of conversation history.
    pub model_auto_compact_token_limit: Option<i64>,

    /// Controls whether the auto-compaction limit applies to the full context or
    /// only to tokens after the carried prefix in the current compaction window.
    pub model_auto_compact_token_limit_scope: Option<AutoCompactTokenLimitScope>,

    /// Default approval policy for executing commands.
    pub approval_policy: Option<AskForApproval>,

    /// Configures who approval requests are routed to for review once they have
    /// been escalated. This does not disable separate safety checks such as
    /// ARC.
    pub approvals_reviewer: Option<ApprovalsReviewer>,

    /// Optional policy instructions for the guardian auto-reviewer.
    #[serde(default)]
    pub auto_review: Option<AutoReviewToml>,

    #[serde(default)]
    pub shell_environment_policy: ShellEnvironmentPolicyToml,

    /// Whether the model may request a login shell for shell-based tools.
    /// Default to `true`
    ///
    /// If `true`, the model may request a login shell (`login = true`), and
    /// omitting `login` defaults to using a login shell.
    /// If `false`, the model can never use a login shell: `login = true`
    /// requests are rejected, and omitting `login` defaults to a non-login
    /// shell.
    #[serde(default = "default_allow_login_shell")]
    pub allow_login_shell: Option<bool>,

    /// Sandbox mode to use.
    pub sandbox_mode: Option<SandboxMode>,

    /// Sandbox configuration to apply if `sandbox` is `WorkspaceWrite`.
    pub sandbox_workspace_write: Option<SandboxWorkspaceWrite>,

    /// Default permissions profile to apply. Names starting with `:` refer to
    /// built-in profiles; other names are resolved from the `[permissions]`
    /// table.
    pub default_permissions: Option<String>,

    /// Named permissions profiles.
    #[serde(default)]
    pub permissions: Option<PermissionsToml>,

    /// Optional external command to spawn for end-user notifications.
    #[serde(default)]
    pub notify: Option<Vec<String>>,

    /// System instructions.
    pub instructions: Option<String>,

    /// Preferred language for model responses in the TUI transcript.
    /// When set, a short instruction is appended to the base instructions
    /// asking the model to respond in this language.
    pub language: Option<String>,

    /// Developer instructions inserted as a `developer` role message.
    #[serde(default)]
    pub developer_instructions: Option<String>,

    /// Whether to inject the `<permissions instructions>` developer block.
    pub include_permissions_instructions: Option<bool>,

    /// Whether to inject the `<apps_instructions>` developer block.
    pub include_apps_instructions: Option<bool>,

    /// Whether to inject the `<collaboration_mode>` developer block.
    pub include_collaboration_mode_instructions: Option<bool>,

    /// Whether to inject the `<environment_context>` user block.
    pub include_environment_context: Option<bool>,

    /// Optional path to a file containing model instructions that will override
    /// the built-in instructions for the selected model. Users are STRONGLY
    /// DISCOURAGED from using this field, as deviating from the instructions
    /// sanctioned by Ody will likely degrade model performance.
    pub model_instructions_file: Option<AbsolutePathBuf>,

    /// Compact prompt used for history compaction.
    pub compact_prompt: Option<String>,

    /// When set, restricts the login mechanism users may use.
    #[serde(default)]
    pub forced_login_method: Option<ForcedLoginMethod>,

    /// Preferred backend for storing CLI auth credentials.
    /// file (default): Use a file in the Ody home directory.
    /// keyring: Use an OS-specific keyring service.
    /// auto: Use the keyring if available, otherwise use a file.
    #[serde(default)]
    pub cli_auth_credentials_store: Option<AuthCredentialsStoreMode>,

    /// Definition for MCP servers that Ody can reach out to for tool calls.
    #[serde(default)]
    // Uses the raw MCP input shape (custom deserialization) rather than `McpServerConfig`.
    #[schemars(schema_with = "crate::schema::mcp_servers_schema")]
    pub mcp_servers: HashMap<String, McpServerConfig>,

    /// Preferred backend for storing MCP OAuth credentials.
    /// keyring: Use an OS-specific keyring service.
    ///          https://github.com/odysseythink/ody/blob/main/ody-rs/rmcp-client/src/oauth.rs#L2
    /// file: Use a file in the Ody home directory.
    /// auto (default): Use the OS-specific keyring service if available, otherwise use a file.
    #[serde(default)]
    pub mcp_oauth_credentials_store: Option<OAuthCredentialsStoreMode>,

    /// Optional fixed port for the local HTTP callback server used during MCP OAuth login.
    /// When unset, Ody will bind to an ephemeral port chosen by the OS.
    pub mcp_oauth_callback_port: Option<u16>,

    /// Optional redirect URI to use during MCP OAuth login.
    /// When set, this URI is used in the OAuth authorization request instead
    /// of the local listener address. The local callback listener still binds
    /// to 127.0.0.1 (using `mcp_oauth_callback_port` when provided).
    pub mcp_oauth_callback_url: Option<String>,

    /// User-defined provider entries that extend the built-in list. Built-in
    /// IDs cannot be overridden.
    #[serde(default, deserialize_with = "deserialize_model_providers")]
    pub model_providers: HashMap<String, ModelProviderInfo>,

    /// Ody-code compatible provider entries keyed by provider id.
    /// These are translated into `model_providers` on load so that ody-code
    /// shaped configs (e.g. `[providers.kimi_gyy]`) work without rewriting.
    #[serde(default)]
    pub providers: HashMap<String, OdyCodeProviderConfig>,

    /// Ody-code compatible default provider id.
    #[serde(default)]
    pub default_provider: Option<String>,

    /// Ody-code compatible default model in the form `provider_id/model_name`.
    #[serde(default)]
    pub default_model: Option<String>,

    /// Ody-code compatible default thinking flag.
    #[serde(default)]
    pub default_thinking: Option<bool>,

    /// Maximum number of bytes to include from an AGENTS.md project doc file.
    #[serde(default = "default_project_doc_max_bytes")]
    pub project_doc_max_bytes: Option<usize>,

    /// Ordered list of fallback filenames to look for when AGENTS.md is missing.
    #[serde(default = "default_project_doc_fallback_filenames")]
    pub project_doc_fallback_filenames: Option<Vec<String>>,

    /// Token budget applied when storing tool/function outputs in the context manager.
    pub tool_output_token_limit: Option<usize>,

    /// Maximum poll window for background terminal output (`write_stdin`), in milliseconds.
    /// Default: `300000` (5 minutes).
    pub background_terminal_max_timeout: Option<u64>,

    /// Deprecated: ignored.
    #[schemars(skip)]
    pub js_repl_node_path: Option<AbsolutePathBuf>,

    /// Deprecated: ignored.
    #[schemars(skip)]
    pub js_repl_node_module_dirs: Option<Vec<AbsolutePathBuf>>,

    /// Profile to use from the `profiles` map.
    pub profile: Option<String>,

    /// Named profiles to facilitate switching between different configurations.
    #[serde(default)]
    pub profiles: HashMap<String, ConfigProfile>,

    /// Settings that govern if and what will be written to `~/.ody-code/history.jsonl`.
    #[serde(default = "default_history")]
    pub history: Option<History>,

    /// Directory where Ody stores the SQLite state DB.
    /// Defaults to `$ODY_SQLITE_HOME` when set. Otherwise uses `$ODY_HOME`.
    pub sqlite_home: Option<AbsolutePathBuf>,

    /// Directory where Ody writes log files. Setting this value explicitly
    /// also enables the TUI text log in this directory.
    /// Defaults to `$ODY_HOME/log`.
    pub log_dir: Option<AbsolutePathBuf>,

    /// Debugging and reproducibility settings.
    pub debug: Option<DebugToml>,

    /// Optional URI-based file opener. If set, citations to files in the model
    /// output will be hyperlinked using the specified URI scheme.
    pub file_opener: Option<UriBasedFileOpener>,

    /// Collection of settings that are specific to the TUI.
    pub tui: Option<Tui>,

    /// When set to `true`, `AgentReasoning` events will be hidden from the
    /// UI/output. Defaults to `false`.
    #[serde(default = "default_hide_agent_reasoning")]
    pub hide_agent_reasoning: Option<bool>,

    /// When set to `true`, `AgentReasoningRawContentEvent` events will be shown in the UI/output.
    /// Defaults to `false`.
    pub show_raw_agent_reasoning: Option<bool>,

    pub model_reasoning_effort: Option<ReasoningEffort>,
    /// Deprecated: use `plan_mode.reasoning_effort` instead.
    pub plan_mode_reasoning_effort: Option<ReasoningEffort>,
    /// Plan mode settings.
    #[serde(default = "default_plan_mode_config")]
    pub plan_mode: Option<PlanModeConfigToml>,
    pub model_reasoning_summary: Option<ReasoningSummary>,
    /// Optional verbosity control for GPT-5 models (Responses API `text.verbosity`).
    pub model_verbosity: Option<Verbosity>,

    /// Override to force-enable reasoning summaries for the configured model.
    pub model_supports_reasoning_summaries: Option<bool>,

    /// Optional path to a JSON model catalog (applied on startup only).
    /// Per-thread `config` overrides are accepted but do not reapply this (no-ops).
    pub model_catalog_json: Option<AbsolutePathBuf>,

    /// Optionally specify a personality for the model
    pub personality: Option<Personality>,

    /// Optional explicit service tier request id for new turns (for example
    /// `default`, `priority`, or `flex`; legacy `fast` also works).
    pub service_tier: Option<String>,

    /// Optional product SKU forwarded on host-owned Ody Apps MCP requests.
    pub apps_mcp_product_sku: Option<String>,

    /// Orchestrator-owned feature settings.
    pub orchestrator: Option<OrchestratorToml>,

    /// Base URL override for the built-in `odysseythink` model provider.
    pub odysseythink_base_url: Option<String>,

    /// Machine-local realtime audio device preferences used by realtime voice.
    #[serde(default)]
    pub audio: Option<RealtimeAudioToml>,

    /// Experimental / do not use. Overrides only the realtime conversation
    /// websocket transport base URL (the `Op::RealtimeConversation`
    /// `/v1/realtime`
    /// connection) without changing normal provider HTTP requests.
    pub experimental_realtime_ws_base_url: Option<String>,
    /// Experimental / do not use. Overrides only the WebRTC realtime call
    /// creation base URL. This is separate from `experimental_realtime_ws_base_url`
    /// because WebRTC call creation is HTTP, while sideband control is websocket.
    pub experimental_realtime_webrtc_call_base_url: Option<String>,
    /// Experimental / do not use. Selects the realtime websocket model/snapshot
    /// used for the `Op::RealtimeConversation` connection.
    pub experimental_realtime_ws_model: Option<String>,
    /// Experimental / do not use. Realtime websocket session selection.
    /// `version` controls v1/v2 and `type` controls conversational/transcription.
    #[serde(default)]
    pub realtime: Option<RealtimeToml>,
    /// Experimental / do not use. Overrides only the realtime conversation
    /// websocket transport instructions (the `Op::RealtimeConversation`
    /// `/ws` session.update instructions) without changing normal prompts.
    pub experimental_realtime_ws_backend_prompt: Option<String>,
    /// Experimental / do not use. Replaces the synthesized realtime startup
    /// context appended to websocket session instructions. An empty string
    /// disables startup context injection entirely.
    pub experimental_realtime_ws_startup_context: Option<String>,
    /// Experimental / do not use. Replaces the built-in realtime start
    /// instructions inserted into developer messages when realtime becomes
    /// active.
    pub experimental_realtime_start_instructions: Option<String>,

    /// Experimental / do not use. When set, app-server fetches thread-scoped
    /// config from a remote service at this endpoint.
    pub experimental_thread_config_endpoint: Option<String>,

    /// Removed. Former remote thread-store endpoint setting kept only so we can
    /// fail fast instead of silently falling back to local persistence.
    #[schemars(skip)]
    pub experimental_thread_store_endpoint: Option<String>,

    /// Experimental / do not use. Selects the thread store implementation.
    pub experimental_thread_store: Option<ThreadStoreToml>,
    pub projects: Option<HashMap<String, ProjectConfig>>,

    /// Controls the web search tool mode: disabled, cached, indexed, or live.
    pub web_search: Option<WebSearchMode>,

    /// Nested tools section for feature toggles
    pub tools: Option<ToolsToml>,

    /// Additional discoverable tools that can be suggested for installation.
    pub tool_suggest: Option<ToolSuggestConfig>,

    /// Agent-related settings (thread limits, etc.).
    pub agents: Option<AgentsToml>,

    /// Memories subsystem settings.
    pub memories: Option<MemoriesToml>,

    /// User-level skill config entries keyed by SKILL.md path.
    pub skills: Option<SkillsConfig>,

    /// Lifecycle hooks configured inline in TOML plus user-level overrides.
    pub hooks: Option<HooksToml>,

    /// User-level plugin config entries keyed by plugin name.
    #[serde(default)]
    pub plugins: HashMap<String, PluginConfig>,

    /// User-level marketplace entries keyed by marketplace name.
    #[serde(default)]
    pub marketplaces: HashMap<String, MarketplaceConfig>,

    /// Centralized feature flags (new). Prefer this over individual toggles.
    #[serde(default)]
    // Injects known feature keys into the schema and forbids unknown keys.
    #[schemars(schema_with = "crate::schema::features_schema")]
    pub features: Option<FeaturesToml>,

    /// Suppress warnings about unstable (under development) features.
    pub suppress_unstable_features_warning: Option<bool>,

    /// Compatibility-only settings retained so legacy `ghost_snapshot`
    /// config still loads.
    #[serde(default)]
    pub ghost_snapshot: Option<GhostSnapshotToml>,

    /// Markers used to detect the project root when searching parent
    /// directories for `.ody` folders. Defaults to [".git"] when unset.
    #[serde(default)]
    pub project_root_markers: Option<Vec<String>>,

    /// When `true`, checks for Ody updates on startup and surfaces update prompts.
    /// Set to `false` only if your Ody updates are centrally managed.
    /// Defaults to `true`.
    pub check_for_update_on_startup: Option<bool>,

    /// When true, disables burst-paste detection for typed input entirely.
    /// All characters are inserted as they are received, and no buffering
    /// or placeholder replacement will occur for fast keypress bursts.
    pub disable_paste_burst: Option<bool>,

    /// When `false`, disables analytics across Ody product surfaces in this machine.
    /// Defaults to `true`.
    pub analytics: Option<AnalyticsConfigToml>,

    /// When `false`, disables feedback collection across Ody product surfaces.
    /// Defaults to `true`.
    pub feedback: Option<FeedbackConfigToml>,

    /// Settings for app-specific controls.
    #[serde(default)]
    pub apps: Option<AppsConfigToml>,

    /// Opaque desktop settings stored alongside the rest of config.toml.
    #[serde(default)]
    pub desktop: Option<HashMap<String, JsonValue>>,

    /// OTEL configuration.
    pub otel: Option<OtelConfigToml>,

    /// Windows-specific configuration.
    #[serde(default)]
    pub windows: Option<WindowsToml>,

    /// Collection of in-product notices (different from notifications)
    /// See [`crate::types::Notice`] for more details
    pub notice: Option<Notice>,

    pub experimental_compact_prompt_file: Option<AbsolutePathBuf>,
    pub experimental_use_unified_exec_tool: Option<bool>,
}

/// Ody-code compatible provider configuration.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct OdyCodeProviderConfig {
    pub r#type: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub oauth: Option<OdyCodeOAuthRef>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub custom_headers: HashMap<String, String>,
}

/// Ody-code compatible OAuth reference.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct OdyCodeOAuthRef {
    pub storage: String,
    pub key: String,
}

impl ConfigToml {
    /// Convert ody-code compatible `providers` into ody-rs `ModelProviderInfo` entries.
    pub fn convert_ody_code_providers(&self) -> HashMap<String, ModelProviderInfo> {
        use ody_model_provider_info::WireApi;

        self.providers
            .iter()
            .map(|(id, provider)| {
                let display_name = provider_display_name(&provider.r#type);
                let wire_api = match provider.r#type.as_str() {
                    "openai" | "openai_responses" => WireApi::Responses,
                    "anthropic" => WireApi::AnthropicMessages,
                    "google-genai" | "vertexai" => WireApi::GoogleGenAI,
                    "kimi" | "deepseek" | "glm" => WireApi::Chat,
                    _ => WireApi::Chat,
                };
                let mut info = ModelProviderInfo {
                    name: display_name,
                    base_url: provider.base_url.clone(),
                    env_key: None,
                    env_key_instructions: None,
                    experimental_bearer_token: provider.api_key.clone(),
                    auth: None,
                    wire_api,
                    query_params: None,
                    http_headers: Some(provider.custom_headers.clone()),
                    env_http_headers: None,
                    request_max_retries: None,
                    stream_max_retries: None,
                    stream_idle_timeout_ms: None,
                    websocket_connect_timeout_ms: None,
                    supports_websockets: false,
                    capabilities: ProviderCapabilities::default(),
                };
                if info.http_headers.as_ref().map_or(true, |h| h.is_empty()) {
                    info.http_headers = None;
                }
                info.normalize_capabilities();
                (id.clone(), info)
            })
            .collect()
    }

    /// Resolve the default model declared in ody-code format.
    ///
    /// Returns `(provider_id, model_name)` when `default_model` is present.
    pub fn resolve_ody_code_default_model(&self) -> (Option<String>, Option<String>) {
        match &self.default_model {
            Some(default_model) => match default_model.split_once('/') {
                Some((provider, model)) => {
                    (Some(provider.to_string()), Some(model.to_string()))
                }
                None => (self.default_provider.clone(), Some(default_model.clone())),
            },
            None => (self.default_provider.clone(), None),
        }
    }
}

fn provider_display_name(provider_type: &str) -> String {
    match provider_type {
        "kimi" => "Kimi".to_string(),
        "deepseek" => "DeepSeek".to_string(),
        "glm" => "GLM".to_string(),
        "openai" | "openai_responses" => "OpenAI".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ConfigLockfileToml {
    pub version: u32,
    pub ody_version: String,

    /// Replayable effective config captured in the lockfile.
    pub config: ConfigToml,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct DebugToml {
    pub config_lockfile: Option<DebugConfigLockToml>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct DebugConfigLockToml {
    /// Directory where Ody writes effective session config lock files.
    pub export_dir: Option<AbsolutePathBuf>,

    /// Lockfile to replay as the authoritative effective config.
    pub load_path: Option<AbsolutePathBuf>,

    /// Allow replaying a lock generated by a different Ody version.
    pub allow_ody_version_mismatch: Option<bool>,

    /// Save fields resolved from the model catalog/session configuration.
    pub save_fields_resolved_from_model_catalog: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThreadStoreToml {
    Local {},
    #[schemars(skip)]
    InMemory {
        id: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
pub struct AutoReviewToml {
    /// Additional policy instructions inserted into the guardian prompt.
    pub policy: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ProjectConfig {
    pub trust_level: Option<TrustLevel>,
}

impl ProjectConfig {
    pub fn is_trusted(&self) -> bool {
        matches!(self.trust_level, Some(TrustLevel::Trusted))
    }

    pub fn is_untrusted(&self) -> bool {
        matches!(self.trust_level, Some(TrustLevel::Untrusted))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RealtimeAudioConfig {
    pub microphone: Option<String>,
    pub speaker: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeWsMode {
    #[default]
    Conversational,
    Transcription,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeTransport {
    #[default]
    #[serde(rename = "webrtc")]
    WebRtc,
    Websocket,
}

pub use ody_protocol::protocol::RealtimeConversationVersion as RealtimeWsVersion;
pub use ody_protocol::protocol::RealtimeVoice;

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RealtimeConfig {
    pub version: RealtimeWsVersion,
    #[serde(rename = "type")]
    pub session_type: RealtimeWsMode,
    pub transport: RealtimeTransport,
    pub voice: Option<RealtimeVoice>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RealtimeToml {
    pub version: Option<RealtimeWsVersion>,
    #[serde(rename = "type")]
    pub session_type: Option<RealtimeWsMode>,
    pub transport: Option<RealtimeTransport>,
    pub voice: Option<RealtimeVoice>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RealtimeAudioToml {
    pub microphone: Option<String>,
    pub speaker: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ToolsToml {
    #[serde(
        default,
        deserialize_with = "deserialize_optional_web_search_tool_config"
    )]
    pub web_search: Option<WebSearchToolConfig>,
    pub experimental_request_user_input: Option<ExperimentalRequestUserInput>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ExperimentalRequestUserInput {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WebSearchToolConfigInput {
    Enabled(bool),
    Config(WebSearchToolConfig),
}

fn deserialize_optional_web_search_tool_config<'de, D>(
    deserializer: D,
) -> Result<Option<WebSearchToolConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<WebSearchToolConfigInput>::deserialize(deserializer)?;

    Ok(match value {
        None => None,
        Some(WebSearchToolConfigInput::Enabled(enabled)) => {
            let _ = enabled;
            None
        }
        Some(WebSearchToolConfigInput::Config(config)) => Some(config),
    })
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct AgentsToml {
    /// Maximum number of agent threads that can be open concurrently.
    /// When unset, no limit is enforced.
    #[schemars(range(min = 1))]
    pub max_threads: Option<usize>,
    /// Maximum nesting depth allowed for spawned agent threads.
    /// Root sessions start at depth 0.
    #[schemars(range(min = 1))]
    pub max_depth: Option<i32>,
    /// Default maximum runtime in seconds for agent job workers.
    #[schemars(range(min = 1))]
    pub job_max_runtime_seconds: Option<u64>,
    /// Whether to record a model-visible message when an agent turn is interrupted.
    /// Defaults to true.
    pub interrupt_message: Option<bool>,

    /// User-defined role declarations keyed by role name.
    ///
    /// Example:
    /// ```toml
    /// [agents.researcher]
    /// description = "Research-focused role."
    /// config_file = "./agents/researcher.toml"
    /// nickname_candidates = ["Herodotus", "Ibn Battuta"]
    /// ```
    #[serde(default, flatten)]
    pub roles: BTreeMap<String, AgentRoleToml>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct AgentRoleToml {
    /// Human-facing role documentation used in spawn tool guidance.
    /// Required unless supplied by the referenced agent role file.
    pub description: Option<String>,

    /// Path to a role-specific config layer.
    /// Relative paths are resolved relative to the `config.toml` that defines them.
    pub config_file: Option<AbsolutePathBuf>,

    /// Candidate nicknames for agents spawned with this role.
    pub nickname_candidates: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct GhostSnapshotToml {
    /// Legacy no-op setting retained for compatibility.
    #[serde(alias = "ignore_untracked_files_over_bytes")]
    pub ignore_large_untracked_files: Option<i64>,
    /// Legacy no-op setting retained for compatibility.
    #[serde(alias = "large_untracked_dir_warning_threshold")]
    pub ignore_large_untracked_dirs: Option<i64>,
    /// Legacy no-op setting retained for compatibility.
    pub disable_warnings: Option<bool>,
}

impl ConfigToml {
    /// Derive the effective permission profile from legacy sandbox config.
    ///
    /// Call this only after ruling out `default_permissions`: named
    /// `[permissions]` profiles must be compiled through the permissions
    /// profile pipeline, not reconstructed from `sandbox_mode`.
    pub async fn derive_permission_profile(
        &self,
        sandbox_mode_override: Option<SandboxMode>,
        windows_sandbox_level: WindowsSandboxLevel,
        active_project: Option<&ProjectConfig>,
        permission_profile_constraint: Option<&crate::Constrained<PermissionProfile>>,
    ) -> PermissionProfile {
        let configured_sandbox_mode = sandbox_mode_override.or(self.sandbox_mode);
        let resolved_sandbox_mode = configured_sandbox_mode
            .or_else(|| {
                // If no sandbox_mode is set but this directory has a trust decision,
                // default to workspace-write except on unsandboxed Windows where we
                // default to read-only.
                active_project
                    .filter(|project| project.is_trusted() || project.is_untrusted())
                    .map(|_| {
                        if cfg!(target_os = "windows")
                            && windows_sandbox_level == WindowsSandboxLevel::Disabled
                        {
                            SandboxMode::ReadOnly
                        } else {
                            SandboxMode::WorkspaceWrite
                        }
                    })
            })
            .unwrap_or_default();
        let effective_sandbox_mode = if cfg!(target_os = "windows")
            // If the experimental Windows sandbox is enabled, do not force a downgrade.
            && windows_sandbox_level == WindowsSandboxLevel::Disabled
            && matches!(resolved_sandbox_mode, SandboxMode::WorkspaceWrite)
        {
            SandboxMode::ReadOnly
        } else {
            resolved_sandbox_mode
        };

        let permission_profile = match effective_sandbox_mode {
            SandboxMode::ReadOnly => PermissionProfile::read_only(),
            SandboxMode::WorkspaceWrite => match self.sandbox_workspace_write.as_ref() {
                Some(SandboxWorkspaceWrite {
                    writable_roots,
                    network_access,
                    exclude_tmpdir_env_var,
                    exclude_slash_tmp,
                }) => {
                    let network_policy = if *network_access {
                        NetworkSandboxPolicy::Enabled
                    } else {
                        NetworkSandboxPolicy::Restricted
                    };
                    PermissionProfile::workspace_write_with(
                        writable_roots,
                        network_policy,
                        *exclude_tmpdir_env_var,
                        *exclude_slash_tmp,
                    )
                }
                None => PermissionProfile::workspace_write(),
            },
            SandboxMode::DangerFullAccess => PermissionProfile::Disabled,
        };
        if configured_sandbox_mode.is_none()
            && let Some(constraint) = permission_profile_constraint
            && let Err(err) = constraint.can_set(&permission_profile)
        {
            tracing::warn!(
                error = %err,
                "default sandbox policy is disallowed by requirements; falling back to required default"
            );
            PermissionProfile::read_only()
        } else {
            permission_profile
        }
    }

    /// Resolves the cwd to an existing project, or returns None if ConfigToml
    /// does not contain a project corresponding to cwd or the resolved git repo
    /// root for cwd.
    pub fn get_active_project(
        &self,
        resolved_cwd: &Path,
        repo_root: Option<&Path>,
    ) -> Option<ProjectConfig> {
        let projects = self.projects.as_ref()?;

        for normalized_cwd in normalized_project_lookup_keys(resolved_cwd) {
            if let Some(project_config) = project_config_for_lookup_key(projects, &normalized_cwd) {
                return Some(project_config);
            }
        }

        if let Some(repo_root) = repo_root {
            for normalized_repo_root in normalized_project_lookup_keys(repo_root) {
                if let Some(project_config_for_root) =
                    project_config_for_lookup_key(projects, &normalized_repo_root)
                {
                    return Some(project_config_for_root);
                }
            }
        }

        None
    }
}

/// Canonicalize the path and convert it to a string to be used as a key in the
/// projects trust map. On Windows, strips UNC, when possible, to try to ensure
/// that different paths that point to the same location have the same key.
fn normalized_project_lookup_keys(path: &Path) -> Vec<String> {
    let normalized_path = normalize_project_lookup_key(path.to_string_lossy().to_string());
    let normalized_canonical_path = normalize_project_lookup_key(
        normalize_for_path_comparison(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .to_string(),
    );
    if normalized_path == normalized_canonical_path {
        vec![normalized_canonical_path]
    } else {
        vec![normalized_canonical_path, normalized_path]
    }
}

fn normalize_project_lookup_key(key: String) -> String {
    if cfg!(windows) {
        key.to_ascii_lowercase()
    } else {
        key
    }
}

fn project_config_for_lookup_key(
    projects: &HashMap<String, ProjectConfig>,
    lookup_key: &str,
) -> Option<ProjectConfig> {
    if let Some(project_config) = projects.get(lookup_key) {
        return Some(project_config.clone());
    }

    let mut normalized_matches: Vec<_> = projects
        .iter()
        .filter(|(key, _)| normalize_project_lookup_key((*key).clone()) == lookup_key)
        .collect();
    normalized_matches.sort_by_key(|(key, _)| *key);
    normalized_matches
        .first()
        .map(|(_, project_config)| (**project_config).clone())
}

pub fn validate_reserved_model_provider_ids(
    model_providers: &HashMap<String, ModelProviderInfo>,
) -> Result<(), String> {
    let mut conflicts = model_providers
        .keys()
        .filter(|key| RESERVED_MODEL_PROVIDER_IDS.contains(&key.as_str()))
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>();
    conflicts.sort_unstable();
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "model_providers contains reserved built-in provider IDs: {}. \
Built-in providers cannot be overridden. Rename your custom provider (for example, `odysseythink-custom`).",
            conflicts.join(", ")
        ))
    }
}

pub fn validate_model_providers(
    model_providers: &HashMap<String, ModelProviderInfo>,
) -> Result<(), String> {
    validate_reserved_model_provider_ids(model_providers)?;
    for (key, provider) in model_providers {
        if provider.name.trim().is_empty() {
            return Err(format!(
                "model_providers.{key}: provider name must not be empty"
            ));
        }
        provider
            .validate()
            .map_err(|message| format!("model_providers.{key}: {message}"))?;
    }
    Ok(())
}

fn deserialize_model_providers<'de, D>(
    deserializer: D,
) -> Result<HashMap<String, ModelProviderInfo>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let model_providers = HashMap::<String, ModelProviderInfo>::deserialize(deserializer)?;
    validate_model_providers(&model_providers).map_err(serde::de::Error::custom)?;
    Ok(model_providers)
}



#[cfg(test)]
mod tests {
    use super::*;
    use ody_model_provider_info::WireApi;
    use pretty_assertions::assert_eq;

    #[test]
    fn legacy_base_url_is_ignored_as_unknown_key() {
        let config: ConfigToml = toml::from_str(r#"legacy_base_url = "https://example.com""#)
            .expect("should ignore unknown key");
        let _ = config;
    }

    #[test]
    fn ignores_ody_code_config_fields_by_default() {
        let config: ConfigToml = toml::from_str(
            r#"
default_thinking = true
default_model = "kimi_gyy/kimi-for-coding"

[providers.deepseek_1]
type = "deepseek"
api_key = "sk-test"
base_url = "https://api.deepseek.com/v1"

[models."kimi_gyy/kimi-for-coding"]
provider = "kimi_gyy"
model = "kimi-for-coding"
max_context_size = 262144

[mode_models]
plan = "kimi_gyy/kimi-for-coding"
review = "glm_1/glm-5.1"

[services.moonshot_search]
base_url = "https://api.kimi.com/coding/v1/search"
api_key = ""
"#,
        )
        .expect("ody-code shaped config should deserialize without error");

        assert!(config.model.is_none());
        assert!(config.model_provider.is_none());
        assert!(config.model_providers.is_empty());
    }

    #[test]
    fn convert_ody_code_providers_to_model_providers() {
        let config: ConfigToml = toml::from_str(
            r#"
[providers.kimi_gyy]
type = "kimi"
api_key = "sk-kimi"
base_url = "https://api.kimi.com/v1"

[providers.deepseek_1]
type = "deepseek"
api_key = "sk-deepseek"
base_url = "https://api.deepseek.com/v1"

[providers.openai_custom]
type = "openai"
api_key = "sk-openai"
base_url = "https://api.openai.com/v1"
"#,
        )
        .expect("ody-code providers should deserialize");

        let converted = config.convert_ody_code_providers();
        assert_eq!(converted.len(), 3);

        let kimi = converted.get("kimi_gyy").expect("kimi_gyy provider");
        assert_eq!(kimi.name, "Kimi");
        assert_eq!(
            kimi.base_url,
            Some("https://api.kimi.com/v1".to_string())
        );
        assert_eq!(
            kimi.experimental_bearer_token,
            Some("sk-kimi".to_string())
        );

        let openai_custom = converted
            .get("openai_custom")
            .expect("openai_custom provider");
        assert_eq!(openai_custom.name, "OpenAI");
    }

    #[test]
    fn ody_code_providers_reject_reserved_ids() {
        let config: ConfigToml = toml::from_str(
            r#"
[providers.odysseythink]
type = "openai"
"#,
        )
        .expect("config should deserialize");

        let converted = config.convert_ody_code_providers();
        let result = validate_reserved_model_provider_ids(&converted);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("reserved built-in provider IDs"),
            "error should mention reserved built-in provider IDs"
        );
    }

    #[test]
    fn resolve_ody_code_default_model_splits_provider_and_model() {
        let config: ConfigToml = toml::from_str(
            r#"default_model = "kimi_gyy/kimi-for-coding""#,
        )
        .expect("default_model should deserialize");

        let (provider, model) = config.resolve_ody_code_default_model();
        assert_eq!(provider, Some("kimi_gyy".to_string()));
        assert_eq!(model, Some("kimi-for-coding".to_string()));
    }

    #[test]
    fn resolve_ody_code_default_model_uses_default_provider_without_slash() {
        let config: ConfigToml = toml::from_str(
            r#"
default_provider = "kimi_gyy"
default_model = "kimi-for-coding"
"#,
        )
        .expect("config should deserialize");

        let (provider, model) = config.resolve_ody_code_default_model();
        assert_eq!(provider, Some("kimi_gyy".to_string()));
        assert_eq!(model, Some("kimi-for-coding".to_string()));
    }

    #[test]
    fn convert_ody_code_anthropic_type_is_anthropic_messages() {
        let config: ConfigToml = toml::from_str(
            r#"
[providers.anthropic_custom]
type = "anthropic"
"#,
        )
        .expect("config should deserialize");
        let converted = config.convert_ody_code_providers();
        let provider = converted.get("anthropic_custom").expect("provider");
        assert_eq!(provider.wire_api, WireApi::AnthropicMessages);
    }

    #[test]
    fn convert_ody_code_google_genai_type_is_google_genai() {
        let config: ConfigToml = toml::from_str(
            r#"
[providers.google_custom]
type = "google-genai"
"#,
        )
        .expect("config should deserialize");
        let converted = config.convert_ody_code_providers();
        let provider = converted.get("google_custom").expect("provider");
        assert_eq!(provider.wire_api, WireApi::GoogleGenAI);
    }

    #[test]
    fn convert_ody_code_openai_type_normalizes_capabilities() {
        let config: ConfigToml = toml::from_str(
            r#"
[providers.openai_custom]
type = "openai"
"#,
        )
        .expect("config should deserialize");
        let converted = config.convert_ody_code_providers();
        let provider = converted.get("openai_custom").expect("provider");
        assert_eq!(provider.wire_api, WireApi::Responses);
        assert!(provider.capabilities.supports_websockets);
    }

    #[test]
    fn plan_mode_defaults() {
        let config: ConfigToml = toml::from_str("").expect("empty config should deserialize");
        let plan_mode = config.plan_mode.expect("default plan_mode table should be present");
        assert_eq!(plan_mode.enforcement, Some(PlanEnforcement::Strict));
        assert_eq!(plan_mode.persist_plan_file, Some(true));
        assert_eq!(plan_mode.context_isolation, Some(PlanContextIsolation::Off));
        assert_eq!(plan_mode.split_threshold, Some(8));
        assert!(plan_mode.model.is_none());
        assert!(plan_mode.reasoning_effort.is_none());
    }

    #[test]
    fn plan_mode_defaults_include_cadence() {
        let config: ConfigToml = toml::from_str("").expect("empty config should deserialize");
        let plan_mode = config
            .plan_mode
            .expect("default plan_mode table should be present");
        assert_eq!(plan_mode.full_refresh_turns, Some(5));
        assert_eq!(plan_mode.dedup_min_turns, Some(2));
    }

    #[test]
    fn plan_mode_deserializes_cadence_fields() {
        let config: ConfigToml = toml::from_str(
            r#"
[plan_mode]
full_refresh_turns = 3
dedup_min_turns = 1
"#,
        )
        .expect("plan_mode config should deserialize");

        let plan_mode = config.plan_mode.expect("plan_mode should be present");
        assert_eq!(plan_mode.full_refresh_turns, Some(3));
        assert_eq!(plan_mode.dedup_min_turns, Some(1));
    }

    #[test]
    fn plan_mode_deserializes_all_fields() {
        let config: ConfigToml = toml::from_str(
            r#"
[plan_mode]
enforcement = "ask"
persist_plan_file = false
context_isolation = "on"
model = "kimi-k2-thinking"
reasoning_effort = "high"
split_threshold = 16
"#,
        )
        .expect("plan_mode config should deserialize");

        let plan_mode = config.plan_mode.expect("plan_mode should be present");
        assert_eq!(plan_mode.enforcement, Some(PlanEnforcement::Ask));
        assert_eq!(plan_mode.persist_plan_file, Some(false));
        assert_eq!(plan_mode.context_isolation, Some(PlanContextIsolation::On));
        assert_eq!(plan_mode.model, Some("kimi-k2-thinking".to_string()));
        assert_eq!(plan_mode.reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(plan_mode.split_threshold, Some(16));
    }

    #[test]
    fn default_split_plan_compaction_ratio_is_half() {
        let cfg = PlanModeConfigToml::default();
        assert_eq!(cfg.split_plan_compaction_ratio, Some(0.5));
    }

    #[test]
    fn deserialize_split_plan_compaction_ratio() {
        let toml = r#"
            split_plan_compaction_ratio = 0.75
        "#;
        let cfg: PlanModeConfigToml = toml::from_str(toml).unwrap();
        assert_eq!(cfg.split_plan_compaction_ratio, Some(0.75));
    }

    #[test]
    fn deserialize_split_plan_compaction_ratio_zero_disables() {
        let toml = r#"
            split_plan_compaction_ratio = 0.0
        "#;
        let cfg: PlanModeConfigToml = toml::from_str(toml).unwrap();
        assert_eq!(cfg.split_plan_compaction_ratio, Some(0.0));
    }

    #[test]
    fn deserialize_language_field() {
        let cfg: ConfigToml = toml::from_str(r#"language = "zh""#).unwrap();
        assert_eq!(cfg.language.as_deref(), Some("zh"));
    }
}
