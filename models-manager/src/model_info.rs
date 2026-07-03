use ody_model_provider_info::ModelProviderInfo;
use ody_model_provider_info::ProviderCapabilities;
use ody_model_provider_info::WireApi;
use ody_protocol::config_types::ReasoningSummary;
use ody_protocol::odysseythink_models::InputModality;
use ody_protocol::odysseythink_models::ModelCapabilities;
use ody_protocol::odysseythink_models::ModelInfo;
use ody_protocol::odysseythink_models::ModelInstructionsVariables;
use ody_protocol::odysseythink_models::ModelMessages;
use ody_protocol::odysseythink_models::ModelVisibility;
use ody_protocol::odysseythink_models::ModelsResponse;
use ody_protocol::odysseythink_models::TruncationMode;
use ody_protocol::odysseythink_models::TruncationPolicyConfig;

use crate::config::ModelsManagerConfig;
use ody_utils_output_truncation::approx_bytes_for_tokens;
use tracing::warn;

pub const BASE_INSTRUCTIONS: &str = include_str!("../prompt.md");
const DEFAULT_PERSONALITY_HEADER: &str = "You are Ody, a coding agent based on GPT-5. You and the user share the same workspace and collaborate to achieve the user's goals.";
const LOCAL_FRIENDLY_TEMPLATE: &str =
    "You optimize for team morale and being a supportive teammate as much as code quality.";
const LOCAL_PRAGMATIC_TEMPLATE: &str = "You are a deeply pragmatic, effective software engineer.";
const PERSONALITY_PLACEHOLDER: &str = "{{ personality }}";

/// Model capability fallback when no user config or built-in catalog value exists.
pub fn default_model_capabilities_for_wire_api(wire_api: WireApi) -> ModelCapabilities {
    use ody_protocol::odysseythink_models::InputModality::{Image, Text};
    match wire_api {
        WireApi::Responses => ModelCapabilities {
            supports_tools: true,
            supports_vision: true,
            supports_multiple_system_messages: true,
            input_modalities: vec![Text, Image],
            ..Default::default()
        },
        WireApi::Chat | WireApi::GoogleGenAI => ModelCapabilities {
            supports_tools: true,
            supports_vision: true,
            input_modalities: vec![Text, Image],
            ..Default::default()
        },
        WireApi::AnthropicMessages => ModelCapabilities {
            supports_tools: true,
            supports_vision: true,
            supports_turn_pause: true,
            input_modalities: vec![Text, Image],
            ..Default::default()
        },
        WireApi::Local => ModelCapabilities {
            supports_tools: true,
            input_modalities: vec![Text],
            ..Default::default()
        },
    }
}

/// Resolve model capabilities from configured / built-in / inferred sources.
///
/// Precedence: configured > built_in > wire_api inference > conservative default.
/// Also applies provider-level upper bounds and consistency clamps.
pub fn resolve_model_capabilities(
    provider_caps: &ProviderCapabilities,
    wire_api: WireApi,
    configured: Option<&ModelCapabilities>,
    built_in: Option<&ModelCapabilities>,
    model_slug: &str,
) -> ModelCapabilities {
    use ody_protocol::odysseythink_models::WebSearchToolType;

    let mut caps = if let Some(configured) = configured {
        tracing::debug!(model = %model_slug, "model capabilities from user config");
        configured.clone()
    } else if let Some(built_in) = built_in {
        tracing::debug!(model = %model_slug, "model capabilities from bundled catalog");
        built_in.clone()
    } else {
        tracing::debug!(model = %model_slug, wire_api = ?wire_api, "model capabilities inferred from wire_api");
        default_model_capabilities_for_wire_api(wire_api)
    };

    // Provider-level web_search is an upper bound for model-level search support.
    if !provider_caps.web_search {
        caps.supports_search_tool = false;
        caps.web_search_tool_type = WebSearchToolType::Text;
    }

    // Context window consistency: context_window must not exceed max_context_window,
    // and if missing it inherits max_context_window.
    if let (Some(context), Some(max)) = (caps.context_window, caps.max_context_window) {
        caps.context_window = Some(context.min(max));
    } else if caps.context_window.is_none() && caps.max_context_window.is_some() {
        caps.context_window = caps.max_context_window;
    }

    // auto_compact_token_limit must not exceed 90% of the resolved context window.
    if let Some(context_window) = caps.context_window {
        let max_auto_compact = (context_window * 9) / 10;
        if let Some(limit) = caps.auto_compact_token_limit {
            caps.auto_compact_token_limit = Some(limit.min(max_auto_compact));
        }
    }

    caps
}

