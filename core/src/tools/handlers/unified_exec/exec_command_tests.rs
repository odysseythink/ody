use super::*;
use crate::safety::PLAN_MODE_REJECTION_MARKER;
use crate::session::tests::make_session_and_context_with_rx;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::turn_diff_tracker::TurnDiffTracker;
use ody_config::config_toml::PlanEnforcement;
use ody_config::config_toml::PlanModeConfigToml;
use ody_protocol::config_types::ModeKind;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::WarningEvent;
use ody_tools::ToolExecutor;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

async fn plan_mode_invocation_for_command(
    cmd: &str,
) -> (
    ToolInvocation,
    async_channel::Receiver<ody_protocol::protocol::Event>,
) {
    let (session, turn, rx) = make_session_and_context_with_rx().await;

    let mut turn = Arc::try_unwrap(turn).expect("turn Arc should be unique");
    turn.collaboration_mode.mode = ModeKind::Plan;

    let mut config = Arc::try_unwrap(turn.config).expect("config Arc should be unique");
    config.plan_mode = Some(PlanModeConfigToml {
        enforcement: Some(PlanEnforcement::Strict),
        ..Default::default()
    });
    turn.config = Arc::new(config);

    let turn = Arc::new(turn);

    let payload = ToolPayload::Function {
        arguments: json!({ "cmd": cmd }).to_string(),
    };
    let invocation = ToolInvocation {
        session,
        turn,
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        tracker: Arc::new(Mutex::new(TurnDiffTracker::new())),
        call_id: "call-exec-command".to_string(),
        tool_name: ody_tools::ToolName::plain("exec_command"),
        source: crate::tools::context::ToolCallSource::Direct,
        payload,
    };
    (invocation, rx)
}

#[tokio::test]
async fn plan_mode_exec_denial_uses_display_and_emits_warning() {
    let (invocation, rx) = plan_mode_invocation_for_command("rm -rf /").await;
    let handler = ExecCommandHandler::default();

    let result = handler.handle(invocation).await;
    let msg = match result {
        Err(FunctionCallError::RespondToModel(msg)) => msg,
        Err(other) => panic!("expected RespondToModel error, got {other}"),
        Ok(_) => panic!("expected error, got success"),
    };

    assert!(
        !msg.contains("ProcessFailed {"),
        "error should not expose Debug representation: {msg}"
    );
    assert!(
        msg.contains(PLAN_MODE_REJECTION_MARKER),
        "denial message should contain marker: {msg}"
    );
    assert!(
        msg.contains("rm -rf /"),
        "denial message should include command: {msg}"
    );

    loop {
        let event = rx.recv().await.expect("expected event");
        match event.msg {
            EventMsg::Warning(WarningEvent { message })
                if message.contains(PLAN_MODE_REJECTION_MARKER) =>
            {
                break;
            }
            _ => continue,
        }
    }
}
