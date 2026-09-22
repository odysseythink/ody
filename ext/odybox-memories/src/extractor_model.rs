//! Model-backed extractor: runs the extraction prompt through the configured
//! model provider.
//!
//! This mirrors `ody-memories-write`'s Phase 1 request shape (system prompt +
//! structured output schema + a single detached model call) but is a separate
//! implementation: it does not touch, call, or share code with the Ody memory
//! pipeline.

use std::sync::Arc;

use futures::StreamExt;
use ody_client::default_client::originator;
use ody_core::ModelClient;
use ody_core::Prompt;
use ody_core::ResponseEvent;
use ody_core::ThreadManager;
use ody_core::config::Config;
use ody_core::content_items_to_text;
use ody_core::detached_memory_responses_metadata;
use ody_core::resolve_installation_id;
use ody_features::Feature;
use ody_model_provider::create_model_provider;
use ody_otel::SessionTelemetry;
use ody_protocol::SessionId;
use ody_protocol::ThreadId;
use ody_protocol::config_types::ReasoningSummary;
use ody_protocol::model_metadata::ModelInfo;
use ody_protocol::model_metadata::ReasoningEffort;
use ody_protocol::models::BaseInstructions;
use ody_protocol::models::ContentItem;
use ody_protocol::models::ResponseItem;
use ody_protocol::protocol::SessionSource;
use ody_rollout_trace::InferenceTraceContext;
use ody_terminal_detection::user_agent;
use serde_json::Value;
use serde_json::json;

use crate::extract::ExtractError;
use crate::extract::ExtractFuture;
use crate::extract::MemoryExtractor;
use crate::prompts;
use crate::prompts::Transcript;

/// Reasoning effort for extraction. Low: this is a summarisation task.
const EXTRACTION_REASONING_EFFORT: ReasoningEffort = ReasoningEffort::Low;

/// Extracts assistant memory by calling the configured model provider.
pub struct ModelMemoryExtractor {
    config: Arc<Config>,
    session_source: SessionSource,
    model_info: ModelInfo,
    session_telemetry: SessionTelemetry,
    reasoning_summary: ReasoningSummary,
}

impl ModelMemoryExtractor {
    pub async fn new(
        thread_manager: Arc<ThreadManager>,
        config: Arc<Config>,
        session_source: SessionSource,
    ) -> anyhow::Result<Self> {
        let provider = create_model_provider(config.model_provider.clone());
        let model_name = config
            .memories
            .extract_model
            .clone()
            .unwrap_or_else(|| provider.memory_extraction_preferred_model().to_string());

        let model_info = thread_manager
            .get_models_manager()
            .get_model_info(&model_name, &config.to_models_manager_config())
            .await;

        let session_telemetry = SessionTelemetry::new(
            ThreadId::default(),
            &model_name,
            &model_name,
            None,
            originator().value,
            config.otel.log_user_prompt,
            user_agent(),
            session_source.clone(),
        );

        let reasoning_summary = config
            .model_reasoning_summary
            .unwrap_or(model_info.default_reasoning_summary);

        Ok(Self {
            config,
            session_source,
            model_info,
            session_telemetry,
            reasoning_summary,
        })
    }
}

impl MemoryExtractor for ModelMemoryExtractor {
    fn extract<'a>(&'a self, transcript: &'a Transcript) -> ExtractFuture<'a> {
        Box::pin(async move {
            let raw = self
                .stream_extraction(transcript)
                .await
                .map_err(|err| ExtractError::Model(err.to_string()))?;
            if raw.trim().is_empty() {
                return Err(ExtractError::Model(
                    "model returned an empty response".to_string(),
                ));
            }
            crate::extract::parse_extraction_result(&raw)
        })
    }
}

impl ModelMemoryExtractor {
    fn output_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string" },
                "claims": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "kind": {
                                "type": "string",
                                "enum": ["preference", "user_profile", "entity", "task", "domain_fact", "procedure", "correction"]
                            },
                            "subject": { "type": "string" },
                            "statement": { "type": "string" },
                            "confidence": { "type": "string", "enum": ["low", "medium", "high"] },
                            "scope": { "type": ["string", "null"] },
                            "decision_implication": { "type": ["string", "null"] },
                            "review_in_days": { "type": ["integer", "null"] },
                            "supersedes": { "type": ["string", "null"] },
                            "evidence": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "item": { "type": "integer" },
                                        "quote": { "type": "string" }
                                    },
                                    "required": ["item", "quote"],
                                    "additionalProperties": false
                                }
                            }
                        },
                        "required": ["kind", "subject", "statement", "confidence", "evidence"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["summary", "claims"],
            "additionalProperties": false
        })
    }

    async fn stream_extraction(&self, transcript: &Transcript) -> anyhow::Result<String> {
        let thread_id = ThreadId::default();
        let installation_id = resolve_installation_id(&self.config.ody_home).await?;

        let mut prompt = Prompt::default();
        prompt.base_instructions = BaseInstructions {
            text: prompts::EXTRACTION_SYSTEM_PROMPT.to_string(),
        };
        prompt.input = vec![ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: prompts::build_extraction_input(transcript, /*truncated*/ false),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }];
        prompt.output_schema = Some(Self::output_schema());
        prompt.output_schema_strict = true;

        let model_client = ModelClient::new(
            thread_id,
            self.config.model_provider.clone(),
            self.session_source.clone(),
            self.config.model_verbosity,
            self.config
                .features
                .enabled(Feature::EnableRequestCompression),
            self.config.features.enabled(Feature::RuntimeMetrics),
            /*beta_features_header*/ None,
            self.config.features.enabled(Feature::ItemIds),
            /*attestation_provider*/ None,
        );
        let mut client_session = model_client.new_session();

        let responses_metadata = detached_memory_responses_metadata(
            installation_id,
            SessionId::from(thread_id).to_string(),
            thread_id.to_string(),
            format!("{thread_id}:0"),
            &self.session_source,
            &self.config.cwd,
            /*sandbox*/ None,
        )
        .await;

        let mut stream = client_session
            .stream(
                &prompt,
                &self.model_info,
                &self.session_telemetry,
                Some(EXTRACTION_REASONING_EFFORT),
                self.reasoning_summary,
                /*service_tier*/ None,
                &responses_metadata,
                &InferenceTraceContext::disabled(),
            )
            .await?;

        let mut result = String::new();
        while let Some(message) = stream.next().await.transpose()? {
            match message {
                ResponseEvent::OutputTextDelta(delta) => result.push_str(&delta),
                ResponseEvent::OutputItemDone(item) => {
                    if result.is_empty()
                        && let ResponseItem::Message { content, .. } = item
                        && let Some(text) = content_items_to_text(&content)
                    {
                        result.push_str(&text);
                    }
                }
                ResponseEvent::Completed { .. } => break,
                _ => {}
            }
        }

        Ok(result)
    }
}
