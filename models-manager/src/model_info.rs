use ody_model_provider_info::ProviderCapabilities;
use ody_model_provider_info::WireApi;
use ody_protocol::config_types::ReasoningSummary;
use ody_protocol::model_metadata::InputModality;
use ody_protocol::model_metadata::ModelCapabilities;
use ody_protocol::model_metadata::ModelInfo;
use ody_protocol::model_metadata::ModelInstructionsVariables;
use ody_protocol::model_metadata::ModelMessages;
use ody_protocol::model_metadata::ModelVisibility;
use ody_protocol::model_metadata::ModelsResponse;
use ody_protocol::model_metadata::TruncationMode;
use ody_protocol::model_metadata::TruncationPolicyConfig;

use crate::config::ModelsManagerConfig;
use ody_utils_output_truncation::approx_bytes_for_tokens;
use tracing::warn;

pub const BASE_INSTRUCTIONS: &str = include_str!("../prompt.md");

/// Truncation policy applied when neither the catalog nor user config provides
/// one. A zero limit would truncate every tool output down to just the
/// `…N chars truncated…` marker (see `truncate_with_byte_estimate` in
/// `ody-utils-string`), leaving the model blind to all shell output. 10_000
/// bytes matches the hosted `/models` catalog default.
pub const DEFAULT_TRUNCATION_POLICY: TruncationPolicyConfig = TruncationPolicyConfig::bytes(10_000);
const DEFAULT_PERSONALITY_HEADER: &str = "You are Ody, a coding agent based on GPT-5. You and the user share the same workspace and collaborate to achieve the user's goals.";
const LOCAL_FRIENDLY_TEMPLATE: &str =
    "You optimize for team morale and being a supportive teammate as much as code quality.";
const LOCAL_PRAGMATIC_TEMPLATE: &str = "You are a deeply pragmatic, effective software engineer.";
const PERSONALITY_PLACEHOLDER: &str = "{{ personality }}";

