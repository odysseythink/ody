use super::write_edit::atomic_write;
use super::write_edit::ensure_write_allowed;
use super::write_edit::file_change_for_write;
use super::write_edit::resolve_write_cwd;
use super::write_edit::resolve_write_path;
use super::write_edit::single_file_change;
use crate::function_tool::FunctionCallError;
use crate::plan_mode_injector::parts_manifest::normalize_part_path;
use crate::plan_mode_injector::parts_manifest::parse_parts_manifest;
use crate::plan_mode_injector::parts_manifest::part_completion_violations;
use crate::plan_mode_injector::parts_manifest::row_is_verified_done;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::events::ToolEventCtx;
use crate::tools::events::emit_direct_file_change;
use crate::tools::handlers::file_tools_spec::FileToolOptions;
use crate::tools::handlers::file_tools_spec::WRITE_FILE_TOOL_NAME;
use crate::tools::handlers::file_tools_spec::create_write_file_tool;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use ody_protocol::config_types::ModeKind;
use ody_protocol::models::ResponseInputItem;
use ody_protocol::protocol::FileChange;
use ody_tools::ToolName;
use ody_tools::ToolSpec;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use std::path::Path;

#[derive(Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
    #[serde(default)]
    append: bool,
    #[serde(default)]
    environment_id: Option<String>,
}

struct WriteFileOutput {
    bytes_written: usize,
}

