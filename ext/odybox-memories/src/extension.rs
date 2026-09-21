//! Extension contributors that mount assistant memory onto a thread.
//!
//! The mounting point mirrors `ody-memories-extension`: a thread-lifecycle
//! contributor decides state, a config contributor refreshes derived paths, and
//! a tool contributor exposes the tool set. The difference is that every step is
//! behind a product gate, so non-odyBox threads behave exactly as if this crate
//! did not exist.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Weak;

use chrono::Utc;
use ody_core::ThreadManager;
use ody_core::config::Config;
use ody_extension_api::ConfigContributor;
use ody_extension_api::ExtensionData;
use ody_extension_api::ExtensionFuture;
use ody_extension_api::ExtensionRegistryBuilder;
use ody_extension_api::ThreadLifecycleContributor;
use ody_extension_api::ThreadResumeInput;
use ody_extension_api::ThreadStartInput;
use ody_extension_api::ToolCall;
use ody_extension_api::ToolContributor;
use ody_protocol::protocol::Product;
use ody_protocol::protocol::SessionSource;
use ody_tools::ToolExecutor;

use crate::backend::AssistantMemoryStore;
use crate::extractor_model::ModelMemoryExtractor;
use crate::gate::assistant_memory_enabled;
use crate::gate::product_for_source;
use crate::ledger::ExtractionLedger;
use crate::pipeline;
use crate::store::assistant_memory_root;
use crate::tools;

/// Thread-scoped assistant-memory state.
///
/// Its presence in the thread store *is* the gate: every downstream contributor
/// reads this and nothing else, so a missing entry means "not an odyBox thread".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssistantMemoryConfig {
    /// The product this thread belongs to. Always [`Product::OdyBox`].
    pub product: Product,
    /// Root directory for assistant memory artifacts.
    pub memory_root: PathBuf,
}

impl AssistantMemoryConfig {
    fn new(config: &Config, product: Product) -> Self {
        Self {
            product,
            memory_root: assistant_memory_root(&config.ody_home),
        }
    }

    fn with_refreshed_root(&self, config: &Config) -> Self {
        Self {
            product: self.product,
            memory_root: assistant_memory_root(&config.ody_home),
        }
    }
}

/// Context captured at thread start, reused when the host resumes the thread.
///
/// `ThreadResumeInput` carries neither `Config` nor `SessionSource`, so a resume
/// cannot re-derive the product gate on its own. We therefore keep what start
/// already resolved and reuse it.
///
/// This covers resumes **within the same process**. A resume after a restart
/// finds an empty thread store and does nothing; that content is still picked
/// up later, because every thread start scans all unprocessed sessions.
#[derive(Clone)]
struct PipelineContext {
    config: Arc<Config>,
    session_source: SessionSource,
}

/// Mounts odyBox assistant memory onto odyBox threads only.
#[derive(Clone, Default)]
pub struct AssistantMemoryExtension {
    /// Used to reach the models manager when building the extraction extractor.
    /// Weak so the extension never keeps the thread manager alive.
    thread_manager: Weak<ThreadManager>,
}

impl AssistantMemoryExtension {
    pub fn new(thread_manager: Weak<ThreadManager>) -> Self {
        Self { thread_manager }
    }
}

impl ThreadLifecycleContributor<Config> for AssistantMemoryExtension {
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            let product = product_for_source(input.session_source);
            if !assistant_memory_enabled(product) {
                // Not an odyBox thread. Insert nothing: the closed gate is
                // observable as an absent thread-store entry.
                tracing::debug!(
                    session_source = %input.session_source,
                    "assistant memory: gate closed for this session source"
                );
                return;
            }
            let Some(product) = product else {
                return;
            };
            let assistant_config = AssistantMemoryConfig::new(input.config, product);
            let memory_root = assistant_config.memory_root.clone();
            input.thread_store.insert(assistant_config);

            let config = Arc::new(input.config.clone());
            let session_source = input.session_source.clone();
            input.thread_store.insert(PipelineContext {
                config: Arc::clone(&config),
                session_source: session_source.clone(),
            });

