use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use crate::apply_patch;
use crate::apply_patch::InternalApplyPatchInvocation;
use crate::apply_patch::convert_apply_patch_to_protocol;
use crate::function_tool::FunctionCallError;
use crate::safety::PLAN_MODE_REJECTION_MARKER;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::session::turn_context::TurnEnvironment;
use crate::tools::context::ApplyPatchToolOutput;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::SharedTurnDiffTracker;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::events::ToolEmitter;
use crate::tools::events::ToolEventCtx;
use crate::tools::handlers::apply_granted_turn_permissions;
use crate::tools::handlers::apply_patch_spec::create_apply_patch_tool;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::handlers::updated_hook_command;
use crate::tools::hook_names::HookToolName;
use crate::tools::orchestrator::ToolOrchestrator;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::PostToolUsePayload;
use crate::tools::registry::PreToolUsePayload;
use crate::tools::registry::ToolArgumentDiffConsumer;
use crate::tools::registry::ToolExecutor;
use crate::tools::runtimes::apply_patch::ApplyPatchRequest;
use crate::tools::runtimes::apply_patch::ApplyPatchRuntime;
use crate::tools::sandboxing::ToolCtx;
use ody_apply_patch::ApplyPatchAction;
use ody_apply_patch::ApplyPatchFileChange;
use ody_apply_patch::Hunk;
use ody_apply_patch::StreamingPatchParser;
use ody_exec_server::ExecutorFileSystem;
use ody_features::Feature;
use ody_protocol::models::AdditionalPermissionProfile;
use ody_protocol::models::FileSystemPermissions;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::FileChange;
use ody_protocol::protocol::PatchApplyUpdatedEvent;
use ody_protocol::protocol::WarningEvent;
use ody_sandboxing::policy_transforms::effective_file_system_sandbox_policy;
use ody_sandboxing::policy_transforms::merge_permission_profiles;
use ody_sandboxing::policy_transforms::normalize_additional_permissions;
use ody_tools::ToolName;
use ody_tools::ToolSpec;
use ody_utils_absolute_path::AbsolutePathBuf;
use ody_utils_path_uri::PathUri;
use serde::Deserialize;
use tracing::debug;

const APPLY_PATCH_ARGUMENT_DIFF_BUFFER_INTERVAL: Duration = Duration::from_millis(500);
/// Handles `apply_patch` requests and routes verified patches to the selected
/// environment filesystem.
#[derive(Default)]
pub struct ApplyPatchHandler {
    multi_environment: bool,
}

impl ApplyPatchHandler {
    pub(crate) fn new(multi_environment: bool) -> Self {
        Self { multi_environment }
    }
}

/// The arguments of the `apply_patch` function tool: the patch text verbatim.
#[derive(Debug, Deserialize)]
struct ApplyPatchToolArgs {
    input: String,
}

#[derive(Default)]
struct ApplyPatchArgumentDiffConsumer {
    decoder: PatchInputDecoder,
    parser: StreamingPatchParser,
    last_sent_at: Option<Instant>,
    pending: Option<PatchApplyUpdatedEvent>,
}

impl ToolArgumentDiffConsumer for ApplyPatchArgumentDiffConsumer {
    fn consume_diff(
        &mut self,
        turn: &TurnContext,
        call_id: String,
        diff: &str,
    ) -> Option<EventMsg> {
        if !turn
            .config
            .features
            .enabled(Feature::ApplyPatchStreamingEvents)
        {
            return None;
        }

        self.push_delta(call_id, diff)
            .map(EventMsg::PatchApplyUpdated)
    }

    fn finish(&mut self) -> Result<Option<EventMsg>, FunctionCallError> {
        self.finish_update_on_complete()
            .map(|event| event.map(EventMsg::PatchApplyUpdated))
    }
}

