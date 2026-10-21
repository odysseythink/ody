//! Production [`AgentBackend`] wrapping the in-process app-server client.
//!
//! Owns the translation between app-server v1 protocol types and the
//! normalized [`BackendEvent`] stream consumed by the ACP bridge. Event
//! translation helpers are pure functions so they can be tested without a
//! running runtime.

use std::collections::HashMap;
use std::io::Result as IoResult;
use std::path::PathBuf;

use async_trait::async_trait;
use ody_app_server_client::InProcessAppServerClient;
use ody_app_server_client::InProcessClientStartArgs;
use ody_app_server_client::InProcessServerEvent;
use ody_app_server_protocol::ClientRequest;
use ody_app_server_protocol::CommandExecutionApprovalDecision;
use ody_app_server_protocol::CommandExecutionRequestApprovalResponse;
use ody_app_server_protocol::CommandExecutionStatus;
use ody_app_server_protocol::CurrentTimeReadResponse;
use ody_app_server_protocol::FileChangeApprovalDecision;
use ody_app_server_protocol::FileChangeRequestApprovalResponse;
use ody_app_server_protocol::GrantedPermissionProfile;
use ody_app_server_protocol::JSONRPCErrorError;
use ody_app_server_protocol::McpToolCallStatus;
use ody_app_server_protocol::PatchApplyStatus;
use ody_app_server_protocol::PermissionGrantScope;
use ody_app_server_protocol::PermissionsRequestApprovalResponse;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::RequestPermissionProfile;
use ody_app_server_protocol::ServerNotification;
use ody_app_server_protocol::ServerRequest;
use ody_app_server_protocol::ThreadItem;
use ody_app_server_protocol::ThreadStartParams;
use ody_app_server_protocol::ThreadStartResponse;
use ody_app_server_protocol::TurnInterruptParams;
use ody_app_server_protocol::TurnInterruptResponse;
use ody_app_server_protocol::TurnStartParams;
use ody_app_server_protocol::TurnStartResponse;
use ody_app_server_protocol::TurnStatus;
use ody_app_server_protocol::UserInput;
use tracing::warn;

use crate::bridge::AgentBackend;
use crate::bridge::ApprovalDecision;
use crate::bridge::ApprovalKind;
use crate::bridge::BackendEvent;
use crate::bridge::BackendTurnStatus;

/// A server-initiated approval request parked until the ACP client answers.
enum PendingApproval {
    CommandExecution(RequestId),
    FileChange(RequestId),
    Permissions {
        request_id: RequestId,
        requested: RequestPermissionProfile,
    },
}

pub struct AppServerBackend {
    client: InProcessAppServerClient,
    next_request_id: i64,
    next_token: u64,
    pending_approvals: HashMap<u64, PendingApproval>,
}

impl AppServerBackend {
    pub async fn start(args: InProcessClientStartArgs) -> IoResult<Self> {
        let client = InProcessAppServerClient::start(args).await?;
        Ok(Self {
            client,
            next_request_id: 1,
            next_token: 1,
            pending_approvals: HashMap::new(),
        })
    }

    fn request_id(&mut self) -> RequestId {
        let id = self.next_request_id;
        self.next_request_id += 1;
        RequestId::Integer(id)
    }

    async fn reject_request(&self, request_id: RequestId, message: String) {
        if let Err(err) = self
            .client
            .reject_server_request(
                request_id,
                JSONRPCErrorError {
                    code: -32000,
                    message,
                    data: None,
                },
            )
            .await
        {
            warn!("failed to reject server request: {err}");
        }
    }

    fn register_approval(&mut self, pending: PendingApproval) -> u64 {
        let token = self.next_token;
        self.next_token += 1;
        self.pending_approvals.insert(token, pending);
        token
    }

    /// Converts one in-process event into a normalized backend event, handling
    /// requests that can be answered inline. Returns `None` for events the ACP
    /// bridge does not care about.
    async fn convert_event(&mut self, event: InProcessServerEvent) -> Option<BackendEvent> {
        match event {
            InProcessServerEvent::Lagged { skipped } => {
                warn!("app-server event stream lagged; skipped {skipped} events");
                None
            }
            InProcessServerEvent::ServerNotification(notification) => {
                convert_notification(notification)
            }
            InProcessServerEvent::ServerRequest(request) => self.convert_request(request).await,
        }
    }