pub fn with_config_overrides(mut model: ModelInfo, config: &ModelsManagerConfig) -> ModelInfo {
    if let Some(supports_reasoning_summaries) = config.model_supports_reasoning_summaries
        && supports_reasoning_summaries
    {
        model.supports_reasoning_summaries = true;
        model.capabilities.supports_reasoning_summaries = true;
    }
    if let Some(context_window) = config.model_context_window {
        model.context_window = Some(
            model
                .max_context_window
                .map_or(context_window, |max_context_window| {
                    context_window.min(max_context_window)
                }),
        );
        model.capabilities.context_window = model.context_window;
    }
    if let Some(auto_compact_token_limit) = config.model_auto_compact_token_limit {
        model.auto_compact_token_limit = Some(auto_compact_token_limit);
        model.capabilities.auto_compact_token_limit = model.auto_compact_token_limit;
    }
    if let Some(token_limit) = config.tool_output_token_limit {
        model.truncation_policy = match model.truncation_policy.mode {
            TruncationMode::Bytes => {
                let byte_limit =
                    i64::try_from(approx_bytes_for_tokens(token_limit)).unwrap_or(i64::MAX);
                TruncationPolicyConfig::bytes(byte_limit)
            }
            TruncationMode::Tokens => {
                let limit = i64::try_from(token_limit).unwrap_or(i64::MAX);
                TruncationPolicyConfig::tokens(limit)
            }
        };
        model.capabilities.truncation_policy = model.truncation_policy;
    }

    if let Some(base_instructions) = &config.base_instructions {
        model.base_instructions = base_instructions.clone();
        model.model_messages = None;
    } else if !config.personality_enabled {
        model.model_messages = None;
    }

    model
}

/// Build a minimal fallback model descriptor for missing/unknown slugs.
pub fn model_info_from_slug(slug: &str) -> ModelInfo {
    warn!("Unknown model {slug} is used. This will use fallback model metadata.");
    let caps = resolve_model_capabilities(
        &ProviderCapabilities::default(),
        WireApi::Local,
        None,
        None,
        slug,
    );
    ModelInfo {
        slug: slug.to_string(),
        display_name: slug.to_string(),
        description: None,
        default_reasoning_level: None,
        supported_reasoning_levels: Vec::new(),
        shell_type: caps.shell_type,
        visibility: ModelVisibility::None,
        supported_in_api: true,
        priority: 99,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        availability_nux: None,
        upgrade: None,
        base_instructions: BASE_INSTRUCTIONS.to_string(),
        model_messages: local_personality_messages_for_slug(slug),
        supports_reasoning_summaries: caps.supports_reasoning_summaries,
        default_reasoning_summary: ReasoningSummary::Auto,
        support_verbosity: false,
        default_verbosity: None,
        apply_patch_tool_type: None,
        web_search_tool_type: caps.web_search_tool_type,
        truncation_policy: caps.truncation_policy,
        supports_parallel_tool_calls: caps.supports_parallel_tool_calls,
        supports_image_detail_original: caps.supports_image_detail_original,
        context_window: caps.context_window,
        max_context_window: caps.max_context_window,
        auto_compact_token_limit: caps.auto_compact_token_limit,
        comp_hash: None,
        effective_context_window_percent: caps.effective_context_window_percent,
        experimental_supported_tools: Vec::new(),
        input_modalities: caps.input_modalities.clone(),
        used_fallback_model_metadata: true, // this is the fallback model metadata
        supports_search_tool: caps.supports_search_tool,
        use_responses_lite: false,
        auto_review_model_override: None,
        tool_mode: caps.tool_mode.clone(),
        multi_agent_version: None,
        capabilities: caps,
    }
}

/// Bundled or fallback model catalog for a provider.
///
/// Returns a curated static catalog for the built-in OpenAI-compatible Chat
/// Completions providers (Kimi / DeepSeek / GLM). For other Chat Completions
/// providers and for Local providers, returns a single-model fallback catalog
/// inferred from the wire API. Returns `None` for providers whose catalog is
/// fetched dynamically (Responses / Anthropic Messages / Google GenAI).
pub fn model_catalog_for_provider(
    provider_id: &str,
    info: &ModelProviderInfo,
) -> Option<ModelsResponse> {
    match info.wire_api {
        WireApi::Chat => {
            let specs: &[ChatModelSpec] = match provider_id {
                "kimi" => &[
                    ChatModelSpec::thinking("kimi-k2-0711", "Kimi K2 (0711)", 262_144),
                    ChatModelSpec::thinking("kimi-k2-1024", "Kimi K2 (1024)", 262_144),
                    ChatModelSpec::plain("kimi-for-coding", "Kimi for Coding", 262_144),
                ],
                "deepseek" => &[
                    ChatModelSpec::plain("deepseek-chat", "DeepSeek Chat", 65_536),
                    ChatModelSpec::thinking("deepseek-reasoner", "DeepSeek Reasoner", 65_536),
                ],
                "glm" => &[
                    ChatModelSpec::thinking("glm-4.6", "GLM-4.6", 200_000),
                    ChatModelSpec::thinking("glm-4.5", "GLM-4.5", 131_072),
                    ChatModelSpec::plain("glm-4.5-air", "GLM-4.5 Air", 131_072),
                ],
                _ => {
                    let model = fallback_catalog_model(provider_id, WireApi::Chat, 128_000);
                    return Some(ModelsResponse {
                        models: vec![model],
                    });
                }
            };

            let models = specs
                .iter()
                .enumerate()
                .map(|(index, spec)| spec.to_model_info(index as i32))
                .collect();
            Some(ModelsResponse { models })
        }
        WireApi::Local => {
            let model = fallback_catalog_model(provider_id, WireApi::Local, 32_768);
            Some(ModelsResponse {
                models: vec![model],
            })
        }
        WireApi::Responses | WireApi::AnthropicMessages | WireApi::GoogleGenAI => None,
    }
}