impl ApplyPatchArgumentDiffConsumer {
    fn push_delta(&mut self, call_id: String, delta: &str) -> Option<PatchApplyUpdatedEvent> {
        let patch_text = self.decoder.push(delta);
        if patch_text.is_empty() {
            return None;
        }
        let hunks = self.parser.push_delta(&patch_text).ok()?;
        if hunks.is_empty() {
            return None;
        }
        let changes = convert_apply_patch_hunks_to_protocol(&hunks);
        let event = PatchApplyUpdatedEvent { call_id, changes };
        let now = Instant::now();
        match self.last_sent_at {
            Some(last_sent_at)
                if now.duration_since(last_sent_at) < APPLY_PATCH_ARGUMENT_DIFF_BUFFER_INTERVAL =>
            {
                self.pending = Some(event);
                None
            }
            Some(_) | None => {
                self.pending = None;
                self.last_sent_at = Some(now);
                Some(event)
            }
        }
    }

    fn finish_update_on_complete(
        &mut self,
    ) -> Result<Option<PatchApplyUpdatedEvent>, FunctionCallError> {
        // Streaming is preview-only: `handle_call` re-parses the finished
        // arguments and is the authority on whether the patch is valid. A
        // half-streamed patch must never fail the tool call here.
        if let Err(err) = self.parser.finish() {
            debug!("apply_patch streaming preview did not parse: {err}");
            return Ok(None);
        }

        let event = self.pending.take();
        if event.is_some() {
            self.last_sent_at = Some(Instant::now());
        }
        Ok(event)
    }
}

/// Incrementally decodes the `input` string out of the streamed JSON arguments
/// of the `apply_patch` function tool, so the patch can be previewed while the
/// model is still generating it.
///
/// Only the decoded prefix is emitted on each call; once the string is closed
/// (or the arguments turn out not to contain a decodable `input`), the decoder
/// goes quiet. Errors are never surfaced — this feeds a preview, not the apply.
#[derive(Default)]
struct PatchInputDecoder {
    buffer: String,
    /// Byte offset of the next undecoded byte of the `input` string value.
    cursor: Option<usize>,
    done: bool,
}

impl PatchInputDecoder {
    fn push(&mut self, delta: &str) -> String {
        if self.done {
            return String::new();
        }
        self.buffer.push_str(delta);

        if self.cursor.is_none() {
            self.cursor = find_json_string_value_start(&self.buffer, "input");
        }
        let Some(mut cursor) = self.cursor else {
            return String::new();
        };

        let bytes = self.buffer.as_bytes();
        let mut decoded = String::new();
        while cursor < bytes.len() {
            match bytes[cursor] {
                // Closing quote: the whole `input` value has arrived.
                b'"' => {
                    self.done = true;
                    cursor += 1;
                    break;
                }
                b'\\' => match decode_json_escape(&self.buffer, cursor) {
                    JsonEscape::Decoded { ch, len } => {
                        decoded.push(ch);
                        cursor += len;
                    }
                    // The escape is split across deltas; resume once more arrives.
                    JsonEscape::Incomplete => break,
                    JsonEscape::Invalid => {
                        self.done = true;
                        break;
                    }
                },
                _ => {
                    let ch = self.buffer[cursor..].chars().next().unwrap_or_default();
                    decoded.push(ch);
                    cursor += ch.len_utf8();
                }
            }
        }
        self.cursor = Some(cursor);
        decoded
    }
}

enum JsonEscape {
    Decoded { ch: char, len: usize },
    Incomplete,
    Invalid,
}

