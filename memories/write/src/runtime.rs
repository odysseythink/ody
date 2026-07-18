use futures::StreamExt;
use ody_client::default_client::originator;
use ody_core::ModelClient;
use ody_core::NewThread;
use ody_core::OdyThread;
use ody_core::Prompt;
use ody_core::ResponseEvent;
use ody_core::StartThreadOptions;
use ody_core::ThreadManager;
use ody_core::config::Config;
use ody_core::content_items_to_text;
use ody_core::detached_memory_responses_metadata;
use ody_core::resolve_installation_id;
use ody_features::Feature;
use ody_model_provider::ModelProvider;
use ody_model_provider::SharedModelProvider;
use ody_model_provider::create_model_provider;
use ody_otel::SessionTelemetry;
use ody_otel::TelemetryAuthMode;
use ody_protocol::SessionId;
use ody_protocol::ThreadId;
use ody_protocol::config_types::ReasoningSummary;
use ody_protocol::model_metadata::ModelInfo;
use ody_protocol::model_metadata::ReasoningEffort;
use ody_protocol::protocol::InitialHistory;
use ody_protocol::protocol::InternalSessionSource;
use ody_protocol::protocol::Op;
use ody_protocol::protocol::SessionSource;
use ody_protocol::protocol::ThreadSource;
use ody_protocol::protocol::TokenUsage;
use ody_protocol::user_input::UserInput;
use ody_rollout_trace::InferenceTraceContext;
use ody_state::StateRuntime;
use ody_terminal_detection::user_agent;
use std::sync::Arc;
use std::time::Duration;

pub(crate) struct SpawnedConsolidationAgent {
    pub(crate) thread_id: ThreadId,
    pub(crate) thread: Arc<OdyThread>,
}

#[derive(Clone, Debug)]
pub(crate) struct StageOneRequestContext {
    pub(crate) model_info: ModelInfo,
    pub(crate) session_telemetry: SessionTelemetry,
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
    pub(crate) reasoning_summary: ReasoningSummary,
    pub(crate) service_tier: Option<String>,
}

impl StageOneRequestContext {
    pub(crate) fn start_timer(&self, name: &str) -> Option<ody_otel::Timer> {
        self.session_telemetry.start_timer(name, &[]).ok()
    }

    pub(crate) fn counter(&self, name: &str, inc: i64, tags: &[(&str, &str)]) {
        self.session_telemetry.counter(name, inc, tags);
    }

    pub(crate) fn histogram(&self, name: &str, value: i64, tags: &[(&str, &str)]) {
        self.session_telemetry.histogram(name, value, tags);
    }
}

pub(crate) struct MemoryStartupContext {
    thread_id: ThreadId,
    thread: Arc<OdyThread>,
    thread_manager: Arc<ThreadManager>,
    provider: SharedModelProvider,
    session_telemetry: SessionTelemetry,
}

impl MemoryStartupContext {
    pub(crate) fn new(
        thread_manager: Arc<ThreadManager>,
        thread_id: ThreadId,
        thread: Arc<OdyThread>,
        config: &Config,
        source: SessionSource,
    ) -> Self {
        let provider = create_model_provider(config.model_provider.clone());
        Self::new_with_provider(thread_manager, thread_id, thread, config, source, provider)
    }

    #[cfg(test)]
    pub(crate) fn new_for_testing(
        thread_manager: Arc<ThreadManager>,
        thread_id: ThreadId,
        thread: Arc<OdyThread>,
        config: &Config,
        source: SessionSource,
        provider: SharedModelProvider,
    ) -> Self {
        Self::new_with_provider(thread_manager, thread_id, thread, config, source, provider)
    }

    fn new_with_provider(
        thread_manager: Arc<ThreadManager>,
        thread_id: ThreadId,
        thread: Arc<OdyThread>,
        config: &Config,
        source: SessionSource,
        provider: SharedModelProvider,
    ) -> Self {
        let model = config.model.as_deref().unwrap_or("unknown");
        let session_telemetry = SessionTelemetry::new(
            thread_id,
            model,
            model,
            None,
            originator().value,
            config.otel.log_user_prompt,
            user_agent(),
            source,
        );

        Self {
            thread_id,
            thread,
            thread_manager,
            provider,
            session_telemetry,
        }
    }

    pub(crate) fn thread_id(&self) -> ThreadId {
        self.thread_id
    }

    pub(crate) fn state_db(&self) -> Option<Arc<StateRuntime>> {
        self.thread.state_db()
    }

    pub(crate) fn provider(&self) -> &dyn ModelProvider {
        self.provider.as_ref()
    }

    pub(crate) fn counter(&self, name: &str, inc: i64, tags: &[(&str, &str)]) {
        self.session_telemetry.counter(name, inc, tags);
    }

    pub(crate) fn histogram(&self, name: &str, value: i64, tags: &[(&str, &str)]) {
        self.session_telemetry.histogram(name, value, tags);
    }

    pub(crate) fn start_timer(&self, name: &str) -> Option<ody_otel::Timer> {
        self.session_telemetry.start_timer(name, &[]).ok()
    }