impl ToolOutput for WriteFileOutput {
    fn log_preview(&self) -> String {
        format!("{} bytes written", self.bytes_written)
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        FunctionToolOutput::from_text(
            format!("Wrote {} bytes to the file", self.bytes_written),
            Some(true),
        )
        .to_response_item(call_id, payload)
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> Value {
        json!({
            "success": true,
            "bytes_written": self.bytes_written,
        })
    }
}

#[derive(Default)]
pub struct WriteFileHandler {
    options: FileToolOptions,
}

impl WriteFileHandler {
    pub fn new(options: FileToolOptions) -> Self {
        Self { options }
    }
}

fn split_part_size_violation(
    markdown: &str,
    stem_dir: &Path,
    path: &Path,
    bytes: usize,
    max_bytes: usize,
) -> Option<String> {
    if max_bytes == 0 || bytes <= max_bytes {
        return None;
    }
    let manifest = parse_parts_manifest(markdown).manifest?;
    let row = manifest.rows.iter().find(|row| {
        normalize_part_path(stem_dir, &row.file).is_some_and(|expected| expected == path)
    })?;
    Some(format!(
        "write_file rejected: `{}` is a split-plan part and {} bytes exceeds its configured {}-byte limit. The accepted `## Parts` manifest is frozen: do not split, rename, or replace its rows after this rejection. Keep the same task and preserve its concrete implementation detail, source evidence, edge cases, and behavioral tests; if the complete part cannot fit, ask the user to raise `plan_mode.max_part_bytes`. No file was changed.",
        row.file, bytes, max_bytes
    ))
}

fn enforce_split_part_size_budget(
    turn: &crate::session::turn_context::TurnContext,
    path: &Path,
    bytes: usize,
) -> Result<(), FunctionCallError> {
    if !matches!(
        turn.collaboration_mode.mode,
        ModeKind::Plan | ModeKind::Design
    ) {
        return Ok(());
    }
    let Some(artifact) = turn.plan_artifact.as_ref() else {
        return Ok(());
    };
    let (Some(stem_dir), Some(markdown)) = (artifact.stem_dir(), artifact.last_plan_text()) else {
        return Ok(());
    };
    let max_bytes = turn
        .config
        .plan_mode
        .as_ref()
        .and_then(|config| config.max_part_bytes)
        .unwrap_or(0);
    if let Some(message) = split_part_size_violation(&markdown, &stem_dir, path, bytes, max_bytes) {
        return Err(FunctionCallError::RespondToModel(message));
    }
    Ok(())
}

fn task_part_write_violation(
    markdown: &str,
    stem_dir: &Path,
    path: &Path,
    max_tasks: usize,
    max_bytes: usize,
) -> Option<String> {
    let manifest = parse_parts_manifest(markdown).manifest?;
    if !manifest.is_task_mode() {
        return None;
    }
    let expected = manifest.rows.iter().find(|row| {
        !row_is_verified_done(stem_dir, row)
            || !part_completion_violations(stem_dir, &manifest, row, max_tasks, max_bytes)
                .is_empty()
    })?;
    let expected_path = normalize_part_path(stem_dir, &expected.file)?;
    if path == expected_path {
        return None;
    }
    let attempted = manifest.rows.iter().find(|row| {
        normalize_part_path(stem_dir, &row.file).is_some_and(|candidate| candidate == path)
    });
    match attempted {
        Some(row) => Some(format!(
            "write_file rejected: task-mode plans advance in manifest order. `{}` is the active pending task; do not write `{}` until its row is verified done.",
            expected.id, row.id
        )),
        None if path.parent().is_some_and(|parent| parent == stem_dir) => Some(format!(
            "write_file rejected: `{}` is not a manifest task part. The active pending task is `{}` at `{}`.",
            path.display(),
            expected.id,
            expected_path.display()
        )),
        None => None,
    }
}

fn enforce_task_part_order(
    turn: &crate::session::turn_context::TurnContext,
    path: &Path,
) -> Result<(), FunctionCallError> {
    if turn.collaboration_mode.mode != ModeKind::Plan {
        return Ok(());
    }
    let Some(artifact) = turn.plan_artifact.as_ref() else {
        return Ok(());
    };
    let (Some(stem_dir), Some(markdown)) = (artifact.stem_dir(), artifact.last_plan_text()) else {
        return Ok(());
    };
    let plan_mode = turn.config.plan_mode.as_ref();
    let max_tasks = plan_mode
        .and_then(|config| config.max_tasks_per_part)
        .unwrap_or(3);
    let max_bytes = plan_mode
        .and_then(|config| config.max_part_bytes)
        .unwrap_or(0);
    if let Some(message) =
        task_part_write_violation(&markdown, &stem_dir, path, max_tasks, max_bytes)
    {
        return Err(FunctionCallError::RespondToModel(message));
    }
    Ok(())
}

impl ToolExecutor<ToolInvocation> for WriteFileHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(WRITE_FILE_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_write_file_tool(self.options)
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolInvocation {
                session,
                turn,
                call_id,
                payload,
                ..
            } = invocation;
            let ToolPayload::Function { arguments } = payload else {
                return Err(FunctionCallError::RespondToModel(format!(
                    "{WRITE_FILE_TOOL_NAME} handler received unsupported payload"
                )));
            };
            let args: WriteFileArgs = parse_arguments(&arguments)?;
            let abs_path =
                resolve_write_path(turn.as_ref(), args.environment_id.as_deref(), &args.path)
                    .await?;
            let cwd = resolve_write_cwd(turn.as_ref(), args.environment_id.as_deref()).await?;
            let Some(turn_environment) = crate::tools::handlers::resolve_tool_environment(
                turn.as_ref(),
                args.environment_id.as_deref(),
            )?
            else {
                return Err(FunctionCallError::RespondToModel(
                    "write_file is unavailable in this session".to_string(),
                ));
            };
            ensure_write_allowed(
                session.as_ref(),
                turn.as_ref(),
                &turn_environment.environment_id,
                &abs_path,
                &cwd,
            )
            .await?;

            let old_content = tokio::fs::read_to_string(abs_path.as_path()).await.ok();
            let new_content = if args.append {
                format!("{}{}", old_content.as_deref().unwrap_or(""), args.content)
            } else {
                args.content
            };

            enforce_task_part_order(turn.as_ref(), abs_path.as_path())?;
            enforce_split_part_size_budget(turn.as_ref(), abs_path.as_path(), new_content.len())?;

            atomic_write(&abs_path, new_content.as_bytes()).await?;

            let change =
                file_change_for_write(abs_path.as_path(), old_content.as_deref(), &new_content);
            let unified_diff = if let FileChange::Update { unified_diff, .. } = &change {
                Some(unified_diff.clone())
            } else {
                None
            };
            let path_buf = abs_path.as_path().to_path_buf();
            let change = single_file_change(path_buf.clone(), change)
                .into_iter()
                .next()
                .map(|(_, change)| change)
                .expect("single file change");
            emit_direct_file_change(
                ToolEventCtx::new(session.as_ref(), turn.as_ref(), &call_id, None),
                path_buf,
                change,
                WRITE_FILE_TOOL_NAME,
                format!("Wrote {} bytes", new_content.len()),
                String::new(),
                unified_diff,
            )
            .await;

            Ok(boxed_tool_output(WriteFileOutput {
                bytes_written: new_content.len(),
            }))
        })
    }
}

