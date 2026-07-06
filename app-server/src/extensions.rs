use std::sync::Arc;
use std::sync::Weak;

use ody_analytics::AnalyticsEventsClient;
use ody_app_server_protocol::ServerNotification;
use ody_app_server_protocol::ThreadGoal;
use ody_app_server_protocol::ThreadGoalUpdatedNotification;
use ody_core::NewThread;
use ody_core::StartThreadOptions;
use ody_core::ThreadManager;
use ody_core::config::Config;
use ody_exec_server::EnvironmentManager;
use ody_extension_api::AgentSpawnFuture;
use ody_extension_api::AgentSpawner;
use ody_extension_api::ExtensionEventSink;
use ody_extension_api::ExtensionRegistry;
use ody_extension_api::ExtensionRegistryBuilder;
use ody_goal_extension::GoalService;
use ody_login::AuthManager;
use ody_protocol::ThreadId;
use ody_protocol::error::OdyErr;
use ody_protocol::protocol::Event;
use ody_protocol::protocol::EventMsg;
use ody_rollout::state_db::StateDbHandle;
use ody_thread_store::ThreadStore;

use crate::outgoing_message::OutgoingMessageSender;
use crate::thread_state::ThreadListenerCommand;
use crate::thread_state::ThreadStateManager;

pub(crate) struct ThreadExtensionDependencies {
    pub(crate) event_sink: Arc<dyn ExtensionEventSink>,
    pub(crate) auth_manager: Arc<AuthManager>,
    pub(crate) state_db: Option<StateDbHandle>,
    pub(crate) analytics_events_client: AnalyticsEventsClient,
    pub(crate) thread_manager: Weak<ThreadManager>,
    pub(crate) goal_service: Arc<GoalService>,
    pub(crate) environment_manager: Arc<EnvironmentManager>,
    pub(crate) executor_skill_provider: Arc<dyn ody_skills_extension::SkillProvider>,
    /// Process-scoped persistence backend for extensions that need stored thread history.
    pub(crate) thread_store: Arc<dyn ThreadStore>,
}

pub(crate) fn thread_extensions<S>(
    guardian_agent_spawner: S,
    dependencies: ThreadExtensionDependencies,
) -> Arc<ExtensionRegistry<Config>>
where
    S: AgentSpawner<StartThreadOptions, Spawned = NewThread, Error = OdyErr> + 'static,
{
    let ThreadExtensionDependencies {
        event_sink,
        auth_manager,
        state_db,
        analytics_events_client,
        thread_manager,
        goal_service,
        environment_manager,
        executor_skill_provider,
        thread_store: _thread_store,
    } = dependencies;
    let mut builder = ExtensionRegistryBuilder::<Config>::with_event_sink(event_sink);
    if let Some(state_db) = state_db {
        ody_goal_extension::install_with_backend(
            &mut builder,
            state_db,
            analytics_events_client,
            ody_otel::global(),
            thread_manager,
            goal_service,
            |config: &Config| config.features.enabled(ody_features::Feature::Goals),
        );
    }
    ody_guardian::install(&mut builder, guardian_agent_spawner);
    ody_memories_extension::install(&mut builder, ody_otel::global());
    ody_mcp_extension::install(&mut builder);
    ody_mcp_extension::install_executor_plugins(&mut builder, environment_manager);
    ody_web_search_extension::install(&mut builder, auth_manager);
    let skill_providers = ody_skills_extension::SkillProviders::new()
        .with_executor_provider(executor_skill_provider)
        .with_orchestrator_provider(Arc::new(
            ody_skills_extension::OrchestratorSkillProvider::new(),
        ));
    ody_skills_extension::install_with_providers(
        &mut builder,
        skill_providers,
        |config: &Config| ody_skills_extension::SkillsExtensionConfig {
            include_instructions: config.include_skill_instructions,
            bundled_skills_enabled: config.bundled_skills_enabled(),
            orchestrator_skills_enabled: config.orchestrator_skills_enabled,
            knowledge_microagents_enabled: config.knowledge_microagents_enabled,
            knowledge_max_skills_per_turn: config.knowledge_max_skills_per_turn,
            knowledge_max_contents_bytes: config.knowledge_max_contents_bytes,
            host_model_tools_enabled: config.host_model_tools_enabled,
            executor_model_tools_enabled: config.executor_model_tools_enabled,
        },
    );
    Arc::new(builder.build())
}

pub(crate) fn app_server_extension_event_sink(
    outgoing: Arc<OutgoingMessageSender>,
    thread_state_manager: ThreadStateManager,
) -> Arc<dyn ExtensionEventSink> {
    Arc::new(AppServerExtensionEventSink {
        outgoing,
        thread_state_manager,
    })
}

struct AppServerExtensionEventSink {
    outgoing: Arc<OutgoingMessageSender>,
    thread_state_manager: ThreadStateManager,
}