    async fn convert_request(&mut self, request: ServerRequest) -> Option<BackendEvent> {
        match request {
            ServerRequest::CommandExecutionRequestApproval { request_id, params } => {
                let title = params
                    .command
                    .clone()
                    .or(params.reason.clone())
                    .unwrap_or_else(|| "command execution".to_string());
                let thread_id = params.thread_id.clone();
                let item_id = params.item_id.clone();
                let token = self.register_approval(PendingApproval::CommandExecution(request_id));
                Some(BackendEvent::ApprovalRequested {
                    token,
                    thread_id,
                    item_id,
                    title,
                    kind: ApprovalKind::CommandExecution,
                })
            }
            ServerRequest::FileChangeRequestApproval { request_id, params } => {
                let title = params
                    .reason
                    .clone()
                    .unwrap_or_else(|| "file change".to_string());
                let thread_id = params.thread_id.clone();
                let item_id = params.item_id.clone();
                let token = self.register_approval(PendingApproval::FileChange(request_id));
                Some(BackendEvent::ApprovalRequested {
                    token,
                    thread_id,
                    item_id,
                    title,
                    kind: ApprovalKind::FileChange,
                })
            }
            ServerRequest::PermissionsRequestApproval { request_id, params } => {
                let title = params
                    .reason
                    .clone()
                    .unwrap_or_else(|| "additional permissions".to_string());
                let thread_id = params.thread_id.clone();
                let item_id = params.item_id.clone();
                let token = self.register_approval(PendingApproval::Permissions {
                    request_id,
                    requested: params.permissions.clone(),
                });
                Some(BackendEvent::ApprovalRequested {
                    token,
                    thread_id,
                    item_id,
                    title,
                    kind: ApprovalKind::Permissions,
                })
            }
            ServerRequest::CurrentTimeRead { request_id, .. } => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_secs() as i64)
                    .unwrap_or_default();
                let value = serde_json::to_value(CurrentTimeReadResponse {
                    current_time_at: now,
                })
                .unwrap_or_default();
                if let Err(err) = self.client.resolve_server_request(request_id, value).await {
                    warn!("failed to answer currentTime/read: {err}");
                }
                None
            }
            other => {
                let method = server_request_method_name(&other);
                self.reject_request(
                    other.id().clone(),
                    format!("{method} is not supported by the ACP frontend"),
                )
                .await;
                None
            }
        }
    }
}

#[async_trait]
impl AgentBackend for AppServerBackend {
    async fn start_thread(&mut self, cwd: Option<String>) -> anyhow::Result<String> {
        let params = ThreadStartParams {
            cwd,
            ..ThreadStartParams::default()
        };
        let request_id = self.request_id();
        let response: ThreadStartResponse = self
            .client
            .request_typed(ClientRequest::ThreadStart {
                request_id,
                params,
            })
            .await
            .map_err(|err| anyhow::anyhow!("thread/start failed: {err}"))?;
        Ok(response.thread.id)
    }

    async fn start_turn(
        &mut self,
        thread_id: &str,
        text: &str,
        cwd: Option<PathBuf>,
    ) -> anyhow::Result<String> {
        let request_id = self.request_id();
        let response: TurnStartResponse = self
            .client
            .request_typed(ClientRequest::TurnStart {
                request_id,
                params: TurnStartParams {
                    thread_id: thread_id.to_string(),
                    client_user_message_id: None,
                    input: vec![UserInput::Text {
                        text: text.to_string(),
                        text_elements: Vec::new(),
                    }],
                    responsesapi_client_metadata: None,
                    additional_context: None,
                    environments: None,
                    cwd,
                    runtime_workspace_roots: None,
                    approval_policy: None,
                    approvals_reviewer: None,
                    sandbox_policy: None,
                    permissions: None,
                    model: None,
                    service_tier: None,
                    effort: None,
                    summary: None,
                    personality: None,
                    output_schema: None,
                    collaboration_mode: None,
                    multi_agent_mode: None,
                },
            })
            .await
            .map_err(|err| anyhow::anyhow!("turn/start failed: {err}"))?;
        Ok(response.turn.id)
    }