            tracing::info!(
                product = ?product,
                root = %memory_root.display(),
                "assistant memory: odyBox thread detected, starting write pipeline"
            );
            spawn_write_pipeline(
                self.thread_manager.clone(),
                config,
                memory_root,
                session_source,
            );
        })
    }

    fn on_thread_resume<'a>(&'a self, input: ThreadResumeInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            // No config and no session source are available here: the only signal
            // is what thread start captured in this process.
            let Some(context) = input.thread_store.get::<PipelineContext>() else {
                tracing::debug!(
                    "assistant memory: resume without captured context; skipping pipeline"
                );
                return;
            };

            let memory_root = assistant_memory_root(&context.config.ody_home);
            tracing::info!(
                root = %memory_root.display(),
                "assistant memory: odyBox thread resumed, starting write pipeline"
            );
            spawn_write_pipeline(
                self.thread_manager.clone(),
                Arc::clone(&context.config),
                memory_root,
                context.session_source.clone(),
            );
        })
    }
}

impl ConfigContributor<Config> for AssistantMemoryExtension {
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        _previous_config: &Config,
        new_config: &Config,
    ) {
        // The gate is derived from the session source, which is fixed for the
        // life of a thread, so a config reload must not be able to open it.
        // We only refresh the derived root on threads that are already gated in.
        let Some(existing) = thread_store.get::<AssistantMemoryConfig>() else {
            return;
        };
        thread_store.insert(existing.with_refreshed_root(new_config));
    }
}

impl ToolContributor for AssistantMemoryExtension {
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        let Some(gate) = thread_store.get::<AssistantMemoryConfig>() else {
            // Gate closed (or thread not started yet): expose nothing.
            return Vec::new();
        };

        // Bound to the assistant memory root, never to the Ody memory
        // workspace, so these tools cannot read or write Ody memory files.
        tools::assistant_memory_tools(AssistantMemoryStore::new(gate.memory_root.clone()))
    }
}

/// Installs the assistant-memory contributors into the extension registry.
///
/// Safe to install unconditionally: every contributor is a no-op unless the
/// thread's session source resolves to [`Product::OdyBox`].
pub fn install(
    registry: &mut ExtensionRegistryBuilder<Config>,
    thread_manager: Weak<ThreadManager>,
) {
    tracing::info!("assistant memory: mounting odyBox assistant memory extension");
    let extension = Arc::new(AssistantMemoryExtension::new(thread_manager));
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.tool_contributor(extension);
}

/// Spawns one background write-pipeline pass for an odyBox thread start.
///
/// Best effort: any failure is logged and dropped. The pipeline must never
/// affect thread startup.
fn spawn_write_pipeline(
    thread_manager: Weak<ThreadManager>,
    config: Arc<Config>,
    memory_root: PathBuf,
    session_source: SessionSource,
) {
    tokio::spawn(async move {
        let Some(thread_manager) = thread_manager.upgrade() else {
            tracing::warn!("assistant memory: thread manager was dropped; skipping pipeline");
            return;
        };
        let extractor =
            match ModelMemoryExtractor::new(thread_manager, Arc::clone(&config), session_source)
                .await
            {
                Ok(extractor) => extractor,
                Err(err) => {
                    tracing::warn!("assistant memory: could not build the extractor: {err}");
                    return;
                }
            };

        let ledger = ExtractionLedger::new(memory_root);
        let ody_home = config.ody_home.to_path_buf();
        let recorded_at = Utc::now().to_rfc3339();

        // `get_threads` only consults `default_provider` for provider filtering,
        // which this pipeline does not enable (model_providers = None).
        match pipeline::run_once(
            &extractor,
            &ledger,
            &ody_home,
            /*default_provider*/ "",
            pipeline::DEFAULT_SESSION_BUDGET,
            &recorded_at,
        )
        .await
        {
            Ok(report) => {
                tracing::debug!(?report, "assistant memory pipeline pass complete");
            }
            Err(err) => {
                tracing::warn!("assistant memory pipeline pass failed: {err}");
            }
        }
    });
}