/// Model capability fallback when no user config or built-in catalog value exists.
pub fn default_model_capabilities_for_wire_api(wire_api: WireApi) -> ModelCapabilities {
    use ody_protocol::model_metadata::InputModality::{Image, Text};
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
    use ody_protocol::model_metadata::WebSearchToolType;

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

    // A zero truncation budget hides every tool output from the model (the
    // truncator keeps only the "…N chars truncated…" marker), so clamp to a
    // usable default when no source provided a positive limit.
    if caps.truncation_policy.limit <= 0 {
        caps.truncation_policy = DEFAULT_TRUNCATION_POLICY;
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

/// Build a minimal fallback model descriptor for missing/unknown slugs using the
/// given provider context.
///
/// `provider_id` and `provider_caps` let the fallback respect the provider's
/// wire API and capability matrix instead of always falling back to conservative
/// Local defaults.
pub fn model_info_from_slug_with_provider(
    slug: &str,
    provider_id: &str,
    wire_api: WireApi,
    provider_caps: &ProviderCapabilities,
) -> ModelInfo {
    warn!(
        "Unknown model {slug} for provider {provider_id} is used. This will use fallback model metadata."
    );
    let caps = resolve_model_capabilities(provider_caps, wire_api, None, None, slug);
    ModelInfo {
        slug: slug.to_string(),
        display_name: slug.to_string(),
        description: None,
        default_reasoning_level: None,
        supported_reasoning_levels: Vec::new(),
        shell_type: caps.shell_type,
        visibility: ModelVisibility::None,
        provider: provider_id.to_string(),
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

/// Build a minimal fallback model descriptor for missing/unknown slugs.
///
/// This is a convenience wrapper that falls back to conservative Chat provider
/// defaults. Prefer `model_info_from_slug_with_provider` when the active
/// provider is known.
pub fn model_info_from_slug(slug: &str) -> ModelInfo {
    model_info_from_slug_with_provider(slug, slug, WireApi::Chat, &ProviderCapabilities::default())
}

/// A model declared in user config via `[models."provider/model"]` tables.
///
/// This mirrors the ody-code shaped `OdyCodeModelConfig` (converted by the
/// caller) and lets user-declared models participate in the catalog with real
/// metadata instead of fallback metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfiguredModelSpec {
    /// Provider id this model belongs to.
    pub provider: String,
    /// Model name sent to the provider.
    pub model: String,
    /// Total context window size in tokens.
    pub max_context_size: Option<i64>,
    /// Maximum output tokens per response.
    pub max_output_size: Option<i64>,
    /// Capability flags, e.g. "tool_use", "thinking", "image_in".
    pub capabilities: Vec<String>,
    /// Human-readable name shown in model pickers.
    pub display_name: Option<String>,
}

/// Catalog built from user-declared `[models.*]` config entries.
///
/// Returns `None` when no entry targets `provider_id`, so callers can fall
/// back to the static/remote catalog chain unchanged.
pub fn configured_model_catalog_for_provider(
    provider_id: &str,
    wire_api: WireApi,
    provider_caps: &ProviderCapabilities,
    entries: &[ConfiguredModelSpec],
) -> Option<ModelsResponse> {
    let models: Vec<ModelInfo> = entries
        .iter()
        .filter(|entry| entry.provider == provider_id)
        .enumerate()
        .map(|(index, entry)| {
            entry.to_model_info(index as i32, provider_id, wire_api, provider_caps)
        })
        .collect();
    if models.is_empty() {
        None
    } else {
        Some(ModelsResponse { models })
    }
}

impl ConfiguredModelSpec {
    fn to_model_info(
        &self,
        priority: i32,
        provider_id: &str,
        wire_api: WireApi,
        provider_caps: &ProviderCapabilities,
    ) -> ModelInfo {
        let declared = |flag: &str| self.capabilities.iter().any(|cap| cap == flag);
        let caps = if self.capabilities.is_empty() {
            // No explicit capability list: infer from the wire API like the
            // built-in catalogs do.
            default_model_capabilities_for_wire_api(wire_api)
        } else {
            let vision = declared("image_in") || declared("video_in");
            ModelCapabilities {
                supports_tools: declared("tool_use"),
                supports_parallel_tool_calls: declared("tool_use"),
                supports_thinking: declared("thinking"),
                supports_reasoning_summaries: declared("thinking"),
                supports_vision: vision,
                input_modalities: if vision {
                    vec![InputModality::Text, InputModality::Image]
                } else {
                    vec![InputModality::Text]
                },
                ..Default::default()
            }
        };
        let mut caps = ModelCapabilities {
            context_window: self.max_context_size,
            max_context_window: self.max_context_size,
            max_output_tokens: self.max_output_size,
            ..caps
        };
        // Apply provider-level upper bounds and consistency clamps (including
        // the non-zero truncation budget guarantee).
        caps = resolve_model_capabilities(provider_caps, wire_api, Some(&caps), None, &self.model);

        let mut model =
            model_info_from_slug_with_provider(&self.model, provider_id, wire_api, provider_caps);
        model.display_name = self
            .display_name
            .clone()
            .unwrap_or_else(|| self.model.clone());
        model.visibility = ModelVisibility::List;
        model.priority = priority;
        model.used_fallback_model_metadata = false;
        model.context_window = caps.context_window;
        model.max_context_window = caps.max_context_window;
        model.supports_reasoning_summaries = caps.supports_reasoning_summaries;
        model.supports_parallel_tool_calls = caps.supports_parallel_tool_calls;
        model.capabilities = caps;
        // Keep top-level fields in sync with capabilities.
        model.input_modalities = model.capabilities.input_modalities.clone();
        model.web_search_tool_type = model.capabilities.web_search_tool_type;
        model.truncation_policy = model.capabilities.truncation_policy;
        model.shell_type = model.capabilities.shell_type;
        model.tool_mode = model.capabilities.tool_mode.clone();
        model.effective_context_window_percent =
            model.capabilities.effective_context_window_percent;
        model
    }
}

/// Providers whose bundled models ship Ody personality instructions.
///
/// Personality is a pure prompt-layer feature: it swaps a `{{ personality }}`
/// placeholder in the model instructions for a short personality blurb, so any
/// instruction-following model supports it once we supply the template and the
/// per-personality text below. It requires no special model/provider capability
/// (unlike reasoning levels, which the provider API must actually honor).
pub fn provider_supports_personality(provider: &str) -> bool {
    matches!(provider, "kimi" | "deepseek" | "glm")
}

/// Build personality-enabled `ModelMessages` from a model's base instructions.
///
/// The template is the model's own `base_instructions` with a `{{ personality }}`
/// placeholder prepended; [`ModelInfo::get_model_instructions`] swaps the
/// placeholder for the selected personality blurb (empty for the default), so the
/// non-personality output is the base instructions unchanged.
pub fn personality_messages_from_base_instructions(base_instructions: &str) -> ModelMessages {
    ModelMessages {
        instructions_template: Some(format!(
            "{PERSONALITY_PLACEHOLDER}\n\n{base_instructions}"
        )),
        instructions_variables: Some(ModelInstructionsVariables {
            personality_default: Some(String::new()),
            personality_friendly: Some(LOCAL_FRIENDLY_TEMPLATE.to_string()),
            personality_pragmatic: Some(LOCAL_PRAGMATIC_TEMPLATE.to_string()),
        }),
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