/// Decodes the JSON escape sequence starting at `start` (which must be a `\`).
fn decode_json_escape(buffer: &str, start: usize) -> JsonEscape {
    let bytes = buffer.as_bytes();
    let Some(kind) = bytes.get(start + 1) else {
        return JsonEscape::Incomplete;
    };
    let simple = match kind {
        b'"' => Some('"'),
        b'\\' => Some('\\'),
        b'/' => Some('/'),
        b'b' => Some('\u{8}'),
        b'f' => Some('\u{c}'),
        b'n' => Some('\n'),
        b'r' => Some('\r'),
        b't' => Some('\t'),
        _ => None,
    };
    if let Some(ch) = simple {
        return JsonEscape::Decoded { ch, len: 2 };
    }
    if *kind != b'u' {
        return JsonEscape::Invalid;
    }

    let Some(first) = decode_json_hex4(buffer, start) else {
        return if buffer.len() < start + 6 {
            JsonEscape::Incomplete
        } else {
            JsonEscape::Invalid
        };
    };
    // Non-surrogate: a single \uXXXX is the whole character.
    if !(0xD800..=0xDFFF).contains(&first) {
        return match char::from_u32(first) {
            Some(ch) => JsonEscape::Decoded { ch, len: 6 },
            None => JsonEscape::Invalid,
        };
    }
    // Astral characters arrive as a surrogate pair: 😀.
    if !(0xD800..=0xDBFF).contains(&first) {
        return JsonEscape::Invalid;
    }
    if buffer.len() < start + 12 {
        return JsonEscape::Incomplete;
    }
    if bytes.get(start + 6) != Some(&b'\\') || bytes.get(start + 7) != Some(&b'u') {
        return JsonEscape::Invalid;
    }
    let Some(low) = decode_json_hex4(buffer, start + 6) else {
        return JsonEscape::Invalid;
    };
    if !(0xDC00..=0xDFFF).contains(&low) {
        return JsonEscape::Invalid;
    }
    let code = 0x1_0000 + ((first - 0xD800) << 10) + (low - 0xDC00);
    match char::from_u32(code) {
        Some(ch) => JsonEscape::Decoded { ch, len: 12 },
        None => JsonEscape::Invalid,
    }
}

/// Reads the 4 hex digits of a `\uXXXX` escape whose backslash is at `start`.
fn decode_json_hex4(buffer: &str, start: usize) -> Option<u32> {
    let hex = buffer.get(start + 2..start + 6)?;
    u32::from_str_radix(hex, 16).ok()
}

/// Finds the byte offset just past the opening quote of `"<key>": "` in a
/// possibly-incomplete JSON object. Returns `None` until the opening quote of
/// the value has arrived.
fn find_json_string_value_start(buffer: &str, key: &str) -> Option<usize> {
    let key_pattern = format!("\"{key}\"");
    let key_at = buffer.find(&key_pattern)?;
    let mut rest = buffer[key_at + key_pattern.len()..].char_indices();
    let mut seen_colon = false;
    for (offset, ch) in &mut rest {
        if ch.is_whitespace() {
            continue;
        }
        if ch == ':' && !seen_colon {
            seen_colon = true;
            continue;
        }
        if ch == '"' && seen_colon {
            return Some(key_at + key_pattern.len() + offset + 1);
        }
        // Anything else means this is not a string-valued `input` key.
        return None;
    }
    None
}

fn convert_apply_patch_hunks_to_protocol(hunks: &[Hunk]) -> HashMap<PathBuf, FileChange> {
    hunks
        .iter()
        .map(|hunk| {
            let path = hunk_source_path(hunk).to_path_buf();
            let change = match hunk {
                Hunk::AddFile { contents, .. } => FileChange::Add {
                    content: contents.clone(),
                },
                Hunk::DeleteFile { .. } => FileChange::Delete {
                    content: String::new(),
                },
                Hunk::UpdateFile {
                    chunks, move_path, ..
                } => FileChange::Update {
                    unified_diff: format_update_chunks_for_progress(chunks),
                    move_path: move_path.clone(),
                },
            };
            (path, change)
        })
        .collect()
}

fn hunk_source_path(hunk: &Hunk) -> &Path {
    match hunk {
        Hunk::AddFile { path, .. } | Hunk::DeleteFile { path } | Hunk::UpdateFile { path, .. } => {
            path
        }
    }
}

fn format_update_chunks_for_progress(chunks: &[ody_apply_patch::UpdateFileChunk]) -> String {
    let mut unified_diff = String::new();
    for chunk in chunks {
        match &chunk.change_context {
            Some(context) => {
                unified_diff.push_str("@@ ");
                unified_diff.push_str(context);
                unified_diff.push('\n');
            }
            None => {
                unified_diff.push_str("@@");
                unified_diff.push('\n');
            }
        }
        for line in &chunk.old_lines {
            unified_diff.push('-');
            unified_diff.push_str(line);
            unified_diff.push('\n');
        }
        for line in &chunk.new_lines {
            unified_diff.push('+');
            unified_diff.push_str(line);
            unified_diff.push('\n');
        }
        if chunk.is_end_of_file {
            unified_diff.push_str("*** End of File");
            unified_diff.push('\n');
        }
    }
    unified_diff
}

