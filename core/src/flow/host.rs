//! Session-backed [`FlowAgentHost`] mapping (M1.2): each flow `agent()`
//! call spawns a real sub-agent through `crate::agent::control` and waits
//! for its final status, reusing the multi_agents spawn/wait primitives
//! (depth limits, status subscription, spawn visibility events) without
//! going through the ToolExecutor layer (parent report decision #3).
//!
//! Cancellation safety: `run_agent` installs an [`AbortOnDrop`] guard after
//! spawn; when the kernel short-circuits a batch and drops the future, the
//! guard interrupts the still-running sub-agent on the runtime (the drop
//! path itself never awaits — see R6 in the M1 execution plan).

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use ody_protocol::ThreadId;
use ody_protocol::model_metadata::ReasoningEffort;
use ody_protocol::protocol::AgentStatus;
use ody_protocol::protocol::CollabAgentSpawnBeginEvent;
use ody_protocol::protocol::CollabAgentSpawnEndEvent;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::FlowPhaseBeginEvent;
use ody_protocol::protocol::FlowPhaseEndEvent;
use ody_protocol::protocol::FlowStepCompletedEvent;
use ody_protocol::protocol::Op;
use ody_protocol::user_input::UserInput;
use tokio::runtime::Handle;

use crate::TurnContext;
use crate::agent::control::AgentControl;
use crate::agent::control::SpawnAgentOptions;
use crate::agent::exceeds_thread_spawn_depth_limit;
use crate::agent::next_thread_spawn_depth;
use crate::agent::status::is_final;
use crate::session::session::Session;
use crate::tools::handlers::multi_agents_common::build_agent_spawn_config;
use crate::tools::handlers::multi_agents_common::thread_spawn_source;
use crate::turn_timing::now_unix_timestamp_ms;

use super::FlowAgentHost;
use super::FlowHostError;
use super::FlowProgress;

/// Global counter for flow run / per-agent spawn call ids. Flow runs share
/// the Collab spawn event rendering with multi_agents, so call ids must be
/// unique per session event stream.
static FLOW_CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// [`FlowAgentHost`] backed by a live session: `run_agent` spawns a real
/// sub-agent thread (via `AgentControl`) and blocks until it reaches a
/// final status. Holds the session + turn so every flow sub-agent inherits
/// the current model/runtime/environments, mirroring the multi_agents
/// `spawn_agent` handler.
#[derive(Clone)]
pub(crate) struct SessionFlowAgentHost {
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    /// Flow skill name; carried for spawn-event metadata and progress
    /// reporting (M1.4 uses `run_call_id` as the progress event call id).
    flow_name: String,
    run_call_id: String,
    /// Disk checkpoint store for this run (M2.1); `None` means no caching.
    checkpoint_store: Option<Arc<crate::flow::CheckpointStore>>,
}

impl SessionFlowAgentHost {
    pub(crate) fn new(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        flow_name: impl Into<String>,
    ) -> Self {
        let run_id = FLOW_CALL_COUNTER.fetch_add(1, Ordering::Relaxed);
        Self {
            session,
            turn,
            flow_name: flow_name.into(),
            run_call_id: format!("flow-run-{run_id}"),
            checkpoint_store: None,
        }
    }

    /// Attach a checkpoint store for this run (M2.1). Call before executing.
    pub(crate) fn with_checkpoint_store(
        mut self,
        store: crate::flow::CheckpointStore,
    ) -> Self {
        self.checkpoint_store = Some(Arc::new(store));
        self
    }

    /// Delete the run's checkpoint file after a fully successful run.
    pub(crate) async fn discard_checkpoint(&self) {
        if let Some(store) = &self.checkpoint_store {
            store.discard().await;
        }
    }

    /// Run id for this flow execution; M1.4 progress events use it as the
    /// `call_id` so all phases/steps of one run share one event identity.
    pub(crate) fn run_call_id(&self) -> &str {
        &self.run_call_id
    }

    fn next_agent_call_id(&self) -> String {
        let agent_seq = FLOW_CALL_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{}-agent-{agent_seq}", self.run_call_id)
    }

