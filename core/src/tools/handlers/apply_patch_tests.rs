use super::*;
use ody_apply_patch::MaybeApplyPatchVerified;
use ody_exec_server::LOCAL_FS;
use ody_protocol::permissions::FileSystemSandboxPolicy;
use ody_protocol::protocol::FileChange;
use core_test_support::PathBufExt;
use core_test_support::PathExt;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::Mutex;

use crate::safety::PLAN_MODE_REJECTION_MARKER;
use crate::session::tests::make_session_and_context;
use crate::session::tests::make_session_and_context_with_rx;
use crate::tools::context::ToolInvocation;
use crate::tools::hook_names::HookToolName;
use crate::tools::registry::PostToolUsePayload;
use crate::tools::registry::PreToolUsePayload;
use crate::turn_diff_tracker::TurnDiffTracker;
use ody_config::config_toml::PlanEnforcement;
use ody_config::config_toml::PlanModeConfigToml;
use ody_protocol::config_types::ModeKind;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::WarningEvent;

fn sample_patch() -> &'static str {
    r#"*** Begin Patch
*** Add File: hello.txt
+hello
*** End Patch"#
}

async fn invocation_for_payload(payload: ToolPayload) -> ToolInvocation {
    let (session, turn) = make_session_and_context().await;
    ToolInvocation {
        session: session.into(),
        turn: turn.into(),
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        tracker: Arc::new(Mutex::new(TurnDiffTracker::new())),
        call_id: "call-apply-patch".to_string(),
        tool_name: ody_tools::ToolName::plain("apply_patch"),
        source: crate::tools::context::ToolCallSource::Direct,
        payload,
    }
}

/// The `apply_patch` function-tool payload carrying `patch`.
fn patch_payload(patch: &str) -> ToolPayload {
    ToolPayload::Function {
        arguments: json!({ "input": patch }).to_string(),
    }
}

#[tokio::test]
async fn pre_tool_use_payload_uses_patch_input() {
    let patch = sample_patch();
    let payload = patch_payload(&patch);
    let invocation = invocation_for_payload(payload).await;
    let handler = ApplyPatchHandler::default();

    assert_eq!(
        handler.pre_tool_use_payload(&invocation),
        Some(PreToolUsePayload {
            tool_name: HookToolName::apply_patch(),
            tool_input: json!({ "command": patch }),
        })
    );
}

#[tokio::test]
async fn post_tool_use_payload_uses_patch_input_and_tool_output() {
    let patch = sample_patch();
    let payload = patch_payload(&patch);
    let invocation = invocation_for_payload(payload).await;
    let output = ApplyPatchToolOutput::from_text("Success. Updated files.".to_string());
    let handler = ApplyPatchHandler::default();

    assert_eq!(
        handler.post_tool_use_payload(&invocation, &output),
        Some(PostToolUsePayload {
            tool_name: HookToolName::apply_patch(),
            tool_use_id: "call-apply-patch".to_string(),
            tool_input: json!({ "command": patch }),
            tool_response: json!("Success. Updated files."),
        })
    );
}