fn file_paths_for_action(action: &ApplyPatchAction) -> Vec<PathUri> {
    let mut keys = Vec::new();
    for (path, change) in action.changes() {
        keys.push(path.clone());

        if let ApplyPatchFileChange::Update { move_path, .. } = change
            && let Some(dest) = move_path
        {
            keys.push(dest.clone());
        }
    }

    keys
}

pub(crate) fn write_permissions_for_paths(
    file_paths: &[AbsolutePathBuf],
    file_system_sandbox_policy: &ody_protocol::permissions::FileSystemSandboxPolicy,
    cwd: &AbsolutePathBuf,
) -> Option<AdditionalPermissionProfile> {
    let write_paths = file_paths
        .iter()
        .map(|path| {
            path.parent()
                .unwrap_or_else(|| path.clone())
                .into_path_buf()
        })
        .filter(|path| {
            !file_system_sandbox_policy.can_write_path_with_cwd(path.as_path(), cwd.as_path())
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(AbsolutePathBuf::from_absolute_path)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;

    let permissions = (!write_paths.is_empty()).then_some(AdditionalPermissionProfile {
        file_system: Some(FileSystemPermissions::from_read_write_roots(
            Some(vec![]),
            Some(write_paths),
        )),
        ..Default::default()
    })?;

    normalize_additional_permissions(permissions).ok()
}

/// Extracts the raw patch text used as the command-shaped hook input for apply_patch.
fn apply_patch_payload_command(payload: &ToolPayload) -> Option<String> {
    match payload {
        ToolPayload::Function { arguments } => {
            serde_json::from_str::<ApplyPatchToolArgs>(arguments)
                .ok()
                .map(|args| args.input)
        }
        _ => None,
    }
}

async fn effective_patch_permissions(
    session: &Session,
    turn: &TurnContext,
    environment_id: &str,
    action: &ApplyPatchAction,
    cwd: &PathUri,
) -> std::io::Result<(
    Vec<PathUri>,
    crate::tools::handlers::EffectiveAdditionalPermissions,
    ody_protocol::permissions::FileSystemSandboxPolicy,
)> {
    let file_paths = file_paths_for_action(action);
    let native_cwd = cwd.to_abs_path()?;
    let granted_permissions = merge_permission_profiles(
        session
            .granted_session_permissions(environment_id)
            .await
            .as_ref(),
        session
            .granted_turn_permissions(environment_id)
            .await
            .as_ref(),
    );
    let base_file_system_sandbox_policy = turn.file_system_sandbox_policy();
    let file_system_sandbox_policy = effective_file_system_sandbox_policy(
        &base_file_system_sandbox_policy,
        granted_permissions.as_ref(),
    );
    let native_file_paths = file_paths
        .iter()
        .map(PathUri::to_abs_path)
        .collect::<Result<Vec<_>, _>>()?;
    let effective_additional_permissions = apply_granted_turn_permissions(
        session,
        environment_id,
        native_cwd.as_path(),
        crate::sandboxing::SandboxPermissions::UseDefault,
        write_permissions_for_paths(&native_file_paths, &file_system_sandbox_policy, &native_cwd),
    )
    .await;

    Ok((
        file_paths,
        effective_additional_permissions,
        file_system_sandbox_policy,
    ))
}

fn patch_permissions_without_path_matching(
    action: &ApplyPatchAction,
) -> (
    Vec<PathUri>,
    crate::tools::handlers::EffectiveAdditionalPermissions,
    ody_protocol::permissions::FileSystemSandboxPolicy,
) {
    // TODO(anp): Make permission matching operate on PathUri. Until then, foreign paths skip
    // permission matching; a managed turn still fails closed at the platform sandbox boundary.
    (
        file_paths_for_action(action),
        crate::tools::handlers::EffectiveAdditionalPermissions {
            sandbox_permissions: crate::sandboxing::SandboxPermissions::UseDefault,
            additional_permissions: None,
            permissions_preapproved: false,
        },
        ody_protocol::permissions::FileSystemSandboxPolicy::unrestricted(),
    )
}

impl ToolExecutor<ToolInvocation> for ApplyPatchHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("apply_patch")
    }

    fn spec(&self) -> ToolSpec {
        create_apply_patch_tool(self.multi_environment)
    }

    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(invocation))
    }
}

