use ody_protocol::config_types::ReasoningSummary;
use ody_protocol::odysseythink_models::ConfigShellToolType;
use ody_protocol::odysseythink_models::ModelInfo;
use ody_protocol::odysseythink_models::ModelInstructionsVariables;
use ody_protocol::odysseythink_models::ModelMessages;
use ody_protocol::odysseythink_models::ModelVisibility;
use ody_protocol::odysseythink_models::ModelsResponse;
use ody_protocol::odysseythink_models::TruncationMode;
use ody_protocol::odysseythink_models::TruncationPolicyConfig;
use ody_protocol::odysseythink_models::WebSearchToolType;
use ody_protocol::odysseythink_models::default_input_modalities;

use crate::config::ModelsManagerConfig;
use ody_utils_output_truncation::approx_bytes_for_tokens;
use tracing::warn;

pub const BASE_INSTRUCTIONS: &str = include_str!("../prompt.md");
const DEFAULT_PERSONALITY_HEADER: &str = "You are Ody, a coding agent based on GPT-5. You and the user share the same workspace and collaborate to achieve the user's goals.";
const LOCAL_FRIENDLY_TEMPLATE: &str =
    "You optimize for team morale and being a supportive teammate as much as code quality.";
const LOCAL_PRAGMATIC_TEMPLATE: &str = "You are a deeply pragmatic, effective software engineer.";
const PERSONALITY_PLACEHOLDER: &str = "{{ personality }}";

pub fn with_config_overrides(mut model: ModelInfo, config: &ModelsManagerConfig) -> ModelInfo {
    if let Some(supports_reasoning_summaries) = config.model_supports_reasoning_summaries
        && supports_reasoning_summaries
    {
        model.supports_reasoning_summaries = true;
    }
    if let Some(context_window) = config.model_context_window {
        model.context_window = Some(
            model
                .max_context_window
                .map_or(context_window, |max_context_window| {
                    context_window.min(max_context_window)
                }),
        );
    }
    if let Some(auto_compact_token_limit) = config.model_auto_compact_token_limit {
        model.auto_compact_token_limit = Some(auto_compact_token_limit);
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
    ModelInfo {
        slug: slug.to_string(),
        display_name: slug.to_string(),
        description: None,
        default_reasoning_level: None,
        supported_reasoning_levels: Vec::new(),
        shell_type: ConfigShellToolType::Default,
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
        supports_reasoning_summaries: false,
        default_reasoning_summary: ReasoningSummary::Auto,
        support_verbosity: false,
        default_verbosity: None,
        apply_patch_tool_type: None,
        web_search_tool_type: WebSearchToolType::Text,
        truncation_policy: TruncationPolicyConfig::bytes(/*limit*/ 10_000),
        supports_parallel_tool_calls: false,
        supports_image_detail_original: false,
        context_window: Some(272_000),
        max_context_window: Some(272_000),
        auto_compact_token_limit: None,
        comp_hash: None,
        effective_context_window_percent: 95,
        experimental_supported_tools: Vec::new(),
        input_modalities: default_input_modalities(),
        used_fallback_model_metadata: true, // this is the fallback model metadata
        supports_search_tool: false,
        use_responses_lite: false,
        auto_review_model_override: None,
        tool_mode: None,
        multi_agent_version: None,
    }
}

/// Bundled static model catalog for an OpenAI-compatible Chat Completions
/// provider (Kimi / DeepSeek / GLM). These providers do not expose the ody
/// `/models` catalog, so a small curated list is shipped instead. Returns
/// `None` for providers that are not handled here.
pub fn chat_provider_models(provider_id: &str) -> Option<ModelsResponse> {
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
        _ => return None,
    };

    let models = specs
        .iter()
        .enumerate()
        .map(|(index, spec)| spec.to_model_info(index as i32))
        .collect();
    Some(ModelsResponse { models })
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
