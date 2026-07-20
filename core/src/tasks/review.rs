use std::sync::Arc;

use ody_prompts::render_review_exit_interrupted;
use ody_prompts::render_review_exit_success;
use ody_protocol::config_types::WebSearchMode;
use ody_protocol::items::TurnItem;
use ody_protocol::model_metadata::ReasoningEffort;
use ody_protocol::models::ContentItem;
use ody_protocol::models::ResponseItem;
use ody_protocol::protocol::AgentMessageContentDeltaEvent;
use ody_protocol::protocol::AskForApproval;
use ody_protocol::protocol::Event;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::ExitedReviewModeEvent;
use ody_protocol::protocol::ItemCompletedEvent;
use ody_protocol::protocol::ReviewOutputEvent;
use ody_protocol::protocol::SubAgentSource;
use tokio_util::sync::CancellationToken;

use crate::config::Constrained;
use crate::ody_delegate::run_ody_thread_one_shot;
use crate::review_format::format_review_findings_block;
use crate::review_format::render_review_output_text;
use crate::session::TurnInput;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::state::TaskKind;
use ody_features::Feature;
use ody_protocol::user_input::UserInput;

use super::SessionTask;
use super::SessionTaskContext;
use super::SessionTaskResult;

/// Sub-agent source label for the adversarial design review. It is deliberately
/// distinct from [`SubAgentSource::Review`] (which the tool-using `/review` code
/// review keeps) so the tool planner can recognize the design review and expose
/// it zero model-visible tools — see `spec_plan::build_tool_specs_and_registry`.
pub(crate) const DESIGN_REVIEW_SUBAGENT_LABEL: &str = "design_review";

#[derive(Clone, Copy)]
pub(crate) struct ReviewTask;

impl ReviewTask {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl SessionTask for ReviewTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Review
    }

    fn span_name(&self) -> &'static str {
        "session_task.review"
    }

    async fn run(
        self: Arc<Self>,
        session: Arc<SessionTaskContext>,
        ctx: Arc<TurnContext>,
        input: Vec<TurnInput>,
        cancellation_token: CancellationToken,
    ) -> SessionTaskResult {
        session
            .session
            .services
            .session_telemetry
            .counter("ody.task.review", /*inc*/ 1, &[]);

        let mut user_input = Vec::new();
        for item in input {
            match item {
                TurnInput::UserInput { mut content, .. } => user_input.append(&mut content),
                TurnInput::ResponseItem(_) | TurnInput::InterAgentCommunication(_) => {}
            }
        }

        // Start sub-ody conversation and get the receiver for events.
        let output = match start_review_conversation(
            session.clone(),
            ctx.clone(),
            user_input,
            cancellation_token.clone(),
        )
        .await
        {
            Some(receiver) => process_review_events(session.clone(), ctx.clone(), receiver).await,
            None => None,
        };
        if !cancellation_token.is_cancelled() {
            exit_review_mode(session.clone_session(), output.clone(), ctx.clone()).await;
        }
        Ok(None)
    }

    async fn abort(&self, session: Arc<SessionTaskContext>, ctx: Arc<TurnContext>) {
        exit_review_mode(session.clone_session(), /*review_output*/ None, ctx).await;
    }
}

pub(crate) async fn run_one_shot_review(
    session: Arc<SessionTaskContext>,
    ctx: Arc<TurnContext>,
    input: Vec<UserInput>,
    cancellation_token: CancellationToken,
    base_instructions: String,
    model: String,
    reasoning_effort: Option<ReasoningEffort>,
) -> Option<ReviewOutputEvent> {
    let receiver = start_review_conversation_with_overrides(
        session,
        ctx,
        input,
        cancellation_token.clone(),
        Some(base_instructions),
        Some(model),
        reasoning_effort,
        // The adversarial design review is a pure critique over the design
        // document (which is already in the prompt): give it NO exploration
        // tools. See the disable block below for why.
        /*disable_exploration_tools*/ true,
    )
    .await?;
    process_one_shot_review_events(receiver, cancellation_token).await
}

async fn start_review_conversation(
    session: Arc<SessionTaskContext>,
    ctx: Arc<TurnContext>,
    input: Vec<UserInput>,
    cancellation_token: CancellationToken,
) -> Option<async_channel::Receiver<Event>> {
    start_review_conversation_with_overrides(
        session,
        ctx,
        input,
        cancellation_token,
        None,
        None,
        None,
        // The `/review` code review legitimately explores the codebase.
        /*disable_exploration_tools*/ false,
    )
    .await
}