impl ApplyPatchHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            tracker,
            call_id,
            tool_name,
            payload,
            ..
        } = invocation;

        let ToolPayload::Function { arguments } = payload else {
            return Err(FunctionCallError::RespondToModel(
                "apply_patch handler received unsupported payload".to_string(),
            ));
        };
        let patch_input = match serde_json::from_str::<ApplyPatchToolArgs>(&arguments) {
            Ok(args) => args.input,
            Err(err) => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "failed to parse apply_patch arguments: {err}"
                )));
            }
        };
        let args = match ody_apply_patch::parse_patch(&patch_input) {
            Ok(args) => args,
            Err(parse_error) => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "apply_patch verification failed: {parse_error}"
                )));
            }
        };
        let selected_environment_id =
            require_environment_id(args.environment_id.as_deref(), self.multi_environment)?;

        // Verify the parsed patch against the selected environment filesystem.
        let Some(turn_environment) =
            resolve_tool_environment(turn.as_ref(), selected_environment_id.as_deref())?
        else {
            return Err(FunctionCallError::RespondToModel(
                "apply_patch is unavailable in this session".to_string(),
            ));
        };
        let fs = turn_environment.environment.get_filesystem();
        let sandbox = turn.file_system_sandbox_context(
            /*additional_permissions*/ None,
            turn_environment.cwd(),
        );
        match ody_apply_patch::verify_apply_patch_args(
            args,
            turn_environment.cwd(),
            fs.as_ref(),
            Some(&sandbox),
        )
        .await
        {
            ody_apply_patch::MaybeApplyPatchVerified::Body(changes) => {
                let (file_paths, effective_additional_permissions, file_system_sandbox_policy) =
                    effective_patch_permissions(
                        session.as_ref(),
                        turn.as_ref(),
                        &turn_environment.environment_id,
                        &changes,
                        turn_environment.cwd(),
                    )
                    .await
                    .unwrap_or_else(|_| patch_permissions_without_path_matching(&changes));
                match apply_patch::apply_patch(turn.as_ref(), &file_system_sandbox_policy, changes)
                    .await
                {
                    InternalApplyPatchInvocation::Output(Err(
                        FunctionCallError::RespondToModel(ref msg),
                    )) if msg.contains(PLAN_MODE_REJECTION_MARKER) => {
                        session
                            .send_event(
                                turn.as_ref(),
                                EventMsg::Warning(WarningEvent {
                                    message: msg.clone(),
                                }),
                            )
                            .await;
                        Err(FunctionCallError::RespondToModel(msg.clone()))
                    }
                    InternalApplyPatchInvocation::Output(item) => {
                        let content = item?;
                        Ok(boxed_tool_output(ApplyPatchToolOutput::from_text(content)))
                    }
                    InternalApplyPatchInvocation::DelegateToRuntime(apply) => {
                        let changes = convert_apply_patch_to_protocol(&apply.action);
                        let emitter = ToolEmitter::apply_patch_for_environment(
                            changes.clone(),
                            apply.auto_approved,
                            turn_environment.environment_id.clone(),
                        );
                        let event_ctx = ToolEventCtx::new(
                            session.as_ref(),
                            turn.as_ref(),
                            &call_id,
                            Some(&tracker),
                        );
                        emitter.begin(event_ctx).await;

                        let req = ApplyPatchRequest {
                            turn_environment: turn_environment.clone(),
                            action: apply.action,
                            file_paths,
                            changes,
                            exec_approval_requirement: apply.exec_approval_requirement,
                            additional_permissions: effective_additional_permissions
                                .additional_permissions,
                            permissions_preapproved: effective_additional_permissions
                                .permissions_preapproved,
                        };

                        let mut orchestrator = ToolOrchestrator::new();
                        let mut runtime = ApplyPatchRuntime::new();
                        let tool_ctx = ToolCtx {
                            session: session.clone(),
                            turn: turn.clone(),
                            call_id: call_id.clone(),
                            tool_name: tool_name.clone(),
                        };
                        let out = orchestrator
                            .run(
                                &mut runtime,
                                &req,
                                &tool_ctx,
                                turn.as_ref(),
                                turn.approval_policy.value(),
                            )
                            .await
                            .map(|result| result.output);
                        let (out, delta) = match out {
                            Ok(output) => (Ok(output.exec_output), Some(output.delta)),
                            Err(error) => (Err(error), Some(runtime.committed_delta().clone())),
                        };
                        let event_ctx = ToolEventCtx::new(
                            session.as_ref(),
                            turn.as_ref(),
                            &call_id,
                            Some(&tracker),
                        );
                        let content = emitter.finish(event_ctx, out, delta.as_ref()).await?;
                        Ok(boxed_tool_output(ApplyPatchToolOutput::from_text(content)))
                    }
                }
            }
            ody_apply_patch::MaybeApplyPatchVerified::CorrectnessError(parse_error) => {
                Err(FunctionCallError::RespondToModel(format!(
                    "apply_patch verification failed: {parse_error}"
                )))
            }
            ody_apply_patch::MaybeApplyPatchVerified::ShellParseError(error) => {
                tracing::trace!("Failed to parse apply_patch input, {error:?}");
                Err(FunctionCallError::RespondToModel(
                    "apply_patch handler received invalid patch input".to_string(),
                ))
            }
            ody_apply_patch::MaybeApplyPatchVerified::NotApplyPatch => {
                Err(FunctionCallError::RespondToModel(
                    "apply_patch handler received non-apply_patch input".to_string(),
                ))
            }
        }
    }
}