    /// Spawn one flow sub-agent with the rendered prompt. Returns the child
    /// thread id; the caller then [`wait_agent`]s on it. Split from
    /// `run_agent` so integration tests can drive the child to completion
    /// between spawn and wait (A1).
    pub(crate) async fn spawn_agent(&self, prompt: String) -> Result<ThreadId, FlowHostError> {
        let call_id = self.next_agent_call_id();
        let parent_session_source = self.turn.session_source.clone();
        let child_depth = next_thread_spawn_depth(&parent_session_source);
        let max_depth = self.turn.config.agent_max_depth;
        if exceeds_thread_spawn_depth_limit(child_depth, max_depth) {
            return Err(FlowHostError(format!(
                "agent depth limit reached (depth {child_depth} exceeds max {max_depth})"
            )));
        }
        self.session
            .send_event(
                &self.turn,
                CollabAgentSpawnBeginEvent {
                    call_id: call_id.clone(),
                    started_at_ms: now_unix_timestamp_ms(),
                    sender_thread_id: self.session.thread_id,
                    prompt: prompt.clone(),
                    model: String::new(),
                    reasoning_effort: ReasoningEffort::default(),
                }
                .into(),
            )
            .await;

        let config = build_agent_spawn_config(
            &self.session.get_base_instructions().await,
            self.turn.as_ref(),
        )
        .map_err(|err| FlowHostError(err.to_string()))?;
        let session_source = thread_spawn_source(
            self.session.thread_id,
            &parent_session_source,
            child_depth,
            /*agent_role*/ None,
            /*task_name*/ None,
        )
        .map_err(|err| FlowHostError(err.to_string()))?;
        let result = self
            .session
            .services
            .agent_control
            .spawn_agent_with_metadata(
                config,
                vec![UserInput::Text {
                    text: prompt.clone(),
                    text_elements: Vec::new(),
                }]
                .into(),
                Some(session_source),
                SpawnAgentOptions {
                    fork_parent_spawn_call_id: None,
                    fork_mode: None,
                    parent_thread_id: Some(self.session.thread_id),
                    environments: Some(self.turn.environments.to_selections()),
                },
            )
            .await;

        let (new_thread_id, status) = match &result {
            Ok(spawned_agent) => (Some(spawned_agent.thread_id), spawned_agent.status.clone()),
            Err(_) => (None, AgentStatus::NotFound),
        };
        let snapshot = match new_thread_id {
            Some(thread_id) => {
                self.session
                    .services
                    .agent_control
                    .get_agent_config_snapshot(thread_id)
                    .await
            }
            None => None,
        };
        let model = snapshot
            .as_ref()
            .map(|snapshot| snapshot.model.clone())
            .unwrap_or_default();
        let reasoning_effort = snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.reasoning_effort.clone())
            .unwrap_or_default();
        self.session
            .send_event(
                &self.turn,
                CollabAgentSpawnEndEvent {
                    call_id,
                    completed_at_ms: now_unix_timestamp_ms(),
                    sender_thread_id: self.session.thread_id,
                    new_thread_id,
                    new_agent_nickname: None,
                    new_agent_role: Some(self.flow_name.clone()),
                    prompt,
                    model,
                    reasoning_effort,
                    status,
                }
                .into(),
            )
            .await;

        result
            .map(|spawned_agent| spawned_agent.thread_id)
            .map_err(|err| FlowHostError(format!("flow sub-agent spawn failed: {err}")))
    }

    /// Wait for a spawned flow sub-agent to reach a final status and return
    /// its final message. Non-completed finals map to [`FlowHostError`].
    pub(crate) async fn wait_agent(&self, thread_id: ThreadId) -> Result<String, FlowHostError> {
        let status = wait_for_final_status(&self.session.services.agent_control, thread_id).await;
        match status {
            AgentStatus::Completed(Some(message)) => Ok(message),
            AgentStatus::Completed(None) => Ok(String::new()),
            AgentStatus::Errored(message) => Err(FlowHostError(message)),
            AgentStatus::Shutdown => {
                Err(FlowHostError("flow sub-agent shut down before completing".to_string()))
            }
            AgentStatus::NotFound => Err(FlowHostError("flow sub-agent not found".to_string())),
            AgentStatus::PendingInit | AgentStatus::Running | AgentStatus::Interrupted => {
                Err(FlowHostError(format!("flow sub-agent ended in non-final status {status:?}")))
            }
        }
    }
}

impl FlowAgentHost for SessionFlowAgentHost {
    async fn run_agent(&self, prompt: String) -> Result<String, FlowHostError> {
        let thread_id = self.spawn_agent(prompt).await?;
        let _guard = AbortOnDrop::new(&self.session, thread_id);
        let result = self.wait_agent(thread_id).await;
        // Release the sub-agent's spawn slot once it reached a final status.
        // V1 AgentRegistry slots are only freed by shutdown/close; without
        // this every flow sub-agent would leak a slot until the session ends
        // and a long flow would eventually hit `agent_max_threads` (D1).
        // The lightweight release (no shutdown round-trip) keeps the flow's
        // critical path free of a wait on the child loop's termination.
        self.session
            .services
            .agent_control
            .release_finished_agent(thread_id)
            .await;
        result
    }