async fn start_review_conversation_with_overrides(
    session: Arc<SessionTaskContext>,
    ctx: Arc<TurnContext>,
    input: Vec<UserInput>,
    cancellation_token: CancellationToken,
    base_instructions_override: Option<String>,
    model_override: Option<String>,
    reasoning_effort_override: Option<ReasoningEffort>,
    disable_exploration_tools: bool,
) -> Option<async_channel::Receiver<Event>> {
    let config = ctx.config.clone();
    let mut sub_agent_config = config.as_ref().clone();
    // Structured single-shot reviews (e.g. the design adversarial review) pass
    // `None` reasoning to suppress a long streamed thinking trace that only adds
    // latency; the model still reasons inside its JSON answer.
    if let Some(reasoning_effort) = reasoning_effort_override {
        sub_agent_config.model_reasoning_effort = Some(reasoning_effort);
    }
    // Carry over review-only feature restrictions so the delegate cannot
    // re-enable blocked tools (web search, collab tools, view image).
    if let Err(err) = sub_agent_config
        .web_search_mode
        .set(WebSearchMode::Disabled)
    {
        panic!("by construction Constrained<WebSearchMode> must always support Disabled: {err}");
    }
    let _ = sub_agent_config.features.disable(Feature::SpawnCsv);
    let _ = sub_agent_config.features.disable(Feature::Collab);
    let _ = sub_agent_config.features.disable(Feature::MultiAgentV2);

    // Set explicit review rubric for the sub-agent
    sub_agent_config.base_instructions =
        base_instructions_override.or(Some(crate::REVIEW_PROMPT.to_string()));
    // The rubric above *replaces* base_instructions, dropping the language
    // directive that config load bakes in — which is why review/adversarial
    // subagent output defaults to English. Re-append it so review findings
    // (guardian and the design adversarial review) honor the configured
    // language. `model_language` is carried on the cloned config.
    if let Some(language) = sub_agent_config.model_language.as_deref()
        && let Some(instructions) = sub_agent_config.base_instructions.as_mut()
    {
        instructions.push('\n');
        instructions.push('\n');
        instructions.push_str(&ody_config::locale::language_directive(language));
    }
    sub_agent_config.permissions.approval_policy = Constrained::allow_only(AskForApproval::Never);

    let model = model_override.unwrap_or_else(|| {
        config
            .review_model
            .clone()
            .unwrap_or_else(|| ctx.model_info.slug.clone())
    });
    // A review model given as a namespaced slug `"<provider_id>/<model>"` (e.g.
    // `design_review_model = "glm_1/glm-5.1"`) needs two things fixed up, because
    // `sub_agent_config` is a clone of the *parent* config:
    //
    // 1. Provider. The clone still carries the parent's `model_provider` (e.g.
    //    `kimi`); overriding only `model` sends the request with the parent's
    //    vendor, silently skipping vendor-specific wire behavior — notably GLM's
    //    `thinking: { type: "disabled" }`. A GLM review then streamed a full
    //    thinking trace (~2x latency) and blew past the review timeout.
    // 2. Wire model name. `model_info.slug` is sent to the wire verbatim, and a
    //    namespaced slug is kept as-is (see `construct_model_info_from_candidates`),
    //    so GLM receives `glm_1/glm-5.1` and rejects it (error 1211 模型不存在).
    //    The provider wants its own model name (`glm-5.1`).
    //
    // The main agent never trips either because its `model` is already resolved
    // to the bare provider model name. Resolve both together, and only when the
    // provider is known — otherwise leave the slug untouched.
    let resolved = model.split_once('/').and_then(|(provider_id, suffix)| {
        let info = sub_agent_config.model_providers.get(provider_id).cloned()?;
        let wire_model = sub_agent_config
            .configured_models
            .get(&model)
            .and_then(|entry| entry.model.clone())
            .unwrap_or_else(|| suffix.to_string());
        Some((provider_id.to_string(), info, wire_model))
    });
    if let Some((provider_id, info, wire_model)) = resolved {
        sub_agent_config.model_provider_id = provider_id;
        sub_agent_config.model_provider = info;
        sub_agent_config.model = Some(wire_model);
    } else {
        sub_agent_config.model = Some(model);
    }
    // The adversarial DESIGN review must be a pure, single-shot critique over the
    // design document (already embedded in the prompt): it raises risks, and that
    // is all. Exploring the codebase and patching the design in response to a risk
    // is the (cheaper) main Design-mode model's job on revise. But the review
    // sub-agent is a clone of the parent, so without intervention it inherits the
    // parent's Design mode + environment and is handed submit_design, apply_patch,
    // update_plan, read_file, grep, exec, … — which the expensive reviewer then
    // uses to loop for multiple turns, roughly tripling latency and pushing it to
    // the edge of the review timeout. We tag it with a dedicated sub-agent source
    // so the tool planner (`spec_plan`) can hand it zero model-visible tools.
    let subagent_source = if disable_exploration_tools {
        SubAgentSource::Other(DESIGN_REVIEW_SUBAGENT_LABEL.to_string())
    } else {
        SubAgentSource::Review
    };
    (run_ody_thread_one_shot(
        sub_agent_config,
        session.models_manager(),
        input,
        session.clone_session(),
        ctx.clone(),
        cancellation_token,
        subagent_source,
        /*final_output_json_schema*/ None,
        /*initial_history*/ None,
    )
    .await)
        .ok()
        .map(|io| io.rx_event)
}