impl CoreToolRuntime for WriteFileHandler {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::make_session_and_context_with_rx;
    use crate::session::turn_context::TurnEnvironment;
    use crate::tools::context::ToolInvocation;
    use crate::turn_diff_tracker::TurnDiffTracker;
    use ody_utils_path_uri::PathUri;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn split_part_size_violation_blocks_only_manifest_parts_over_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let stem = tmp.path().join("topic");
        std::fs::create_dir_all(&stem).unwrap();
        let markdown = "## Parts\n| # | File | Scope | Status |\n|---|---|---|---|\n| 4a | topic/core.md | core | pending |\n";

        let violation = split_part_size_violation(markdown, &stem, &stem.join("core.md"), 25, 24)
            .expect("manifest part should be constrained");
        assert!(violation.contains("manifest is frozen"));
        assert!(violation.contains("do not split, rename, or replace"));
        assert!(violation.contains("raise `plan_mode.max_part_bytes`"));
        assert!(violation.contains("No file was changed"));
        assert!(
            split_part_size_violation(markdown, &stem, &stem.join("unrelated.md"), 25, 24,)
                .is_none()
        );
    }

    #[test]
    fn task_mode_blocks_writing_a_later_task_before_the_active_one() {
        let tmp = tempfile::tempdir().unwrap();
        let stem = tmp.path().join("topic");
        std::fs::create_dir_all(&stem).unwrap();
        let markdown = "## Parts\n| ID | File | Task | Scope | Depends on | Status |\n|---|---|---|---|---|---|\n| T01 | topic/first.md | First task | first | — | pending |\n| T02 | topic/second.md | Second task | second | T01 | pending |\n";

        assert!(task_part_write_violation(markdown, &stem, &stem.join("first.md"), 1, 0).is_none());
        let violation = task_part_write_violation(markdown, &stem, &stem.join("second.md"), 1, 0)
            .expect("later task must be blocked");
        assert!(violation.contains("active pending task"), "{violation}");
        assert!(violation.contains("`T01`"), "{violation}");
    }

    async fn invocation_for_write(
        session: Arc<crate::session::session::Session>,
        turn: Arc<crate::session::turn_context::TurnContext>,
        call_id: &str,
        args: serde_json::Value,
    ) -> ToolInvocation {
        ToolInvocation {
            session,
            turn,
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            tracker: Arc::new(Mutex::new(TurnDiffTracker::new())),
            call_id: call_id.to_string(),
            tool_name: ody_tools::ToolName::plain(WRITE_FILE_TOOL_NAME),
            source: crate::tools::context::ToolCallSource::Direct,
            payload: ToolPayload::Function {
                arguments: args.to_string(),
            },
        }
    }

    fn set_cwd_to_temp(
        turn: &mut Arc<crate::session::turn_context::TurnContext>,
        cwd: &std::path::Path,
    ) {
        let turn_context_mut = Arc::get_mut(turn).expect("single reference");
        let current = turn_context_mut.environments.turn_environments[0].clone();
        turn_context_mut.environments.turn_environments[0] = TurnEnvironment::new(
            current.environment_id,
            current.environment,
            PathUri::from_abs_path(
                &ody_utils_absolute_path::AbsolutePathBuf::from_absolute_path(cwd).unwrap(),
            ),
            current.shell,
        );
    }

    #[tokio::test]
    async fn write_file_creates_new_file() {
        let (session, mut turn, _rx) = make_session_and_context_with_rx().await;
        let dir = tempfile::tempdir().expect("tempdir");
        set_cwd_to_temp(&mut turn, dir.path());

        let invocation = invocation_for_write(
            session,
            turn,
            "write-call-1",
            json!({ "path": "hello.txt", "content": "hello
" }),
        )
        .await;
        let handler = WriteFileHandler::new(FileToolOptions::default());
        handler.handle(invocation).await.expect("write succeeds");

        let content = std::fs::read_to_string(dir.path().join("hello.txt")).expect("read");
        assert_eq!(
            content,
            "hello
"
        );
    }

    #[tokio::test]
    async fn write_file_overwrites_existing_file() {
        let (session, mut turn, _rx) = make_session_and_context_with_rx().await;
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("foo.txt"),
            "old
",
        )
        .unwrap();
        set_cwd_to_temp(&mut turn, dir.path());

        let invocation = invocation_for_write(
            session,
            turn,
            "write-call-2",
            json!({ "path": "foo.txt", "content": "new
" }),
        )
        .await;
        let handler = WriteFileHandler::new(FileToolOptions::default());
        handler.handle(invocation).await.expect("write succeeds");

        let content = std::fs::read_to_string(dir.path().join("foo.txt")).expect("read");
        assert_eq!(
            content,
            "new
"
        );
    }

    #[tokio::test]
    async fn write_file_appends_to_existing_file() {
        let (session, mut turn, _rx) = make_session_and_context_with_rx().await;
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("log.txt"),
            "first
",
        )
        .unwrap();
        set_cwd_to_temp(&mut turn, dir.path());

        let invocation = invocation_for_write(
            session,
            turn,
            "write-call-3",
            json!({ "path": "log.txt", "content": "second
", "append": true }),
        )
        .await;
        let handler = WriteFileHandler::new(FileToolOptions::default());
        handler.handle(invocation).await.expect("write succeeds");

        let content = std::fs::read_to_string(dir.path().join("log.txt")).expect("read");
        assert_eq!(
            content,
            "first
second
"
        );
    }

    #[tokio::test]
    async fn write_file_to_project_skill_assets_with_absolute_path() {
        let (session, mut turn, _rx) = make_session_and_context_with_rx().await;
        let workspace = tempfile::tempdir().expect("tempdir");
        set_cwd_to_temp(&mut turn, workspace.path());

        let assets_dir = workspace.path().join("skills").join("src").join("assets");
        std::fs::create_dir_all(&assets_dir).expect("create assets dir");
        let target = assets_dir.join("systematic-debugging").join("SKILL.md");
        let expected_content = "updated project skill\n";

        let invocation = invocation_for_write(
            session,
            turn,
            "write-project-skill",
            json!({
                "path": target.to_string_lossy().to_string(),
                "content": expected_content
            }),
        )
        .await;
        let handler = WriteFileHandler::new(FileToolOptions::default());
        handler
            .handle(invocation)
            .await
            .expect("write to project skill assets succeeds");

        let content = std::fs::read_to_string(&target).expect("read");
        assert_eq!(content, expected_content);
    }

    #[tokio::test]
    async fn write_file_to_system_skill_directory_outside_workspace() {
        let (session, mut turn, _rx) = make_session_and_context_with_rx().await;
        let workspace = tempfile::tempdir().expect("tempdir");
        set_cwd_to_temp(&mut turn, workspace.path());

        let ody_home = turn.config.ody_home.as_path();
        let system_skill_dir = ody_home.join("skills").join(".system");
        let target = system_skill_dir
            .join("systematic-debugging")
            .join("SKILL.md");
        std::fs::create_dir_all(target.parent().unwrap()).expect("create system skill dir");
        let expected_content = "updated system skill\n";

        let invocation = invocation_for_write(
            session,
            turn,
            "write-system-skill",
            json!({
                "path": target.to_string_lossy().to_string(),
                "content": expected_content
            }),
        )
        .await;
        let handler = WriteFileHandler::new(FileToolOptions::default());
        handler
            .handle(invocation)
            .await
            .expect("write to system skill dir succeeds");

        let content = std::fs::read_to_string(&target).expect("read");
        assert_eq!(content, expected_content);
    }

    #[tokio::test]
    async fn write_file_outside_workspace_still_rejected_for_non_skill_paths() {
        let (session, mut turn, _rx) = make_session_and_context_with_rx().await;
        let workspace = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir");
        set_cwd_to_temp(&mut turn, workspace.path());

        let target = outside.path().join("evil.txt");
        let content = "should not write\n";

        let invocation = invocation_for_write(
            session,
            turn,
            "write-outside",
            json!({
                "path": target.to_string_lossy().to_string(),
                "content": content
            }),
        )
        .await;
        let handler = WriteFileHandler::new(FileToolOptions::default());
        let result = handler.handle(invocation).await;
        assert!(result.is_err(), "expected write outside workspace to fail");
        assert!(
            !target.exists(),
            "file outside workspace should not have been created"
        );
    }
}