    async fn interrupt_turn(&mut self, thread_id: &str, turn_id: &str) -> anyhow::Result<()> {
        let request_id = self.request_id();
        let _: TurnInterruptResponse = self
            .client
            .request_typed(ClientRequest::TurnInterrupt {
                request_id,
                params: TurnInterruptParams {
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                },
            })
            .await
            .map_err(|err| anyhow::anyhow!("turn/interrupt failed: {err}"))?;
        Ok(())
    }

    async fn resolve_approval(
        &mut self,
        token: u64,
        decision: ApprovalDecision,
    ) -> anyhow::Result<()> {
        let Some(pending) = self.pending_approvals.remove(&token) else {
            warn!("approval token {token} is not pending");
            return Ok(());
        };
        match pending {
            PendingApproval::CommandExecution(request_id) => {
                let decision = match decision {
                    ApprovalDecision::AllowOnce => CommandExecutionApprovalDecision::Accept,
                    ApprovalDecision::AllowAlways => {
                        CommandExecutionApprovalDecision::AcceptForSession
                    }
                    ApprovalDecision::Reject => CommandExecutionApprovalDecision::Decline,
                    ApprovalDecision::Cancel => CommandExecutionApprovalDecision::Cancel,
                };
                self.resolve_value(
                    request_id,
                    serde_json::to_value(CommandExecutionRequestApprovalResponse {
                        decision,
                    })?,
                )
                .await
            }
            PendingApproval::FileChange(request_id) => {
                let decision = match decision {
                    ApprovalDecision::AllowOnce => FileChangeApprovalDecision::Accept,
                    ApprovalDecision::AllowAlways => FileChangeApprovalDecision::AcceptForSession,
                    ApprovalDecision::Reject => FileChangeApprovalDecision::Decline,
                    ApprovalDecision::Cancel => FileChangeApprovalDecision::Cancel,
                };
                self.resolve_value(
                    request_id,
                    serde_json::to_value(FileChangeRequestApprovalResponse { decision })?,
                )
                .await
            }
            PendingApproval::Permissions {
                request_id,
                requested,
            } => match decision {
                ApprovalDecision::AllowOnce | ApprovalDecision::AllowAlways => {
                    let scope = match decision {
                        ApprovalDecision::AllowAlways => PermissionGrantScope::Session,
                        _ => PermissionGrantScope::Turn,
                    };
                    self.resolve_value(
                        request_id,
                        serde_json::to_value(PermissionsRequestApprovalResponse {
                            permissions: granted_profile_from_request(requested),
                            scope,
                            strict_auto_review: None,
                        })?,
                    )
                    .await
                }
                // Rejecting leaves the turn running without the extra
                // permissions; the server treats a client error as "grant
                // nothing and continue".
                ApprovalDecision::Reject | ApprovalDecision::Cancel => {
                    self.reject_request(
                        request_id,
                        "permission request declined by the user".to_string(),
                    )
                    .await;
                    Ok(())
                }
            },
        }
    }

    async fn next_event(&mut self) -> Option<BackendEvent> {
        loop {
            let event = self.client.next_event().await?;
            if let Some(converted) = self.convert_event(event).await {
                return Some(converted);
            }
        }
    }
}

impl AppServerBackend {
    async fn resolve_value(
        &self,
        request_id: RequestId,
        value: serde_json::Value,
    ) -> anyhow::Result<()> {
        self.client
            .resolve_server_request(request_id, value)
            .await
            .map_err(|err| anyhow::anyhow!("failed to resolve server request: {err}"))
    }
}

