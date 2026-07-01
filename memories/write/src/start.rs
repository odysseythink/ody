use crate::extensions::seed_extension_instructions;
use crate::memory_root;
use crate::phase1;
use crate::phase2;
use crate::runtime::MemoryStartupContext;
use ody_core::OdyThread;
use ody_core::ThreadManager;
use ody_core::config::Config;
use ody_features::Feature;
use ody_login::AuthManager;
use ody_protocol::ThreadId;
use ody_protocol::protocol::SessionSource;
use std::sync::Arc;
use tracing::warn;

/// Starts the asynchronous startup memory pipeline for an eligible root session.
///
/// The pipeline is skipped for ephemeral sessions, disabled feature flags, and
/// subagent sessions.
pub fn start_memories_startup_task(
    thread_manager: Arc<ThreadManager>,
    auth_manager: Arc<AuthManager>,
    thread_id: ThreadId,
    thread: Arc<OdyThread>,
    config: Arc<Config>,
    source: &SessionSource,
) {
    if config.ephemeral
        || !config.features.enabled(Feature::MemoryTool)
        || source.is_non_root_agent()
    {
        return;
    }

    let context = Arc::new(MemoryStartupContext::new(
        thread_manager,
        Arc::clone(&auth_manager),
        thread_id,
        thread,
        config.as_ref(),
        source.clone(),
    ));

    if context.state_db().is_none() {
        warn!("state db unavailable for memories startup pipeline; skipping");
        return;
    }

    tokio::spawn(async move {
        let root = memory_root(&config.ody_home);
        if let Err(err) = tokio::fs::create_dir_all(&root).await {
            warn!("failed creating memories root: {err}");
            return;
        }
        if let Err(err) = seed_extension_instructions(&root).await {
            warn!("failed seeding memory extension instructions: {err}");
        }

        // Clean memories to make preserve DB size. This does not consume tokens so can be
        // done before the quota check.
        phase1::prune(context.as_ref(), &config).await;

        // Run phase 1.
        phase1::run(Arc::clone(&context), Arc::clone(&config)).await;
        // Run phase 2.
        phase2::run(context, config).await;
    });
}