impl ExtensionEventSink for AppServerExtensionEventSink {
    fn emit(&self, event: Event) {
        match event.msg {
            EventMsg::ThreadGoalUpdated(thread_goal_event) => {
                let thread_id = thread_goal_event.thread_id;
                let turn_id = thread_goal_event.turn_id;
                let goal: ThreadGoal = thread_goal_event.goal.into();
                if let Some(listener_command_tx) = self
                    .thread_state_manager
                    .current_listener_command_tx(thread_id)
                {
                    let command = ThreadListenerCommand::EmitThreadGoalUpdated {
                        turn_id: turn_id.clone(),
                        goal: goal.clone(),
                    };
                    if listener_command_tx.send(command).is_ok() {
                        return;
                    }
                    tracing::warn!(
                        "failed to enqueue extension goal update for {thread_id}: listener command channel is closed"
                    );
                }
                let outgoing = Arc::clone(&self.outgoing);
                tokio::spawn(async move {
                    outgoing
                        .send_server_notification(ServerNotification::ThreadGoalUpdated(
                            ThreadGoalUpdatedNotification {
                                thread_id: thread_id.to_string(),
                                turn_id,
                                goal,
                            },
                        ))
                        .await;
                });
            }
            EventMsg::SkillLoaded(_)
            | EventMsg::SkillActivated(_)
            | EventMsg::SkillLoadError(_) => {
                tracing::debug!(event_id = %event.id, ?event.msg, "dropping unsupported extension event");
            }
            msg => {
                tracing::debug!(event_id = %event.id, ?msg, "dropping unsupported extension event");
            }
        }
    }
}

pub(crate) fn guardian_agent_spawner(
    thread_manager: Weak<ThreadManager>,
) -> impl AgentSpawner<StartThreadOptions, Spawned = NewThread, Error = OdyErr> {
    move |forked_from_thread_id: ThreadId,
          options: StartThreadOptions|
          -> AgentSpawnFuture<'static, NewThread, OdyErr> {
        let thread_manager = thread_manager.clone();
        Box::pin(async move {
            let thread_manager = thread_manager.upgrade().ok_or_else(|| {
                OdyErr::UnsupportedOperation("thread manager dropped".to_string())
            })?;
            thread_manager
                .spawn_subagent(forked_from_thread_id, options)
                .await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ody_protocol::protocol::ThreadGoal as CoreThreadGoal;
    use ody_protocol::protocol::ThreadGoalStatus;
    use ody_protocol::protocol::ThreadGoalUpdatedEvent;
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    use super::*;

    #[tokio::test]
    async fn app_server_event_sink_uses_listener_fifo_for_goal_updates_and_clears() {
        let (outgoing_tx, _outgoing_rx) = mpsc::channel(4);
        let outgoing = Arc::new(OutgoingMessageSender::new(
            outgoing_tx,
            AnalyticsEventsClient::disabled(),
        ));
        let thread_state_manager = ThreadStateManager::new();
        let thread_id = ThreadId::default();
        let (listener_command_tx, mut listener_command_rx) = mpsc::unbounded_channel();
        thread_state_manager.register_listener_command_tx(thread_id, listener_command_tx.clone());
        let sink = app_server_extension_event_sink(outgoing, thread_state_manager);

        for turn_id in ["turn-1", "turn-2"] {
            sink.emit(thread_goal_updated_event(thread_id, turn_id));
        }
        listener_command_tx
            .send(ThreadListenerCommand::EmitThreadGoalCleared)
            .expect("listener command channel should be open");

        let mut observed = Vec::new();
        for _ in 0..3 {
            let command = timeout(Duration::from_secs(1), listener_command_rx.recv())
                .await
                .expect("timed out waiting for listener command")
                .expect("listener command channel closed unexpectedly");
            match command {
                ThreadListenerCommand::EmitThreadGoalUpdated { turn_id, .. } => {
                    observed.push(turn_id.expect("extension goal updates should include turn ids"));
                }
                ThreadListenerCommand::EmitThreadGoalCleared => {
                    observed.push("cleared".to_string())
                }
                _ => panic!("unexpected listener command"),
            }
        }

        assert_eq!(
            vec![
                "turn-1".to_string(),
                "turn-2".to_string(),
                "cleared".to_string()
            ],
            observed
        );
    }

    fn thread_goal_updated_event(thread_id: ThreadId, turn_id: &str) -> Event {
        Event {
            id: turn_id.to_string(),
            msg: EventMsg::ThreadGoalUpdated(ThreadGoalUpdatedEvent {
                thread_id,
                turn_id: Some(turn_id.to_string()),
                goal: CoreThreadGoal {
                    thread_id,
                    objective: "wire extension events".to_string(),
                    status: ThreadGoalStatus::Active,
                    token_budget: Some(123),
                    tokens_used: 45,
                    time_used_seconds: 6,
                    created_at: 7,
                    updated_at: 8,
                },
            }),
        }
    }
}