fn server_request_method_name(request: &ServerRequest) -> String {
    serde_json::to_value(request)
        .ok()
        .and_then(|value| {
            value
                .get("method")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Grants exactly what was requested (mirrors the TUI's conversion).
fn granted_profile_from_request(requested: RequestPermissionProfile) -> GrantedPermissionProfile {
    GrantedPermissionProfile {
        network: requested.network,
        file_system: requested.file_system,
    }
}

/// Translates server notifications into normalized backend events.
fn convert_notification(notification: ServerNotification) -> Option<BackendEvent> {
    match notification {
        ServerNotification::AgentMessageDelta(payload) => Some(BackendEvent::AgentMessageDelta {
            thread_id: payload.thread_id,
            turn_id: payload.turn_id,
            delta: payload.delta,
        }),
        ServerNotification::ItemStarted(payload) => {
            let (item_id, title, kind) = tool_call_started(&payload.item)?;
            Some(BackendEvent::ToolCallStarted {
                thread_id: payload.thread_id,
                turn_id: payload.turn_id,
                item_id,
                title,
                kind,
            })
        }
        ServerNotification::ItemCompleted(payload) => {
            let (item_id, success, output) = tool_call_completed(&payload.item)?;
            Some(BackendEvent::ToolCallCompleted {
                thread_id: payload.thread_id,
                turn_id: payload.turn_id,
                item_id,
                success,
                output,
            })
        }
        ServerNotification::TurnCompleted(payload) => {
            let status = match payload.turn.status {
                TurnStatus::Completed => BackendTurnStatus::Completed,
                TurnStatus::Interrupted => BackendTurnStatus::Interrupted,
                TurnStatus::Failed | TurnStatus::InProgress => BackendTurnStatus::Failed,
            };
            Some(BackendEvent::TurnCompleted {
                thread_id: payload.thread_id,
                turn_id: payload.turn.id,
                status,
            })
        }
        ServerNotification::Error(payload) if !payload.will_retry => Some(BackendEvent::TurnError {
            thread_id: payload.thread_id,
            turn_id: payload.turn_id,
            message: payload.error.message,
        }),
        _ => None,
    }
}

/// Extracts ACP tool-call metadata from a started item.
fn tool_call_started(item: &ThreadItem) -> Option<(String, String, String)> {
    match item {
        ThreadItem::CommandExecution { id, command, .. } => {
            Some((id.clone(), command.clone(), "execute".to_string()))
        }
        ThreadItem::FileChange { id, changes, .. } => Some((
            id.clone(),
            file_change_title(changes),
            "edit".to_string(),
        )),
        ThreadItem::McpToolCall {
            id, server, tool, ..
        } => Some((id.clone(), format!("{server}/{tool}"), "other".to_string())),
        ThreadItem::DynamicToolCall { id, tool, .. } => {
            Some((id.clone(), tool.clone(), "other".to_string()))
        }
        _ => None,
    }
}

/// Extracts completion status from a completed item.
fn tool_call_completed(item: &ThreadItem) -> Option<(String, bool, Option<String>)> {
    match item {
        ThreadItem::CommandExecution {
            id,
            status,
            exit_code,
            aggregated_output,
            ..
        } => {
            let success = matches!(status, CommandExecutionStatus::Completed)
                && exit_code.is_none_or(|code| code == 0);
            Some((id.clone(), success, aggregated_output.clone()))
        }
        ThreadItem::FileChange { id, status, changes } => {
            let success = matches!(status, PatchApplyStatus::Completed);
            Some((id.clone(), success, Some(file_change_title(changes))))
        }
        ThreadItem::McpToolCall {
            id,
            status,
            result,
            error,
            ..
        } => {
            let success = matches!(status, McpToolCallStatus::Completed) && error.is_none();
            let output = error
                .as_ref()
                .map(|error| error.message.clone())
                .or_else(|| {
                    result.as_ref().and_then(|result| {
                        serde_json::to_string(&result.content).ok()
                    })
                });
            Some((id.clone(), success, output))
        }
        ThreadItem::DynamicToolCall { id, success, .. } => {
            Some((id.clone(), success.unwrap_or(false), None))
        }
        _ => None,
    }
}

fn file_change_title(changes: &[ody_app_server_protocol::FileUpdateChange]) -> String {
    let paths: Vec<&str> = changes.iter().map(|change| change.path.as_str()).collect();
    format!("edit {}", paths.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_app_server_protocol::FileUpdateChange;
    use ody_app_server_protocol::PatchChangeKind;
    use pretty_assertions::assert_eq;

    fn command_execution_item(status: CommandExecutionStatus) -> ThreadItem {
        ThreadItem::CommandExecution {
            id: "item-1".to_string(),
            command: "cargo test".to_string(),
            cwd: ody_utils_absolute_path::AbsolutePathBuf::try_from(PathBuf::from("E:/work"))
                .expect("absolute path")
                .into(),
            process_id: None,
            source: Default::default(),
            status,
            command_actions: Vec::new(),
            aggregated_output: Some("all tests passed".to_string()),
            exit_code: Some(0),
            duration_ms: None,
        }
    }

    #[test]
    fn started_command_execution_maps_to_execute_tool_call() {
        let item = command_execution_item(CommandExecutionStatus::InProgress);
        assert_eq!(
            tool_call_started(&item),
            Some((
                "item-1".to_string(),
                "cargo test".to_string(),
                "execute".to_string()
            ))
        );
    }

    #[test]
    fn completed_command_execution_reports_success_and_output() {
        let item = command_execution_item(CommandExecutionStatus::Completed);
        assert_eq!(
            tool_call_completed(&item),
            Some((
                "item-1".to_string(),
                true,
                Some("all tests passed".to_string())
            ))
        );
    }

    #[test]
    fn failed_command_execution_reports_failure() {
        let mut item = command_execution_item(CommandExecutionStatus::Failed);
        if let ThreadItem::CommandExecution { exit_code, .. } = &mut item {
            *exit_code = Some(1);
        }
        let (id, success, _) = tool_call_completed(&item).expect("tool call");
        assert_eq!(id, "item-1");
        assert!(!success);
    }

    #[test]
    fn file_change_maps_to_edit_tool_call() {
        let item = ThreadItem::FileChange {
            id: "item-2".to_string(),
            changes: vec![FileUpdateChange {
                path: "src/main.rs".to_string(),
                kind: PatchChangeKind::Update { move_path: None },
                diff: "@@".to_string(),
            }],
            status: PatchApplyStatus::Completed,
        };
        assert_eq!(
            tool_call_started(&item),
            Some((
                "item-2".to_string(),
                "edit src/main.rs".to_string(),
                "edit".to_string()
            ))
        );
        assert_eq!(
            tool_call_completed(&item),
            Some((
                "item-2".to_string(),
                true,
                Some("edit src/main.rs".to_string())
            ))
        );
    }

    #[test]
    fn agent_message_items_are_not_tool_calls() {
        let item = ThreadItem::AgentMessage {
            id: "item-3".to_string(),
            text: "hello".to_string(),
            phase: None,
            memory_citation: None,
        };
        assert_eq!(tool_call_started(&item), None);
        assert_eq!(tool_call_completed(&item), None);
    }

    #[test]
    fn turn_completed_maps_status() {
        let notification = ServerNotification::TurnCompleted(
            ody_app_server_protocol::TurnCompletedNotification {
                thread_id: "thread-1".to_string(),
                turn: ody_app_server_protocol::Turn {
                    id: "turn-1".to_string(),
                    items: Vec::new(),
                    items_view: Default::default(),
                    status: TurnStatus::Interrupted,
                    error: None,
                    started_at: None,
                    completed_at: None,
                    duration_ms: None,
                },
            },
        );
        assert_eq!(
            convert_notification(notification),
            Some(BackendEvent::TurnCompleted {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
                status: BackendTurnStatus::Interrupted,
            })
        );
    }

    #[test]
    fn retryable_errors_are_ignored() {
        let notification =
            ServerNotification::Error(ody_app_server_protocol::ErrorNotification {
                error: ody_app_server_protocol::TurnError {
                    message: "transient".to_string(),
                    ody_error_info: None,
                    additional_details: None,
                },
                will_retry: true,
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
            });
        assert_eq!(convert_notification(notification), None);
    }

    #[test]
    fn fatal_errors_become_turn_error_events() {
        let notification =
            ServerNotification::Error(ody_app_server_protocol::ErrorNotification {
                error: ody_app_server_protocol::TurnError {
                    message: "boom".to_string(),
                    ody_error_info: None,
                    additional_details: None,
                },
                will_retry: false,
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
            });
        assert_eq!(
            convert_notification(notification),
            Some(BackendEvent::TurnError {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
                message: "boom".to_string(),
            })
        );
    }
}