/// Build a single-model fallback descriptor inferred from a wire API.
fn fallback_catalog_model(provider_id: &str, wire_api: WireApi, context_window: i64) -> ModelInfo {
    let mut caps = resolve_model_capabilities(
        &ProviderCapabilities::default(),
        wire_api,
        None,
        None,
        provider_id,
    );
    caps.context_window = Some(context_window);
    caps.max_context_window = Some(context_window);
    caps.supports_parallel_tool_calls = true;

    let mut model = model_info_from_slug(provider_id);
    model.visibility = ModelVisibility::List;
    model.priority = 0;
    model.supports_parallel_tool_calls = true;
    model.context_window = Some(context_window);
    model.max_context_window = Some(context_window);
    model.capabilities = caps;
    // Keep top-level fields in sync with capabilities.
    model.input_modalities = model.capabilities.input_modalities.clone();
    model.web_search_tool_type = model.capabilities.web_search_tool_type;
    model.truncation_policy = model.capabilities.truncation_policy;
    model.shell_type = model.capabilities.shell_type;
    model.tool_mode = model.capabilities.tool_mode.clone();
    model.effective_context_window_percent = model.capabilities.effective_context_window_percent;
    model.used_fallback_model_metadata = true;
    model
}

struct ChatModelSpec {
    slug: &'static str,
    display_name: &'static str,
    context_window: i64,
    supports_thinking: bool,
}

impl ChatModelSpec {
    const fn thinking(slug: &'static str, display_name: &'static str, context_window: i64) -> Self {
        Self {
            slug,
            display_name,
            context_window,
            supports_thinking: true,
        }
    }

    const fn plain(slug: &'static str, display_name: &'static str, context_window: i64) -> Self {
        Self {
            slug,
            display_name,
            context_window,
            supports_thinking: false,
        }
    }

    fn to_model_info(&self, priority: i32) -> ModelInfo {
        let mut model = model_info_from_slug(self.slug);
        model.display_name = self.display_name.to_string();
        model.visibility = ModelVisibility::List;
        model.priority = priority;
        model.supports_parallel_tool_calls = true;
        model.context_window = Some(self.context_window);
        model.max_context_window = Some(self.context_window);
        model.supports_reasoning_summaries = self.supports_thinking;
        model.used_fallback_model_metadata = false;

        model.capabilities = ModelCapabilities {
            context_window: Some(self.context_window),
            max_context_window: Some(self.context_window),
            supports_thinking: self.supports_thinking,
            supports_reasoning_summaries: self.supports_thinking,
            supports_tools: true,
            supports_parallel_tool_calls: true,
            supports_vision: true,
            supports_image_detail_original: false,
            input_modalities: vec![InputModality::Text, InputModality::Image],
            ..Default::default()
        };
        // Keep top-level fields in sync with capabilities.
        model.input_modalities = model.capabilities.input_modalities.clone();
        model.web_search_tool_type = model.capabilities.web_search_tool_type;
        model.truncation_policy = model.capabilities.truncation_policy;
        model.shell_type = model.capabilities.shell_type;
        model.tool_mode = model.capabilities.tool_mode.clone();
        model.effective_context_window_percent = model.capabilities.effective_context_window_percent;
        model
    }
}

fn local_personality_messages_for_slug(slug: &str) -> Option<ModelMessages> {
    match slug {
        "gpt-5.2-ody" | "exp-ody-personality" => Some(ModelMessages {
            instructions_template: Some(format!(
                "{DEFAULT_PERSONALITY_HEADER}\n\n{PERSONALITY_PLACEHOLDER}\n\n{BASE_INSTRUCTIONS}"
            )),
            instructions_variables: Some(ModelInstructionsVariables {
                personality_default: Some(String::new()),
                personality_friendly: Some(LOCAL_FRIENDLY_TEMPLATE.to_string()),
                personality_pragmatic: Some(LOCAL_PRAGMATIC_TEMPLATE.to_string()),
            }),
        }),
        _ => None,
    }
}

#[cfg(test)]
#[path = "model_info_tests.rs"]
mod tests;
