//! Extension contributors that mount assistant memory onto a thread.
//!
//! The mounting point mirrors `ody-memories-extension`: a thread-lifecycle
//! contributor decides state, a config contributor refreshes derived paths, and
//! a tool contributor exposes the tool set. The difference is that every step is
//! behind a product gate, so non-odyBox threads behave exactly as if this crate
//! did not exist.

use std::path::PathBuf;
use std::sync::Arc;

use ody_core::config::Config;
use ody_extension_api::ConfigContributor;
use ody_extension_api::ExtensionData;
use ody_extension_api::ExtensionFuture;
use ody_extension_api::ExtensionRegistryBuilder;
use ody_extension_api::ThreadLifecycleContributor;
use ody_extension_api::ThreadStartInput;
use ody_extension_api::ToolCall;
use ody_extension_api::ToolContributor;
use ody_protocol::protocol::Product;
use ody_tools::ToolExecutor;

use crate::backend::AssistantMemoryStore;
use crate::gate::assistant_memory_enabled;
use crate::gate::product_for_source;
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

/// Mounts odyBox assistant memory onto odyBox threads only.
#[derive(Clone, Default)]
pub struct AssistantMemoryExtension;

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
                return;
            }
            let Some(product) = product else {
                return;
            };
            input
                .thread_store
                .insert(AssistantMemoryConfig::new(input.config, product));
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
pub fn install(registry: &mut ExtensionRegistryBuilder<Config>) {
    let extension = Arc::new(AssistantMemoryExtension);
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.tool_contributor(extension);
}
