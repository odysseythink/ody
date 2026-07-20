use super::*;
use ody_extension_api::ExtensionData;
use ody_extension_api::TurnItemContributor;
use ody_protocol::items::AgentMessageContent;
use pretty_assertions::assert_eq;
use std::sync::Arc;

struct RewriteAgentMessageContributor;

impl TurnItemContributor for RewriteAgentMessageContributor {
    fn contribute<'a>(
        &'a self,
        _thread_store: &'a ExtensionData,
        _turn_store: &'a ExtensionData,
        item: &'a mut TurnItem,
    ) -> ody_extension_api::ExtensionFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if let TurnItem::AgentMessage(agent_message) = item {
                agent_message.content = vec![AgentMessageContent::Text {
                    text: "plan contributed assistant text".to_string(),
                }];
            }
            Ok(())
        })
    }
}

fn assistant_output_text(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: Some("msg-1".to_string()),
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

#[tokio::test]
async fn plan_mode_uses_contributed_turn_item_for_last_agent_message() {
    let (mut session, turn_context) = crate::session::tests::make_session_and_context().await;
    let mut builder = ody_extension_api::ExtensionRegistryBuilder::new();
    builder.turn_item_contributor(Arc::new(RewriteAgentMessageContributor));
    session.services.extensions = Arc::new(builder.build());
    let turn_store = ExtensionData::new(turn_context.sub_id.clone());
    let mut state = PlanModeStreamState::new(&turn_context.sub_id);
    let mut last_agent_message = None;
    let item = assistant_output_text("original assistant text");

    let handled = handle_assistant_item_done_in_plan_mode(
        &session,
        &turn_context,
        &turn_store,
        &item,
        &mut state,
        /*previously_active_item*/ None,
        &mut last_agent_message,
    )
    .await;

    assert!(handled);
    assert_eq!(
        last_agent_message.as_deref(),
        Some("plan contributed assistant text")
    );
}

use crate::plan_artifact::PlanArtifact;
use crate::plan_mode_injector::ReminderKind;
use ody_protocol::ThreadId;
use ody_protocol::config_types::ModeKind;
use ody_utils_absolute_path::AbsolutePathBuf;

#[tokio::test]
async fn plan_mode_records_full_reminder_at_turn_five() {
    let (sess, tc, rx) = crate::session::tests::make_session_and_context_with_rx().await;
    let mut tc = tc;
    let tc_mut = Arc::get_mut(&mut tc).expect("turn context arc should be unique in test");
    tc_mut.collaboration_mode.mode = ModeKind::Plan;

    let tmp = tempfile::tempdir().unwrap();
    let plans_base_dir = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
    let thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001").unwrap();
    let artifact = PlanArtifact::new_temp(plans_base_dir, thread_id, "2026-07-04");
    artifact.finalize_name("topic").await.unwrap();
    tc_mut.plan_artifact = Some(Arc::new(artifact));

    let plan_markdown = "## Parts\n| # | File | Scope | Status |\n|---|---|---|---|\n| 1 | core.md | models | pending |\n";
    let mut client_session = crate::session::tests::test_model_client_session();

    for _ in 1..=5 {
        run_session_mode_after_turn(&sess, &tc, &mut client_session, plan_markdown)
            .await
            .expect("after-plan hook should succeed");
    }

    // Drain events and find the full reminder.
    let mut found_full = false;
    while let Ok(event) = rx.try_recv() {
        if let ody_protocol::protocol::EventMsg::RawResponseItem(raw) = event.msg {
            if let ody_protocol::models::ResponseItem::Message { content, .. } = raw.item {
                if content.iter().any(|c| matches!(c, ody_protocol::models::ContentItem::InputText { text } if text.contains("## Plan-mode rigor reminder (full)"))) {
                    found_full = true;
                }
            }
        }
    }
    assert!(
        found_full,
        "a full rigor reminder should be recorded by turn 5"
    );
}

#[tokio::test]
async fn design_turn_constructs_artifact_under_designs_dir() {
    let (sess, _tc, _rx) = crate::session::tests::make_session_and_context_with_rx().await;
    let mut collaboration_mode = sess.collaboration_mode().await;
    collaboration_mode.mode = ModeKind::Design;
    {
        let mut state = sess.state.lock().await;
        state.session_configuration.collaboration_mode = collaboration_mode;
    }

    let turn_context = sess.new_default_turn().await;
    let artifact = turn_context
        .plan_artifact
        .as_ref()
        .expect("design artifact should be constructed for Design turns");
    let path = artifact
        .path()
        .expect("design artifact should have a temp path");
    assert!(
        path.components().any(|c| c.as_os_str() == "designs"),
        "design artifact path should be rooted under a `designs/` dir: {path:?}"
    );
}

#[tokio::test]
async fn design_after_turn_injects_design_split_directive() {
    let (sess, tc, rx) = crate::session::tests::make_session_and_context_with_rx().await;
    let mut tc = tc;
    let tc_mut = Arc::get_mut(&mut tc).expect("turn context arc should be unique in test");
    tc_mut.collaboration_mode.mode = ModeKind::Design;

    let tmp = tempfile::tempdir().unwrap();
    let plans_base_dir = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
    let thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001").unwrap();
    let artifact = PlanArtifact::new_design(plans_base_dir, thread_id, "2026-07-04");
    artifact.finalize_name("topic").await.unwrap();
    tc_mut.plan_artifact = Some(Arc::new(artifact));

    let design_markdown = "## Parts\n| # | File | Scope | Status |\n|---|---|---|---|\n| 1 | core.md | data models | pending |\n";
    let mut client_session = crate::session::tests::test_model_client_session();

    run_session_mode_after_turn(&sess, &tc, &mut client_session, design_markdown)
        .await
        .expect("after-turn hook should succeed for design");

    let mut found_directive = false;
    while let Ok(event) = rx.try_recv() {
        if let ody_protocol::protocol::EventMsg::RawResponseItem(raw) = event.msg {
            if let ody_protocol::models::ResponseItem::Message { content, .. } = raw.item {
                if content.iter().any(|c| matches!(c, ody_protocol::models::ContentItem::InputText { text } if text.to_lowercase().contains("one part per turn") && text.contains("core.md"))) {
                    found_directive = true;
                }
            }
        }
    }
    assert!(
        found_directive,
        "design after-turn hook should record a design split directive mentioning one part per turn and core.md"
    );
}

fn checkpoint_plan(statuses: &[StepStatus]) -> Vec<PlanItemArg> {
    statuses
        .iter()
        .enumerate()
        .map(|(i, status)| PlanItemArg {
            step: format!("step {i}"),
            status: status.clone(),
        })
        .collect()
}

/// The whole point: a finished task in a large context is the cheap place to
/// compact, so the global limit does not get to pick a mid-task moment instead.
#[test]
fn task_checkpoint_fires_on_a_finished_task_in_a_large_context() {
    let plan = checkpoint_plan(&[StepStatus::Completed, StepStatus::InProgress]);
    assert!(task_checkpoint_due(
        0.5, &plan, /*crossed_boundary*/ true, /*context_window*/ 249_036,
        /*total_tokens*/ 154_587,
    ));
}

/// Compaction costs a round trip; a small context has nothing to gain.
#[test]
fn task_checkpoint_holds_below_the_ratio() {
    let plan = checkpoint_plan(&[StepStatus::Completed, StepStatus::InProgress]);
    assert!(!task_checkpoint_due(0.5, &plan, true, 249_036, 80_342));
}

/// Every other turn crosses no boundary; the checkpoint must stay quiet or it
/// would compact continuously.
#[test]
fn task_checkpoint_holds_without_a_finished_task() {
    let plan = checkpoint_plan(&[StepStatus::Completed, StepStatus::InProgress]);
    assert!(!task_checkpoint_due(0.5, &plan, false, 249_036, 200_000));
}

/// The final task leaves nothing to resume, so compacting there pays the cost
/// for a context about to go idle.
#[test]
fn task_checkpoint_holds_when_no_work_remains() {
    let plan = checkpoint_plan(&[StepStatus::Completed, StepStatus::Completed]);
    assert!(!task_checkpoint_due(0.5, &plan, true, 249_036, 200_000));
}

#[tokio::test]
async fn design_mode_records_reminder_during_early_clarification() {
    // Simulate the early clarification phase: the model has not written any
    // design yet (no submit_design), so the hook runs with empty markdown. The
    // Design reminder — above all the `request_user_input` pop-up rule — must
    // still be re-injected on cadence, since that phase is exactly where the
    // model drifts to plain-text questions.
    let (sess, tc, rx) = crate::session::tests::make_session_and_context_with_rx().await;
    let mut tc = tc;
    let tc_mut = Arc::get_mut(&mut tc).expect("turn context arc should be unique in test");
    tc_mut.collaboration_mode.mode = ModeKind::Design;

    let tmp = tempfile::tempdir().unwrap();
    let plans_base_dir = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
    let thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000005").unwrap();
    let artifact = PlanArtifact::new_design(plans_base_dir, thread_id, "2026-07-04");
    artifact.finalize_name("topic").await.unwrap();
    tc_mut.plan_artifact = Some(Arc::new(artifact));

    let mut client_session = crate::session::tests::test_model_client_session();

    // No design written yet — empty markdown, as the production call site passes
    // for Design turns before the first submit_design.
    for _ in 1..=5 {
        run_session_mode_after_turn(&sess, &tc, &mut client_session, "")
            .await
            .expect("design after-turn hook should succeed with no design written yet");
    }

    let mut found_reminder = false;
    while let Ok(event) = rx.try_recv() {
        if let ody_protocol::protocol::EventMsg::RawResponseItem(raw) = event.msg {
            if let ody_protocol::models::ResponseItem::Message { content, .. } = raw.item {
                if content.iter().any(|c| matches!(c, ody_protocol::models::ContentItem::InputText { text } if text.contains("request_user_input") && text.contains("Popups are mandatory"))) {
                    found_reminder = true;
                }
            }
        }
    }
    assert!(
        found_reminder,
        "a Design reminder restating the request_user_input pop-up rule should be recorded by turn 5"
    );
}
