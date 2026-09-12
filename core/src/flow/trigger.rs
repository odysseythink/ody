//! M1.3 trigger wiring: explicitly mentioned `SkillType::Flow` skills execute
//! in-turn through the M1.1 kernel instead of producing a `SkillInstructions`
//! injection. Results are recorded as [`FlowResultMessage`] conversation
//! items (A4/A6/A7 in the M1 execution plan).

use std::sync::Arc;

use ody_core_skills::HostSkillsSnapshot;
use ody_core_skills::SkillMetadata;
use ody_core_skills::SkillType;
use ody_exec_server::LOCAL_FS;
use ody_protocol::models::ResponseItem;
use ody_protocol::protocol::AskForApproval;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::ReviewDecision;
use ody_protocol::protocol::WarningEvent;
use ody_protocol::user_input::UserInput;
use ody_utils_path_uri::PathUri;

use crate::TurnContext;
use crate::context::ContextualUserFragment;
use crate::context::FlowResultMessage;
use crate::guardian::GuardianApprovalRequest;
use crate::guardian::new_guardian_review_id;
use crate::guardian::review_approval_request;
use crate::session::session::Session;

use super::FlowContext;
use super::FlowError;
use super::FlowOutcome;
use super::FlowPlanSummary;
use super::FlowRuntime;
use super::SessionFlowAgentHost;
use super::YamlFlowRuntime;

/// Split explicitly mentioned skills into `(flow, injectable)`, preserving
/// mention order within both lists. Only non-flow skills continue down the
/// `SkillInstructions` injection chain (R2).
pub(crate) fn partition_flow_skills(
    mentioned: Vec<SkillMetadata>,
) -> (Vec<SkillMetadata>, Vec<SkillMetadata>) {
    mentioned
        .into_iter()
        .partition(|skill| skill.skill_type == SkillType::Flow)
}

/// Derive Flow trigger arguments from the submission (A6): all text items of
/// the triggering input joined in order; a parseable JSON object is used
/// as-is, anything else becomes `{ "text": <joined> }`.
pub(crate) fn flow_args_from_input(inputs: &[UserInput]) -> serde_json::Map<String, serde_json::Value> {
    let joined = inputs
        .iter()
        .filter_map(|input| match input {
            UserInput::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    match serde_json::from_str::<serde_json::Value>(joined.trim()) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::from_iter([("text".to_string(), serde_json::Value::String(joined))]),
    }
}

/// Read the `flow.yaml` artifact of a flow skill through the filesystem that
/// discovered it (falls back to the local FS).
pub(crate) async fn read_flow_source(
    snapshot: &HostSkillsSnapshot,
    skill: &SkillMetadata,
) -> Result<String, FlowError> {
    let Some(artifact) = skill.flow_artifact.clone() else {
        return Err(FlowError::Parse {
            reason: format!("flow skill '{}' has no flow.yaml artifact", skill.name),
        });
    };
    let fs = snapshot
        .outcome()
        .file_system_for_skill(skill)
        .unwrap_or_else(|| Arc::clone(&LOCAL_FS));
    let path = PathUri::from_abs_path(&artifact);
    fs.read_file_text(&path, /*sandbox*/ None)
        .await
        .map_err(|err| FlowError::Parse {
            reason: format!("failed to read flow.yaml for '{}': {err}", skill.name),
        })
}

/// M2.2: one-shot guardian approval before a flow run. `approval_policy =
/// "never"` bypasses the review (YOLO), mirroring extension_tools.rs:70-77.
/// Denied / TimedOut / Abort and policy amendments all fail the run with
/// [`FlowError::Denied`], recorded as a flow_result failure item by the
/// caller.
async fn ensure_flow_run_approved(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    flow_name: &str,
    plan: &ody_core_skills::FlowPlan,
) -> Result<(), FlowError> {
    if turn_context.approval_policy.value() == AskForApproval::Never {
        return Ok(());
    }
    let review_id = new_guardian_review_id();
    let decision = review_approval_request(
        sess,
        turn_context,
        review_id.clone(),
        GuardianApprovalRequest::FlowRun {
            id: review_id,
            turn_id: turn_context.sub_id.clone(),
            flow_name: flow_name.to_string(),
            summary: FlowPlanSummary::new(flow_name, plan),
        },
        /*retry_reason*/ None,
    )
    .await;
    match decision {
        ReviewDecision::Approved
        | ReviewDecision::ApprovedExecpolicyAmendment { .. }
        | ReviewDecision::ApprovedForSession => Ok(()),
        ReviewDecision::Denied
        | ReviewDecision::TimedOut
        | ReviewDecision::Abort
        | ReviewDecision::NetworkPolicyAmendment { .. } => Err(FlowError::Denied {
            reason: format!("flow '{flow_name}' not approved by guardian ({decision:?})"),
        }),
    }
}

/// Read, validate, and execute one flow skill with checkpointing (M2.1) and
/// run-before guardian approval (M2.2).
pub(crate) async fn run_one_flow_skill(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    skill: &SkillMetadata,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<FlowOutcome, FlowError> {
    let name = skill.name.clone();
    let source = read_flow_source(&turn_context.turn_skills.snapshot, skill).await?;
    let runtime = YamlFlowRuntime::new();
    let plan = runtime.validate(&source)?;
    ensure_flow_run_approved(sess, turn_context, &name, &plan).await?;
    let fingerprint = crate::flow::plan_fingerprint(&source);
    let ody_home = sess.ody_home().await;
    let host = SessionFlowAgentHost::new(Arc::clone(sess), Arc::clone(turn_context), name.clone())
        .with_checkpoint_store(
            crate::flow::CheckpointStore::open(&ody_home, &name, &fingerprint).await,
        );
    let result = runtime.run(plan, FlowContext { args: args.clone() }, &host).await;
    if result.is_ok() {
        host.discard_checkpoint().await;
    }
    result
}

/// Execute every mentioned flow skill serially in mention order (A6) and
/// return one result conversation item per skill. Failures are recorded as
/// items too and surfaced through a `WarningEvent`; telemetry is emitted per
/// run regardless of outcome (A7).
pub(crate) async fn run_flow_skills_in_turn(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    skills: &[SkillMetadata],
    args: serde_json::Map<String, serde_json::Value>,
) -> Vec<ResponseItem> {
    let mut items = Vec::with_capacity(skills.len());
    for skill in skills {
        let name = skill.name.clone();
        let result = run_one_flow_skill(sess, turn_context, skill, &args).await;
        turn_context.session_telemetry.counter(
            "ody.flow.run",
            /*inc*/ 1,
            &[
                ("skill", name.as_str()),
                ("status", if result.is_ok() { "ok" } else { "error" }),
            ],
        );
        match result {
            Ok(outcome) => {
                items.push(ContextualUserFragment::into(FlowResultMessage::success(
                    &name,
                    &outcome.outputs,
                )));
            }
            Err(err) => {
                sess.send_event(
                    turn_context,
                    EventMsg::Warning(WarningEvent { message: format!("Flow '{name}' failed: {err}") }),
                )
                .await;
                items.push(ContextualUserFragment::into(FlowResultMessage::failure(&name, &err)));
            }
        }
    }
    items
}