/// Encodes `chunk` of patch text as it arrives inside the streamed JSON
/// arguments: the first chunk opens the object and the `input` string, the last
/// one closes both.
fn opening_delta(chunk: &str) -> String {
    format!(r#"{{"input": "{}"#, escaped_delta(chunk))
}

fn escaped_delta(chunk: &str) -> String {
    let encoded = serde_json::to_string(chunk).expect("encode chunk");
    encoded[1..encoded.len() - 1].to_string()
}

fn closing_delta(chunk: &str) -> String {
    format!(r#"{}"}}"#, escaped_delta(chunk))
}

#[test]
fn diff_consumer_streams_apply_patch_changes() {
    let mut consumer = ApplyPatchArgumentDiffConsumer::default();
    assert!(
        consumer
            .push_delta("call-1".to_string(), &opening_delta("*** Begin Patch\n"))
            .is_none()
    );

    let event = consumer
        .push_delta(
            "call-1".to_string(),
            &escaped_delta("*** Add File: hello.txt\n+hello"),
        )
        .expect("progress event");
    assert_eq!(
        (event.call_id, event.changes),
        (
            "call-1".to_string(),
            HashMap::from([(
                PathBuf::from("hello.txt"),
                FileChange::Add {
                    content: String::new(),
                },
            )]),
        )
    );

    assert!(
        consumer
            .push_delta("call-1".to_string(), &escaped_delta("\n+world"))
            .is_none()
    );
    assert!(
        consumer
            .push_delta("call-1".to_string(), &closing_delta("\n*** End Patch"))
            .is_none()
    );

    let event = consumer
        .finish_update_on_complete()
        .expect("finish parser")
        .expect("progress event");
    assert_eq!(
        (event.call_id, event.changes),
        (
            "call-1".to_string(),
            HashMap::from([(
                PathBuf::from("hello.txt"),
                FileChange::Add {
                    content: "hello\nworld\n".to_string(),
                },
            )]),
        )
    );
}

#[test]
fn diff_consumer_streams_apply_patch_changes_with_environment_header() {
    let mut consumer = ApplyPatchArgumentDiffConsumer::default();
    assert!(
        consumer
            .push_delta(
                "call-1".to_string(),
                &opening_delta("*** Begin Patch\n*** Environment ID: remote\n"),
            )
            .is_none()
    );

    let event = consumer
        .push_delta(
            "call-1".to_string(),
            &escaped_delta("*** Add File: hello.txt\n+hello"),
        )
        .expect("progress event");
    assert_eq!(
        event.changes,
        HashMap::from([(
            PathBuf::from("hello.txt"),
            FileChange::Add {
                content: String::new(),
            },
        )])
    );
}

#[test]
fn diff_consumer_sends_next_update_after_buffer_interval() {
    let mut consumer = ApplyPatchArgumentDiffConsumer::default();
    consumer.push_delta("call-1".to_string(), &opening_delta("*** Begin Patch\n"));
    let first = consumer
        .push_delta(
            "call-1".to_string(),
            &escaped_delta("*** Add File: hello.txt\n+hello"),
        )
        .expect("first progress event");
    assert_eq!(
        first.changes,
        HashMap::from([(
            PathBuf::from("hello.txt"),
            FileChange::Add {
                content: String::new(),
            },
        )])
    );

    consumer.last_sent_at =
        Some(std::time::Instant::now() - APPLY_PATCH_ARGUMENT_DIFF_BUFFER_INTERVAL);
    let second = consumer
        .push_delta("call-1".to_string(), &escaped_delta("\n+world"))
        .expect("second progress event");
    assert_eq!(
        second.changes,
        HashMap::from([(
            PathBuf::from("hello.txt"),
            FileChange::Add {
                content: "hello\n".to_string(),
            },
        )])
    );
}

#[test]
fn reconcile_environment_id_requires_selection_when_enabled() {
    assert_eq!(
        require_environment_id(Some("remote"), /*allow_environment_id*/ false),
        Err(FunctionCallError::RespondToModel(
            "apply_patch environment selection is unavailable for this turn".to_string(),
        ))
    );
    assert_eq!(
        require_environment_id(
            /*parsed_environment_id*/ None, /*allow_environment_id*/ true
        ),
        Ok(None)
    );
}

#[tokio::test]
async fn approval_keys_include_move_destination() {
    let tmp = TempDir::new().expect("tmp");
    let cwd_path = tmp.path();
    let cwd = cwd_path.abs();
    std::fs::create_dir_all(cwd_path.join("old")).expect("create old dir");
    std::fs::create_dir_all(cwd_path.join("renamed/dir")).expect("create dest dir");
    std::fs::write(cwd_path.join("old/name.txt"), "old content\n").expect("write old file");
    let patch = r#"*** Begin Patch
*** Update File: old/name.txt
*** Move to: renamed/dir/name.txt
@@
-old content
+new content
*** End Patch"#;
    let argv = vec!["apply_patch".to_string(), patch.to_string()];
    // TODO(anp): Keep apply_patch handler test cwd values as PathUri.
    let cwd = PathUri::from_abs_path(&cwd);
    let action = match ody_apply_patch::maybe_parse_apply_patch_verified(
        &argv,
        &cwd,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    {
        MaybeApplyPatchVerified::Body(action) => action,
        other => panic!("expected patch body, got: {other:?}"),
    };

    let keys = file_paths_for_action(&action);
    assert_eq!(keys.len(), 2);
}

#[test]
fn write_permissions_for_paths_skip_dirs_already_writable_under_workspace_root() {
    let tmp = TempDir::new().expect("tmp");
    let cwd_path = tmp.path();
    let cwd = cwd_path.abs();
    let nested = cwd_path.join("nested");
    std::fs::create_dir_all(&nested).expect("create nested dir");
    let file_path = AbsolutePathBuf::try_from(nested.join("file.txt"))
        .expect("nested file path should be absolute");
    let sandbox_policy = FileSystemSandboxPolicy::workspace_write(
        &[],
        /*exclude_tmpdir_env_var*/ true,
        /*exclude_slash_tmp*/ false,
    );

    let permissions = write_permissions_for_paths(&[file_path], &sandbox_policy, &cwd);

    assert_eq!(permissions, None);
}

#[test]
fn write_permissions_for_paths_keep_dirs_outside_workspace_root() {
    let tmp = TempDir::new().expect("tmp");
    let cwd = tmp.path().join("workspace");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&cwd).expect("create cwd");
    std::fs::create_dir_all(&outside).expect("create outside dir");
    let file_path = AbsolutePathBuf::try_from(outside.join("file.txt"))
        .expect("outside file path should be absolute");
    let cwd_abs = cwd.abs();
    let sandbox_policy = FileSystemSandboxPolicy::workspace_write(
        &[],
        /*exclude_tmpdir_env_var*/ true,
        /*exclude_slash_tmp*/ true,
    );

    let permissions = write_permissions_for_paths(&[file_path], &sandbox_policy, &cwd_abs);
    let expected_outside =
        dunce::simplified(&outside.canonicalize().expect("canonicalize outside dir")).abs();

    assert_eq!(
        permissions
            .and_then(|profile| profile.file_system)
            .and_then(|fs| fs.legacy_read_write_roots())
            .and_then(|(_read, write)| write),
        Some(vec![expected_outside])
    );
}

async fn plan_mode_invocation_for_payload(
    payload: ToolPayload,
) -> (ToolInvocation, async_channel::Receiver<ody_protocol::protocol::Event>) {
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

    let invocation = ToolInvocation {
        session,
        turn,
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        tracker: Arc::new(Mutex::new(TurnDiffTracker::new())),
        call_id: "call-apply-patch".to_string(),
        tool_name: ody_tools::ToolName::plain("apply_patch"),
        source: crate::tools::context::ToolCallSource::Direct,
        payload,
    };
    (invocation, rx)
}

#[tokio::test]
async fn plan_mode_patch_denial_emits_warning() {
    let payload = patch_payload(&sample_patch());
    let (invocation, rx) = plan_mode_invocation_for_payload(payload).await;
    let handler = ApplyPatchHandler::default();

    let result = handler.handle(invocation).await;
    let msg = match result {
        Err(FunctionCallError::RespondToModel(msg)) => msg,
        Err(other) => panic!("expected RespondToModel error, got {other}"),
        Ok(_) => panic!("expected error, got success"),
    };
    assert!(
        msg.contains(PLAN_MODE_REJECTION_MARKER),
        "denial message should contain marker: {msg}"
    );
    assert!(msg.contains("hello.txt"), "denial message should name file: {msg}");

    loop {
        let event = rx.recv().await.expect("expected event");
        match event.msg {
            EventMsg::Warning(WarningEvent { message }) if message == msg => break,
            _ => continue,
        }
    }
}

/// Feeds `deltas` to the decoder and returns everything it decoded.
fn decode_stream(deltas: &[&str]) -> String {
    let mut decoder = PatchInputDecoder::default();
    deltas.iter().map(|delta| decoder.push(delta)).collect()
}

const PATCH: &str = "*** Begin Patch\n*** Add File: a.txt\n+hi\n*** End Patch\n";

#[test]
fn patch_input_decoder_reconstructs_the_patch_from_streamed_json() {
    let arguments = json!({ "input": PATCH }).to_string();
    // One byte at a time: the worst case a provider can hand us.
    let deltas = arguments
        .as_bytes()
        .chunks(1)
        .map(|chunk| std::str::from_utf8(chunk).expect("ascii"))
        .collect::<Vec<_>>();

    assert_eq!(decode_stream(&deltas), PATCH);
}

#[test]
fn patch_input_decoder_handles_escapes_split_across_deltas() {
    // The \n escape is torn in half, as is the \" escape.
    assert_eq!(
        decode_stream(&[r#"{"input": "a\"#, r#"nb\"#, r#""c""#, "}"]),
        "a\nb\"c"
    );
}

#[test]
fn patch_input_decoder_decodes_unicode_and_surrogate_pairs() {
    let text = "é😀";
    let arguments = serde_json::to_string(&json!({ "input": text })).expect("serialize");
    // serde_json emits astral chars raw, so force the \u form providers may send.
    let escaped = r#"{"input": "é😀"}"#;

    assert_eq!(decode_stream(&[&arguments]), text);
    assert_eq!(decode_stream(&[escaped]), text);
    // Torn surrogate pair: nothing is emitted until the low half arrives.
    assert_eq!(decode_stream(&[r#"{"input": "\ud83d"#, r#"\ude00""#]), "😀");
}

#[test]
fn patch_input_decoder_stops_at_the_closing_quote() {
    let arguments = format!(
        r#"{{"input": {}, "unexpected": "trailing"}}"#,
        serde_json::to_string(PATCH).expect("serialize")
    );

    assert_eq!(decode_stream(&[&arguments]), PATCH);
}

#[test]
fn patch_input_decoder_stays_quiet_when_there_is_no_input_string() {
    assert_eq!(decode_stream(&[r#"{"command": "not-input"}"#]), "");
    assert_eq!(decode_stream(&[r#"{"input": 42}"#]), "");
    // Partial arguments that have not reached the value yet.
    assert_eq!(decode_stream(&[r#"{"inp"#]), "");
}
