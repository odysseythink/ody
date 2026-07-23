use super::write_edit::atomic_write;
use super::write_edit::ensure_write_allowed;
use super::write_edit::file_change_for_write;
use super::write_edit::resolve_write_cwd;
use super::write_edit::resolve_write_path;
use super::write_edit::single_file_change;
use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::events::ToolEventCtx;
use crate::tools::events::emit_direct_file_change;
use crate::tools::handlers::file_tools_spec::EDIT_FILE_TOOL_NAME;
use crate::tools::handlers::file_tools_spec::FileToolOptions;
use crate::tools::handlers::file_tools_spec::create_edit_file_tool;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use ody_protocol::models::ResponseInputItem;
use ody_protocol::protocol::FileChange;
use ody_tools::ToolName;
use ody_tools::ToolSpec;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;

#[derive(Deserialize)]
struct EditFileArgs {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
    #[serde(default)]
    environment_id: Option<String>,
}

struct EditFileOutput {
    replacements: usize,
}

impl ToolOutput for EditFileOutput {
    fn log_preview(&self) -> String {
        format!("{} replacement(s)", self.replacements)
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        FunctionToolOutput::from_text(
            format!("Applied {} replacement(s)", self.replacements),
            Some(true),
        )
        .to_response_item(call_id, payload)
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> Value {
        json!({
            "success": true,
            "replacements": self.replacements,
        })
    }
}

#[derive(Default)]
pub struct EditFileHandler {
    options: FileToolOptions,
}

impl EditFileHandler {
    pub fn new(options: FileToolOptions) -> Self {
        Self { options }
    }
}

impl ToolExecutor<ToolInvocation> for EditFileHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(EDIT_FILE_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_edit_file_tool(self.options)
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
                    "{EDIT_FILE_TOOL_NAME} handler received unsupported payload"
                )));
            };
            let args: EditFileArgs = parse_arguments(&arguments)?;
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
                    "edit_file is unavailable in this session".to_string(),
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

            let old_content = tokio::fs::read_to_string(abs_path.as_path())
                .await
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!(
                        "unable to read `{}`: {err}",
                        abs_path.as_path().display()
                    ))
                })?;

            if args.old_string.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "edit_file old_string must not be empty".to_string(),
                ));
            }
            if !old_content.contains(&args.old_string) {
                return Err(FunctionCallError::RespondToModel(format!(
                    "edit_file could not find `old_string` in `{}`",
                    abs_path.as_path().display()
                )));
            }

            let new_content = if args.replace_all {
                old_content.replace(&args.old_string, &args.new_string)
            } else {
                old_content.replacen(&args.old_string, &args.new_string, 1)
            };
            let replacements = if args.replace_all {
                old_content.matches(&args.old_string).count()
            } else {
                1
            };

            atomic_write(&abs_path, new_content.as_bytes()).await?;

            let change =
                file_change_for_write(abs_path.as_path(), Some(&old_content), &new_content);
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
                EDIT_FILE_TOOL_NAME,
                format!("Applied {replacements} replacement(s)"),
                String::new(),
                unified_diff,
            )
            .await;

            Ok(boxed_tool_output(EditFileOutput { replacements }))
        })
    }
}

impl CoreToolRuntime for EditFileHandler {}

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

    async fn invocation_for_edit(
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
            tool_name: ody_tools::ToolName::plain(EDIT_FILE_TOOL_NAME),
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
    async fn edit_file_replaces_first_occurrence() {
        let (session, mut turn, _rx) = make_session_and_context_with_rx().await;
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("config.txt"),
            "foo=1
foo=2
",
        )
        .unwrap();
        set_cwd_to_temp(&mut turn, dir.path());

        let invocation = invocation_for_edit(
            session,
            turn,
            "edit-call-1",
            json!({ "path": "config.txt", "old_string": "foo=1", "new_string": "foo=9" }),
        )
        .await;
        let handler = EditFileHandler::new(FileToolOptions::default());
        handler.handle(invocation).await.expect("edit succeeds");

        let content = std::fs::read_to_string(dir.path().join("config.txt")).expect("read");
        assert_eq!(
            content,
            "foo=9
foo=2
"
        );
    }

    #[tokio::test]
    async fn edit_file_replaces_all_occurrences() {
        let (session, mut turn, _rx) = make_session_and_context_with_rx().await;
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("items.txt"),
            "a
a
a
",
        )
        .unwrap();
        set_cwd_to_temp(&mut turn, dir.path());

        let invocation = invocation_for_edit(
            session,
            turn,
            "edit-call-2",
            json!({ "path": "items.txt", "old_string": "a", "new_string": "b", "replace_all": true }),
        )
        .await;
        let handler = EditFileHandler::new(FileToolOptions::default());
        handler.handle(invocation).await.expect("edit succeeds");

        let content = std::fs::read_to_string(dir.path().join("items.txt")).expect("read");
        assert_eq!(
            content,
            "b
b
b
"
        );
    }

    #[tokio::test]
    async fn edit_file_rejects_missing_old_string() {
        let (session, mut turn, _rx) = make_session_and_context_with_rx().await;
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("stable.txt"),
            "unchanged
",
        )
        .unwrap();
        set_cwd_to_temp(&mut turn, dir.path());

        let invocation = invocation_for_edit(
            session,
            turn,
            "edit-call-3",
            json!({ "path": "stable.txt", "old_string": "missing", "new_string": "x" }),
        )
        .await;
        let handler = EditFileHandler::new(FileToolOptions::default());
        let result = handler.handle(invocation).await;
        assert!(
            matches!(result, Err(FunctionCallError::RespondToModel(ref msg)) if msg.contains("could not find")),
            "expected 'could not find' error"
        );
    }

    #[tokio::test]
    async fn edit_file_to_system_skill_directory_outside_workspace() {
        let (session, mut turn, _rx) = make_session_and_context_with_rx().await;
        let workspace = tempfile::tempdir().expect("tempdir");
        set_cwd_to_temp(&mut turn, workspace.path());

        let ody_home = turn.config.ody_home.as_path();
        let system_skill_dir = ody_home.join("skills").join(".system");
        let target = system_skill_dir
            .join("systematic-debugging")
            .join("SKILL.md");
        std::fs::create_dir_all(target.parent().unwrap()).expect("create system skill dir");
        std::fs::write(
            &target,
            "old system skill
",
        )
        .expect("write initial content");

        let invocation = invocation_for_edit(
            session,
            turn,
            "edit-system-skill",
            json!({
                "path": target.to_string_lossy().to_string(),
                "old_string": "old system skill",
                "new_string": "updated system skill"
            }),
        )
        .await;
        let handler = EditFileHandler::new(FileToolOptions::default());
        handler
            .handle(invocation)
            .await
            .expect("edit to system skill dir succeeds");

        let content = std::fs::read_to_string(&target).expect("read");
        assert_eq!(
            content,
            "updated system skill
"
        );
    }
}
