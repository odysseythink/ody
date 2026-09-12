use super::*;

impl AgentControl {
    /// Remove a finished sub-agent and free its spawn slot without waiting for
    /// its loop to process a shutdown op. The flow host uses this on the
    /// completion path so the next step can spawn immediately instead of
    /// paying a shutdown round-trip on the flow's critical path (D1); the
    /// sub-agent's own loop already persisted its rollout before reporting the
    /// final status.
    pub(crate) async fn release_finished_agent(&self, agent_id: ThreadId) {
        let Ok(state) = self.upgrade() else {
            return;
        };
        let _ = state.remove_thread(&agent_id).await;
        self.forget_v2_residency(agent_id);
        self.state.release_spawned_thread(agent_id);
    }

    /// Submit a shutdown request for a live agent without marking it explicitly closed in
    /// persisted spawn-edge state.
    pub(crate) async fn shutdown_live_agent(&self, agent_id: ThreadId) -> OdyResult<String> {
        let state = self.upgrade()?;
        let result = if let Ok(thread) = state.get_thread(agent_id).await {
            thread.ody.session.ensure_rollout_materialized().await;
            thread.ody.session.flush_rollout().await?;
            let result = if matches!(thread.agent_status().await, AgentStatus::Shutdown) {
                Ok(String::new())
            } else {
                state.send_op(agent_id, Op::Shutdown {}).await
            };
            thread.wait_until_terminated().await;
            result
        } else {
            state.send_op(agent_id, Op::Shutdown {}).await
        };
        let _ = state.remove_thread(&agent_id).await;
        self.forget_v2_residency(agent_id);
        self.state.release_spawned_thread(agent_id);
        result
    }

    /// Mark `agent_id` as explicitly closed in persisted spawn-edge state, then shut down the
    /// agent and any live descendants reached from the in-memory tree.
    pub(crate) async fn close_agent(&self, agent_id: ThreadId) -> OdyResult<String> {
        let state = self.upgrade()?;
        let known_agent = self.state.agent_metadata_for_thread(agent_id).is_some();
        match state.get_thread(agent_id).await {
            Ok(thread) => {
                if let Some(state_db_ctx) = thread.state_db()
                    && let Err(err) = state_db_ctx
                        .set_thread_spawn_edge_status(
                            agent_id,
                            DirectionalThreadSpawnEdgeStatus::Closed,
                        )
                        .await
                {
                    warn!("failed to persist thread-spawn edge status for {agent_id}: {err}");
                }
            }
            Err(OdyErr::ThreadNotFound(_)) if known_agent => {
                if let Some(state_db_ctx) = state.state_db()
                    && let Err(err) = state_db_ctx
                        .set_thread_spawn_edge_status(
                            agent_id,
                            DirectionalThreadSpawnEdgeStatus::Closed,
                        )
                        .await
                {
                    return Err(OdyErr::Fatal(format!(
                        "failed to persist stale thread-spawn edge status for {agent_id}: {err}"
                    )));
                }
            }
            Err(OdyErr::ThreadNotFound(_)) => {}
            Err(err) => {
                warn!("failed to inspect agent before close {agent_id}: {err}");
            }
        }
        match Box::pin(self.shutdown_agent_tree(agent_id)).await {
            Err(OdyErr::ThreadNotFound(_)) | Err(OdyErr::InternalAgentDied) if known_agent => {
                Ok(String::new())
            }
            result => result,
        }
    }

    /// Shut down `agent_id` and any live descendants reachable from the in-memory spawn tree.
    pub(crate) async fn shutdown_agent_tree(&self, agent_id: ThreadId) -> OdyResult<String> {
        let descendant_ids = self.live_thread_spawn_descendants(agent_id).await?;
        let result = self.shutdown_live_agent(agent_id).await;
        for descendant_id in descendant_ids {
            match self.shutdown_live_agent(descendant_id).await {
                Ok(_) | Err(OdyErr::ThreadNotFound(_)) | Err(OdyErr::InternalAgentDied) => {}
                Err(err) => return Err(err),
            }
        }
        result
    }
}