    pub(crate) async fn stage_one_request_context(
        &self,
        config: &Config,
        model_name: &str,
        reasoning_effort: ReasoningEffort,
    ) -> StageOneRequestContext {
        let config_snapshot = self.thread.config_snapshot().await;
        let model_info = self
            .thread_manager
            .get_models_manager()
            .get_model_info(model_name, &config.to_models_manager_config())
            .await;
        let reasoning_summary = config
            .model_reasoning_summary
            .unwrap_or(model_info.default_reasoning_summary);

        StageOneRequestContext {
            model_info,
            session_telemetry: self
                .session_telemetry
                .clone()
                .with_model(model_name, model_name),
            reasoning_effort: Some(reasoning_effort),
            reasoning_summary,
            service_tier: config_snapshot.service_tier,
        }
    }

    pub(crate) async fn stream_stage_one_prompt(
        &self,
        config: &Config,
        prompt: &Prompt,
        context: &StageOneRequestContext,
    ) -> anyhow::Result<(String, Option<TokenUsage>)> {
        let installation_id = resolve_installation_id(&config.ody_home).await?;
        let config_snapshot = self.thread.config_snapshot().await;
        let session_source = config_snapshot.session_source;
        let session_id = SessionId::from(self.thread_id);
        let session_id_string = session_id.to_string();
        let model_client = ModelClient::new(
            self.thread_id,
            config.model_provider.clone(),
            session_source.clone(),
            config.model_verbosity,
            config.features.enabled(Feature::EnableRequestCompression),
            config.features.enabled(Feature::RuntimeMetrics),
            /*beta_features_header*/ None,
            config.features.enabled(Feature::ItemIds),
            /*attestation_provider*/ None,
        );

        let mut client_session = model_client.new_session();
        let window_id = format!("{}:0", self.thread_id);
        let responses_metadata = detached_memory_responses_metadata(
            installation_id,
            session_id_string,
            self.thread_id.to_string(),
            window_id,
            &session_source,
            &config.cwd,
            /*sandbox*/ None,
        )
        .await;
        let mut stream = client_session
            .stream(
                prompt,
                &context.model_info,
                &context.session_telemetry,
                context.reasoning_effort.clone(),
                context.reasoning_summary,
                context.service_tier.clone(),
                &responses_metadata,
                &InferenceTraceContext::disabled(),
            )
            .await?;

        let mut result = String::new();
        let mut token_usage = None;
        while let Some(message) = stream.next().await.transpose()? {
            match message {
                ResponseEvent::OutputTextDelta(delta) => result.push_str(&delta),
                ResponseEvent::OutputItemDone(item) => {
                    if result.is_empty()
                        && let ody_protocol::models::ResponseItem::Message { content, .. } = item
                        && let Some(text) = content_items_to_text(&content)
                    {
                        result.push_str(&text);
                    }
                }
                ResponseEvent::Completed {
                    token_usage: usage, ..
                } => {
                    token_usage = usage;
                    break;
                }
                _ => {}
            }
        }

        Ok((result, token_usage))
    }

    pub(crate) async fn spawn_consolidation_agent(
        &self,
        config: Config,
        prompt: Vec<UserInput>,
    ) -> anyhow::Result<SpawnedConsolidationAgent> {
        let environments = self
            .thread_manager
            .default_environment_selections(&config.cwd);
        let NewThread {
            thread_id, thread, ..
        } = self
            .thread_manager
            .start_thread_with_options(StartThreadOptions {
                config,
                initial_history: InitialHistory::New,
                session_source: Some(SessionSource::Internal(
                    InternalSessionSource::MemoryConsolidation,
                )),
                thread_source: Some(ThreadSource::MemoryConsolidation),
                dynamic_tools: Vec::new(),
                metrics_service_name: None,
                parent_trace: None,
                environments,
                thread_extension_init: Default::default(),
                supports_form_elicitation: false,
            })
            .await?;

        let agent = SpawnedConsolidationAgent { thread_id, thread };
        if let Err(err) = agent
            .thread
            .submit(Op::UserInput {
                items: prompt,
                final_output_json_schema: None,
                responsesapi_client_metadata: None,
                additional_context: Default::default(),
                thread_settings: Default::default(),
            })
            .await
        {
            if let Err(shutdown_err) = self.shutdown_consolidation_agent(agent).await {
                tracing::warn!(
                    "failed to shut down consolidation agent after submit error: {shutdown_err}"
                );
            }
            return Err(err.into());
        }

        Ok(agent)
    }

    pub(crate) async fn shutdown_consolidation_agent(
        &self,
        agent: SpawnedConsolidationAgent,
    ) -> anyhow::Result<()> {
        let SpawnedConsolidationAgent { thread_id, thread } = agent;
        let thread = self
            .thread_manager
            .remove_thread(&thread_id)
            .await
            .unwrap_or(thread);

        tokio::time::timeout(Duration::from_secs(10), thread.shutdown_and_wait())
            .await
            .map_err(|_| {
                anyhow::anyhow!("memory consolidation agent {thread_id} shutdown timed out")
            })??;

        Ok(())
    }
}