impl CoreToolRuntime for ApplyPatchHandler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }

    fn create_diff_consumer(&self) -> Option<Box<dyn ToolArgumentDiffConsumer>> {
        Some(Box::<ApplyPatchArgumentDiffConsumer>::default())
    }

    fn pre_tool_use_payload(&self, invocation: &ToolInvocation) -> Option<PreToolUsePayload> {
        apply_patch_payload_command(&invocation.payload).map(|command| PreToolUsePayload {
            tool_name: HookToolName::apply_patch(),
            tool_input: serde_json::json!({ "command": command }),
        })
    }

    fn with_updated_hook_input(
        &self,
        mut invocation: ToolInvocation,
        updated_input: serde_json::Value,
    ) -> Result<ToolInvocation, FunctionCallError> {
        let patch = updated_hook_command(&updated_input)?;
        invocation.payload = match invocation.payload {
            ToolPayload::Function { .. } => ToolPayload::Function {
                arguments: serde_json::json!({ "input": patch }).to_string(),
            },
            payload => payload,
        };
        Ok(invocation)
    }

    fn post_tool_use_payload(
        &self,
        invocation: &ToolInvocation,
        result: &dyn crate::tools::context::ToolOutput,
    ) -> Option<PostToolUsePayload> {
        let tool_response =
            result.post_tool_use_response(&invocation.call_id, &invocation.payload)?;
        Some(PostToolUsePayload {
            tool_name: HookToolName::apply_patch(),
            tool_use_id: invocation.call_id.clone(),
            tool_input: serde_json::json!({
                "command": apply_patch_payload_command(&invocation.payload)?,
            }),
            tool_response,
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn intercept_apply_patch(
    command: &[String],
    cwd: &PathUri,
    fs: &dyn ExecutorFileSystem,
    turn_environment: TurnEnvironment,
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    tracker: Option<&SharedTurnDiffTracker>,
    call_id: &str,
    tool_name: &str,
) -> Result<Option<FunctionToolOutput>, FunctionCallError> {
    let sandbox = turn.file_system_sandbox_context(/*additional_permissions*/ None, cwd);
    match ody_apply_patch::maybe_parse_apply_patch_verified(command, cwd, fs, Some(&sandbox)).await
    {
        ody_apply_patch::MaybeApplyPatchVerified::Body(changes) => {
            let (approval_keys, effective_additional_permissions, file_system_sandbox_policy) =
                effective_patch_permissions(
                    session.as_ref(),
                    turn.as_ref(),
                    &turn_environment.environment_id,
                    &changes,
                    cwd,
                )
                .await
                .unwrap_or_else(|_| patch_permissions_without_path_matching(&changes));
            match apply_patch::apply_patch(turn.as_ref(), &file_system_sandbox_policy, changes)
                .await
            {
                InternalApplyPatchInvocation::Output(Err(FunctionCallError::RespondToModel(
                    ref msg,
                ))) if msg.contains(PLAN_MODE_REJECTION_MARKER) => {
                    session
                        .send_event(
                            turn.as_ref(),
                            EventMsg::Warning(WarningEvent {
                                message: msg.clone(),
                            }),
                        )
                        .await;
                    Err(FunctionCallError::RespondToModel(msg.clone()))
                }
                InternalApplyPatchInvocation::Output(item) => {
                    let content = item?;
                    Ok(Some(FunctionToolOutput::from_text(content, Some(true))))
                }
                InternalApplyPatchInvocation::DelegateToRuntime(apply) => {
                    let changes = convert_apply_patch_to_protocol(&apply.action);
                    let emitter = ToolEmitter::apply_patch_for_environment(
                        changes.clone(),
                        apply.auto_approved,
                        turn_environment.environment_id.clone(),
                    );
                    let event_ctx = ToolEventCtx::new(
                        session.as_ref(),
                        turn.as_ref(),
                        call_id,
                        tracker.as_ref().copied(),
                    );
                    emitter.begin(event_ctx).await;

                    let req = ApplyPatchRequest {
                        turn_environment,
                        action: apply.action,
                        file_paths: approval_keys,
                        changes,
                        exec_approval_requirement: apply.exec_approval_requirement,
                        additional_permissions: effective_additional_permissions
                            .additional_permissions,
                        permissions_preapproved: effective_additional_permissions
                            .permissions_preapproved,
                    };

                    let mut orchestrator = ToolOrchestrator::new();
                    let mut runtime = ApplyPatchRuntime::new();
                    let tool_ctx = ToolCtx {
                        session: session.clone(),
                        turn: turn.clone(),
                        call_id: call_id.to_string(),
                        tool_name: ToolName::plain(tool_name),
                    };
                    let out = orchestrator
                        .run(
                            &mut runtime,
                            &req,
                            &tool_ctx,
                            turn.as_ref(),
                            turn.approval_policy.value(),
                        )
                        .await
                        .map(|result| result.output);
                    let (out, delta) = match out {
                        Ok(output) => (Ok(output.exec_output), Some(output.delta)),
                        Err(error) => (Err(error), Some(runtime.committed_delta().clone())),
                    };
                    let event_ctx = ToolEventCtx::new(
                        session.as_ref(),
                        turn.as_ref(),
                        call_id,
                        tracker.as_ref().copied(),
                    );
                    let content = emitter.finish(event_ctx, out, delta.as_ref()).await?;
                    Ok(Some(FunctionToolOutput::from_text(content, Some(true))))
                }
            }
        }
        ody_apply_patch::MaybeApplyPatchVerified::CorrectnessError(parse_error) => {
            Err(FunctionCallError::RespondToModel(format!(
                "apply_patch verification failed: {parse_error}"
            )))
        }
        ody_apply_patch::MaybeApplyPatchVerified::ShellParseError(error) => {
            tracing::trace!("Failed to parse apply_patch input, {error:?}");
            Ok(None)
        }
        ody_apply_patch::MaybeApplyPatchVerified::NotApplyPatch => Ok(None),
    }
}

fn require_environment_id(
    parsed_environment_id: Option<&str>,
    allow_environment_id: bool,
) -> Result<Option<String>, FunctionCallError> {
    match parsed_environment_id {
        Some(_) if !allow_environment_id => Err(FunctionCallError::RespondToModel(
            "apply_patch environment selection is unavailable for this turn".to_string(),
        )),
        Some(environment_id) => Ok(Some(environment_id.to_string())),
        None => Ok(None),
    }
}

#[cfg(test)]
#[path = "apply_patch_tests.rs"]
mod tests;
