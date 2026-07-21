use super::write_edit::atomic_write;
use super::write_edit::file_change_for_write;
use super::write_edit::resolve_write_cwd;
use super::write_edit::resolve_write_path;
use super::write_edit::ensure_write_allowed;
use super::write_edit::single_file_change;
use crate::function_tool::FunctionCallError;
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
use ody_protocol::models::ResponseInputItem;
use ody_protocol::protocol::FileChange;
use ody_tools::ToolName;
use ody_tools::ToolSpec;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;

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
            let abs_path = resolve_write_path(
                turn.as_ref(),
                args.environment_id.as_deref(),
                &args.path,
            )
            .await?;
            let cwd = resolve_write_cwd(turn.as_ref(), args.environment_id.as_deref()).await?;
            let Some(turn_environment) =
                crate::tools::handlers::resolve_tool_environment(
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
                format!(
                    "{}{}",
                    old_content.as_deref().unwrap_or(""),
                    args.content
                )
            } else {
                args.content
            };

            atomic_write(&abs_path, new_content.as_bytes()).await?;

            let change = file_change_for_write(
                abs_path.as_path(),
                old_content.as_deref(),
                &new_content,
            );
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
                ToolEventCtx::new(
                    session.as_ref(),
                    turn.as_ref(),
                    &call_id,
                    None,
                ),
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
        assert_eq!(content, "hello
");
    }

    #[tokio::test]
    async fn write_file_overwrites_existing_file() {
        let (session, mut turn, _rx) = make_session_and_context_with_rx().await;
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("foo.txt"), "old
").unwrap();
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
        assert_eq!(content, "new
");
    }

    #[tokio::test]
    async fn write_file_appends_to_existing_file() {
        let (session, mut turn, _rx) = make_session_and_context_with_rx().await;
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("log.txt"), "first
").unwrap();
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
        assert_eq!(content, "first
second
");
    }
}