async fn process_review_events(
    session: Arc<SessionTaskContext>,
    ctx: Arc<TurnContext>,
    receiver: async_channel::Receiver<Event>,
) -> Option<ReviewOutputEvent> {
    let mut prev_agent_message: Option<Event> = None;
    while let Ok(event) = receiver.recv().await {
        match event.clone().msg {
            EventMsg::AgentMessage(_) => {
                if let Some(prev) = prev_agent_message.take() {
                    session
                        .clone_session()
                        .send_event(ctx.as_ref(), prev.msg)
                        .await;
                }
                prev_agent_message = Some(event);
            }
            // Suppress ItemCompleted only for assistant messages: forwarding it
            // would trigger legacy AgentMessage via as_legacy_events(), which this
            // review flow intentionally hides in favor of structured output.
            EventMsg::ItemCompleted(ItemCompletedEvent {
                item: TurnItem::AgentMessage(_),
                ..
            })
            | EventMsg::AgentMessageContentDelta(AgentMessageContentDeltaEvent { .. }) => {}
            EventMsg::TurnComplete(task_complete) => {
                // Parse review output from the last agent message (if present).
                let out = task_complete
                    .last_agent_message
                    .as_deref()
                    .map(parse_review_output_event);
                return out;
            }
            EventMsg::TurnAborted(_) => {
                // Cancellation or abort: consumer will finalize with None.
                return None;
            }
            other => {
                session
                    .clone_session()
                    .send_event(ctx.as_ref(), other)
                    .await;
            }
        }
    }
    // Channel closed without TurnComplete: treat as interrupted.
    None
}

async fn process_one_shot_review_events(
    receiver: async_channel::Receiver<Event>,
    cancellation_token: CancellationToken,
) -> Option<ReviewOutputEvent> {
    loop {
        tokio::select! {
            Ok(event) = receiver.recv() => {
                match event.msg {
                    EventMsg::TurnComplete(task_complete) => {
                        return task_complete
                            .last_agent_message
                            .as_deref()
                            .map(parse_review_output_event);
                    }
                    EventMsg::TurnAborted(_) => return None,
                    // Ignore other events: no parent forwarding for design review.
                    _ => {}
                }
            }
            _ = cancellation_token.cancelled() => return None,
        }
    }
}

/// Parse a ReviewOutputEvent from a text blob returned by the reviewer model.
/// If the text is valid JSON matching ReviewOutputEvent, deserialize it.
/// Otherwise, attempt to extract the first JSON object substring and parse it.
/// If parsing still fails, return a structured fallback carrying the plain text
/// in `overall_explanation`.
pub(crate) fn parse_review_output_event(text: &str) -> ReviewOutputEvent {
    if let Ok(ev) = serde_json::from_str::<ReviewOutputEvent>(text) {
        return ev;
    }
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}'))
        && start < end
        && let Some(slice) = text.get(start..=end)
        && let Ok(ev) = serde_json::from_str::<ReviewOutputEvent>(slice)
    {
        return ev;
    }
    ReviewOutputEvent {
        overall_explanation: text.to_string(),
        ..Default::default()
    }
}

/// Emits an ExitedReviewMode Event with optional ReviewOutput,
/// and records a developer message with the review output.
pub(crate) async fn exit_review_mode(
    session: Arc<Session>,
    review_output: Option<ReviewOutputEvent>,
    ctx: Arc<TurnContext>,
) {
    const REVIEW_USER_MESSAGE_ID: &str = "review_rollout_user";
    const REVIEW_ASSISTANT_MESSAGE_ID: &str = "review_rollout_assistant";
    let (user_message, assistant_message) = if let Some(out) = review_output.clone() {
        let mut findings_str = String::new();
        let text = out.overall_explanation.trim();
        if !text.is_empty() {
            findings_str.push_str(text);
        }
        if !out.findings.is_empty() {
            let block = format_review_findings_block(&out.findings, /*selection*/ None);
            findings_str.push_str(&format!("\n{block}"));
        }
        let rendered = render_review_exit_success(&findings_str);
        let assistant_message = render_review_output_text(&out);
        (rendered, assistant_message)
    } else {
        let rendered = render_review_exit_interrupted();
        let assistant_message =
            "Review was interrupted. Please re-run /review and wait for it to complete."
                .to_string();
        (rendered, assistant_message)
    };

    session
        .record_conversation_items(
            &ctx,
            &[ResponseItem::Message {
                id: Some(REVIEW_USER_MESSAGE_ID.to_string()),
                role: "user".to_string(),
                content: vec![ContentItem::InputText { text: user_message }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            }],
        )
        .await;

    session
        .send_event(
            ctx.as_ref(),
            EventMsg::ExitedReviewMode(ExitedReviewModeEvent { review_output }),
        )
        .await;
    session
        .record_response_item_and_emit_turn_item(
            ctx.as_ref(),
            ResponseItem::Message {
                id: Some(REVIEW_ASSISTANT_MESSAGE_ID.to_string()),
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: assistant_message,
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
        )
        .await;

    // Review turns can run before any regular user turn, so explicitly
    // materialize rollout persistence. Do this after emitting review output so
    // file creation + git metadata collection cannot delay client-facing items.
    session.ensure_rollout_materialized().await;
}