    fn report_progress(
        &self,
        progress: FlowProgress,
    ) -> impl std::future::Future<Output = ()> + Send {
        // RPITIT override (not a plain `async fn`): the returned future must
        // stay `Send` like `run_agent`'s (R4 in the M1 execution plan).
        let session = self.session.clone();
        let turn = self.turn.clone();
        let call_id = self.run_call_id.clone();
        let flow_name = self.flow_name.clone();
        async move {
            let event: EventMsg = match progress {
                FlowProgress::PhaseBegin { phase_id, total_steps } => FlowPhaseBeginEvent {
                    call_id,
                    flow_name,
                    phase_id,
                    total_steps,
                    started_at_ms: now_unix_timestamp_ms(),
                }
                .into(),
                FlowProgress::StepCompleted { phase_id, step_index, total_steps } => {
                    FlowStepCompletedEvent {
                        call_id,
                        flow_name,
                        phase_id,
                        step_index,
                        total_steps,
                        completed_at_ms: now_unix_timestamp_ms(),
                    }
                    .into()
                }
                FlowProgress::PhaseEnd { phase_id, completed_steps, total_steps } => {
                    FlowPhaseEndEvent {
                        call_id,
                        flow_name,
                        phase_id,
                        completed_steps,
                        total_steps,
                        completed_at_ms: now_unix_timestamp_ms(),
                    }
                    .into()
                }
            };
            session.send_event(&turn, event).await;
        }
    }

    fn checkpoint_read(
        &self,
        prompt: &str,
    ) -> impl std::future::Future<Output = Option<String>> + Send {
        let store = self.checkpoint_store.clone();
        let prompt = prompt.to_string();
        async move {
            match store {
                Some(store) => store.lookup(&prompt).await,
                None => None,
            }
        }
    }

    fn checkpoint_write(
        &self,
        prompt: &str,
        output: &str,
    ) -> impl std::future::Future<Output = ()> + Send {
        let store = self.checkpoint_store.clone();
        let prompt = prompt.to_string();
        let output = output.to_string();
        async move {
            if let Some(store) = store {
                store.record(&prompt, &output).await;
            }
        }
    }
}

/// Wait until `thread_id` reaches a final status, mirroring the multi_agents
/// `wait_agent` subscription pattern (`subscribe_status` + watch loop with a
/// `get_status` fallback when the sender drops).
async fn wait_for_final_status(agent_control: &AgentControl, thread_id: ThreadId) -> AgentStatus {
    let mut status_rx = match agent_control.subscribe_status(thread_id).await {
        Ok(status_rx) => status_rx,
        Err(_) => return AgentStatus::NotFound,
    };
    let mut status = status_rx.borrow().clone();
    loop {
        if is_final(&status) {
            return status;
        }
        if status_rx.changed().await.is_err() {
            return agent_control.get_status(thread_id).await;
        }
        status = status_rx.borrow().clone();
    }
}

/// Interrupts the flow sub-agent if its `run_agent` future is dropped before
/// completion (batch short-circuit, turn abort). The `Drop` impl only
/// enqueues a task on the session runtime — it never awaits (R6).
pub(crate) struct AbortOnDrop {
    agent_control: AgentControl,
    runtime: Handle,
    thread_id: ThreadId,
}

impl AbortOnDrop {
    pub(crate) fn new(session: &Session, thread_id: ThreadId) -> Self {
        Self {
            agent_control: session.services.agent_control.clone(),
            runtime: session.services.runtime_handle.clone(),
            thread_id,
        }
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        let agent_control = self.agent_control.clone();
        let thread_id = self.thread_id;
        self.runtime.spawn(async move {
            // Release the spawn slot on every path (D1): even when the
            // sub-agent already reached a final status, V1 registry slots are
            // only freed by shutdown/close — returning early here would leak
            // the slot and starve an immediate re-trigger (resume) of
            // `agent_max_threads` capacity.
            if !is_final(&agent_control.get_status(thread_id).await) {
                let _ = agent_control.interrupt_agent(thread_id).await;
            }
            // Lightweight release (no shutdown round-trip): the interrupted
            // sub-agent's loop is shutting down anyway, and waiting on its
            // termination here would delay the slot release that an
            // immediate re-trigger (resume) depends on (D1).
            agent_control.release_finished_agent(thread_id).await;
        });
    }
}
