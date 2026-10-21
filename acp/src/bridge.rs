//! ACP protocol state machine bridging client frames to an [`AgentBackend`].
//!
//! The bridge is transport-agnostic: unit tests drive the `dispatch_*` /
//! `on_backend_event` methods directly and inspect the returned frames, while
//! [`Bridge::run`] wires the same logic to stdin/stdout ndjson.

use std::collections::HashMap;
use std::io::Result as IoResult;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tracing::warn;

use crate::jsonrpc;

/// ACP protocol version served by this agent. Must match odyBox's
/// `acp.ProtocolVersion`.
pub const PROTOCOL_VERSION: i64 = 1;

/// Decision the client made for a permission prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    AllowOnce,
    AllowAlways,
    Reject,
    Cancel,
}

/// What kind of action a permission prompt guards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalKind {
    CommandExecution,
    FileChange,
    Permissions,
}

impl ApprovalKind {
    fn acp_tool_kind(self) -> &'static str {
        match self {
            ApprovalKind::CommandExecution => "execute",
            ApprovalKind::FileChange => "edit",
            ApprovalKind::Permissions => "other",
        }
    }
}

/// Terminal status of a turn, normalized from app-server `TurnStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendTurnStatus {
    Completed,
    Interrupted,
    Failed,
}

/// Backend events already normalized away from app-server protocol types.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendEvent {
    AgentMessageDelta {
        thread_id: String,
        turn_id: String,
        delta: String,
    },
    ToolCallStarted {
        thread_id: String,
        turn_id: String,
        item_id: String,
        title: String,
        kind: String,
    },
    ToolCallCompleted {
        thread_id: String,
        turn_id: String,
        item_id: String,
        success: bool,
        output: Option<String>,
    },
    TurnCompleted {
        thread_id: String,
        turn_id: String,
        status: BackendTurnStatus,
    },
    ApprovalRequested {
        token: u64,
        thread_id: String,
        item_id: String,
        title: String,
        kind: ApprovalKind,
    },
    /// Non-retryable error for an in-flight turn.
    TurnError {
        thread_id: String,
        turn_id: String,
        message: String,
    },
}

/// The agent runtime the bridge drives. Production implementation wraps the
/// in-process app-server client; tests use a scripted fake.
#[async_trait]
pub trait AgentBackend: Send {
    /// Starts a new conversation thread; returns its thread id.
    async fn start_thread(&mut self, cwd: Option<String>) -> anyhow::Result<String>;
    /// Starts one user turn; returns its turn id.
    async fn start_turn(
        &mut self,
        thread_id: &str,
        text: &str,
        cwd: Option<PathBuf>,
    ) -> anyhow::Result<String>;
    /// Interrupts an in-flight turn (best effort).
    async fn interrupt_turn(&mut self, thread_id: &str, turn_id: &str) -> anyhow::Result<()>;
    /// Resolves a previously surfaced approval request.
    async fn resolve_approval(
        &mut self,
        token: u64,
        decision: ApprovalDecision,
    ) -> anyhow::Result<()>;
    /// Next backend event, or `None` when the runtime shut down.
    async fn next_event(&mut self) -> Option<BackendEvent>;
}

#[derive(Debug)]
struct ActivePrompt {
    turn_id: String,
    request_id: Value,
    last_error: Option<String>,
}

#[derive(Debug)]
struct SessionState {
    thread_id: String,
    cwd: Option<PathBuf>,
    active_prompt: Option<ActivePrompt>,
}

pub struct Bridge<B: AgentBackend, W: AsyncWrite + Unpin> {
    backend: B,
    writer: W,
    sessions: HashMap<String, SessionState>,
    session_by_thread: HashMap<String, String>,
    pending_permissions: HashMap<i64, u64>,
    next_outgoing_id: i64,
    next_session_id: u64,
}

impl<B: AgentBackend, W: AsyncWrite + Unpin> Bridge<B, W> {
    pub fn new(backend: B, writer: W) -> Self {
        Self {
            backend,
            writer,
            sessions: HashMap::new(),
            session_by_thread: HashMap::new(),
            pending_permissions: HashMap::new(),
            next_outgoing_id: 1,
            next_session_id: 1,
        }
    }

    /// Serves ndjson frames until EOF on the reader or backend shutdown.
    pub async fn run<R: AsyncBufRead + Unpin>(&mut self, reader: R) -> IoResult<()> {
        let mut lines = reader.lines();
        loop {
            let frames = tokio::select! {
                line = lines.next_line() => {
                    match line? {
                        Some(line) if line.trim().is_empty() => continue,
                        Some(line) => self.handle_line(&line).await,
                        None => break,
                    }
                }
                event = self.backend.next_event() => {
                    match event {
                        Some(event) => self.on_backend_event(event).await,
                        None => break,
                    }
                }
            };
            self.write_frames(frames).await?;
        }
        Ok(())
    }

    async fn write_frames(&mut self, frames: Vec<Value>) -> IoResult<()> {
        for frame in frames {
            self.writer
                .write_all(&jsonrpc::encode_line(&frame))
                .await?;
        }
        self.writer.flush().await
    }

    async fn handle_line(&mut self, line: &str) -> Vec<Value> {
        match jsonrpc::parse_frame(line) {
            Ok(jsonrpc::Incoming::Request { id, method, params }) => {
                self.dispatch_request(id, &method, params).await
            }
            Ok(jsonrpc::Incoming::Notification { method, params }) => {
                self.dispatch_notification(&method, params).await
            }
            Ok(jsonrpc::Incoming::Response { id, result, error }) => {
                self.dispatch_response(&id, result, error).await
            }
            Err(jsonrpc::FrameError::Parse(message)) => {
                vec![jsonrpc::response_error(
                    &Value::Null,
                    jsonrpc::PARSE_ERROR,
                    message,
                )]
            }
            Err(jsonrpc::FrameError::Invalid(message)) => {
                vec![jsonrpc::response_error(
                    &Value::Null,
                    jsonrpc::INVALID_PARAMS,
                    message,
                )]
            }
        }
    }

    async fn dispatch_request(&mut self, id: Value, method: &str, params: Value) -> Vec<Value> {
        match method {
            "initialize" => vec![jsonrpc::response_result(&id, initialize_result())],
            "session/new" => self.handle_session_new(id, params).await,
            "session/prompt" => self.handle_session_prompt(id, params).await,
            other => vec![jsonrpc::response_error(
                &id,
                jsonrpc::METHOD_NOT_FOUND,
                format!("unknown method: {other}"),
            )],
        }
    }

    async fn dispatch_notification(&mut self, method: &str, params: Value) -> Vec<Value> {
        match method {
            "session/cancel" => self.handle_session_cancel(params).await,
            _ => Vec::new(),
        }
    }

    async fn dispatch_response(
        &mut self,
        id: &Value,
        result: Option<Value>,
        error: Option<Value>,
    ) -> Vec<Value> {
        let Some(numeric_id) = id.as_i64() else {
            return Vec::new();
        };
        let Some(token) = self.pending_permissions.remove(&numeric_id) else {
            return Vec::new();
        };
        let decision = match (result, error) {
            (Some(result), None) => permission_decision_from_result(&result),
            _ => ApprovalDecision::Cancel,
        };
        if let Err(err) = self.backend.resolve_approval(token, decision).await {
            warn!("failed to resolve approval: {err}");
        }
        Vec::new()
    }

    async fn handle_session_new(&mut self, id: Value, params: Value) -> Vec<Value> {
        let cwd = params
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::to_string);
        match self.backend.start_thread(cwd.clone()).await {
            Ok(thread_id) => {
                let session_id = format!("acp-session-{}", self.next_session_id);
                self.next_session_id += 1;
                self.session_by_thread
                    .insert(thread_id.clone(), session_id.clone());
                self.sessions.insert(
                    session_id.clone(),
                    SessionState {
                        thread_id,
                        cwd: cwd.map(PathBuf::from),
                        active_prompt: None,
                    },
                );
                vec![jsonrpc::response_result(
                    &id,
                    json!({"sessionId": session_id}),
                )]
            }
            Err(err) => vec![jsonrpc::response_error(
                &id,
                jsonrpc::INTERNAL_ERROR,
                format!("session/new failed: {err}"),
            )],
        }
    }

    async fn handle_session_prompt(&mut self, id: Value, params: Value) -> Vec<Value> {
        let Some(session_id) = params.get("sessionId").and_then(Value::as_str) else {
            return vec![jsonrpc::response_error(
                &id,
                jsonrpc::INVALID_PARAMS,
                "session/prompt requires sessionId",
            )];
        };
        let text = prompt_text(&params);
        if text.is_empty() {
            return vec![jsonrpc::response_error(
                &id,
                jsonrpc::INVALID_PARAMS,
                "session/prompt requires a non-empty text prompt",
            )];
        }
        let Some(session) = self.sessions.get_mut(session_id) else {
            return vec![jsonrpc::response_error(
                &id,
                jsonrpc::INVALID_PARAMS,
                format!("unknown session: {session_id}"),
            )];
        };
        if session.active_prompt.is_some() {
            return vec![jsonrpc::response_error(
                &id,
                jsonrpc::INTERNAL_ERROR,
                "a turn is already in progress for this session",
            )];
        }
        let thread_id = session.thread_id.clone();
        let cwd = session.cwd.clone();
        match self.backend.start_turn(&thread_id, &text, cwd).await {
            Ok(turn_id) => {
                let session = self.sessions.get_mut(session_id).expect("session exists");
                session.active_prompt = Some(ActivePrompt {
                    turn_id,
                    request_id: id,
                    last_error: None,
                });
                // The response is deferred until the turn completes.
                Vec::new()
            }
            Err(err) => vec![jsonrpc::response_error(
                &id,
                jsonrpc::INTERNAL_ERROR,
                format!("session/prompt failed: {err}"),
            )],
        }
    }

    async fn handle_session_cancel(&mut self, params: Value) -> Vec<Value> {
        let Some(session_id) = params.get("sessionId").and_then(Value::as_str) else {
            return Vec::new();
        };
        let Some(session) = self.sessions.get(session_id) else {
            return Vec::new();
        };
        let Some(active) = &session.active_prompt else {
            return Vec::new();
        };
        if let Err(err) = self
            .backend
            .interrupt_turn(&session.thread_id, &active.turn_id)
            .await
        {
            warn!("turn/interrupt failed: {err}");
        }
        Vec::new()
    }

    async fn on_backend_event(&mut self, event: BackendEvent) -> Vec<Value> {
        match event {
            BackendEvent::AgentMessageDelta {
                thread_id, delta, ..
            } => self
                .session_by_thread
                .get(&thread_id)
                .map(|session_id| {
                    vec![jsonrpc::notification(
                        "session/update",
                        json!({
                            "sessionId": session_id,
                            "update": {
                                "sessionUpdate": "agent_message_chunk",
                                "content": {"type": "text", "text": delta},
                            },
                        }),
                    )]
                })
                .unwrap_or_default(),
            BackendEvent::ToolCallStarted {
                thread_id,
                item_id,
                title,
                kind,
                ..
            } => self
                .session_by_thread
                .get(&thread_id)
                .map(|session_id| {
                    vec![jsonrpc::notification(
                        "session/update",
                        json!({
                            "sessionId": session_id,
                            "update": {
                                "sessionUpdate": "tool_call",
                                "toolCallId": item_id,
                                "title": title,
                                "kind": kind,
                                "status": "in_progress",
                            },
                        }),
                    )]
                })
                .unwrap_or_default(),
            BackendEvent::ToolCallCompleted {
                thread_id,
                item_id,
                success,
                output,
                ..
            } => self
                .session_by_thread
                .get(&thread_id)
                .map(|session_id| {
                    vec![jsonrpc::notification(
                        "session/update",
                        json!({
                            "sessionId": session_id,
                            "update": {
                                "sessionUpdate": "tool_call_update",
                                "toolCallId": item_id,
                                "status": if success { "completed" } else { "failed" },
                                "rawOutput": output,
                            },
                        }),
                    )]
                })
                .unwrap_or_default(),
            BackendEvent::TurnError {
                thread_id,
                turn_id,
                message,
            } => {
                if let Some(session) = self.session_mut_by_thread(&thread_id)
                    && let Some(active) = session.active_prompt.as_mut()
                    && active.turn_id == turn_id
                {
                    active.last_error = Some(message);
                }
                Vec::new()
            }
            BackendEvent::TurnCompleted {
                thread_id,
                turn_id,
                status,
            } => {
                let Some(session) = self.session_mut_by_thread(&thread_id) else {
                    return Vec::new();
                };
                let matches = session
                    .active_prompt
                    .as_ref()
                    .is_some_and(|active| active.turn_id == turn_id);
                if !matches {
                    return Vec::new();
                }
                let active = session.active_prompt.take().expect("checked above");
                let frame = match status {
                    BackendTurnStatus::Completed => jsonrpc::response_result(
                        &active.request_id,
                        json!({"stopReason": "end_turn"}),
                    ),
                    BackendTurnStatus::Interrupted => jsonrpc::response_result(
                        &active.request_id,
                        json!({"stopReason": "cancelled"}),
                    ),
                    BackendTurnStatus::Failed => jsonrpc::response_error(
                        &active.request_id,
                        jsonrpc::INTERNAL_ERROR,
                        active.last_error.unwrap_or_else(|| "turn failed".to_string()),
                    ),
                };
                vec![frame]
            }
            BackendEvent::ApprovalRequested {
                token,
                thread_id,
                item_id,
                title,
                kind,
            } => {
                let Some(session_id) = self.session_by_thread.get(&thread_id).cloned() else {
                    // No ACP session owns this thread; decline so the agent is
                    // not stuck waiting on a prompt nobody can answer.
                    if let Err(err) = self
                        .backend
                        .resolve_approval(token, ApprovalDecision::Cancel)
                        .await
                    {
                        warn!("failed to decline orphan approval: {err}");
                    }
                    return Vec::new();
                };
                let outgoing_id = self.next_outgoing_id;
                self.next_outgoing_id += 1;
                self.pending_permissions.insert(outgoing_id, token);
                vec![jsonrpc::request(
                    outgoing_id,
                    "session/request_permission",
                    json!({
                        "sessionId": session_id,
                        "toolCall": {
                            "toolCallId": item_id,
                            "title": title,
                            "kind": kind.acp_tool_kind(),
                            "status": "pending",
                        },
                        "options": [
                            {"optionId": "allow_once", "name": "Allow once", "kind": "allow_once"},
                            {"optionId": "allow_always", "name": "Allow always", "kind": "allow_always"},
                            {"optionId": "reject_once", "name": "Reject", "kind": "reject_once"},
                        ],
                    }),
                )]
            }
        }
    }

    fn session_mut_by_thread(&mut self, thread_id: &str) -> Option<&mut SessionState> {
        let session_id = self.session_by_thread.get(thread_id)?.clone();
        self.sessions.get_mut(&session_id)
    }
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "agentCapabilities": {
            "loadSession": false,
            "promptCapabilities": {
                "embeddedContext": true,
            },
        },
        "agentInfo": {
            "name": "ody",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "authMethods": [],
    })
}

fn prompt_text(params: &Value) -> String {
    params
        .get("prompt")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn permission_decision_from_result(result: &Value) -> ApprovalDecision {
    let outcome = result.get("outcome");
    match outcome
        .and_then(|outcome| outcome.get("outcome"))
        .and_then(Value::as_str)
    {
        Some("selected") => match outcome
            .and_then(|outcome| outcome.get("optionId"))
            .and_then(Value::as_str)
        {
            Some("allow_once") => ApprovalDecision::AllowOnce,
            Some("allow_always") => ApprovalDecision::AllowAlways,
            _ => ApprovalDecision::Reject,
        },
        _ => ApprovalDecision::Cancel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        StartThread { cwd: Option<String> },
        StartTurn { thread_id: String, text: String },
        Interrupt { thread_id: String, turn_id: String },
        ResolveApproval { token: u64, decision: ApprovalDecision },
    }

    struct FakeBackend {
        calls: Arc<Mutex<Vec<Call>>>,
        events: VecDeque<BackendEvent>,
    }

    impl FakeBackend {
        fn new() -> (Self, Arc<Mutex<Vec<Call>>>) {
            let calls = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    calls: Arc::clone(&calls),
                    events: VecDeque::new(),
                },
                calls,
            )
        }

        fn with_events(mut self, events: Vec<BackendEvent>) -> Self {
            self.events = events.into_iter().collect();
            self
        }
    }

    #[async_trait]
    impl AgentBackend for FakeBackend {
        async fn start_thread(&mut self, cwd: Option<String>) -> anyhow::Result<String> {
            self.calls
                .lock()
                .expect("calls")
                .push(Call::StartThread { cwd });
            Ok("thread-1".to_string())
        }

        async fn start_turn(
            &mut self,
            thread_id: &str,
            text: &str,
            _cwd: Option<PathBuf>,
        ) -> anyhow::Result<String> {
            self.calls.lock().expect("calls").push(Call::StartTurn {
                thread_id: thread_id.to_string(),
                text: text.to_string(),
            });
            Ok("turn-1".to_string())
        }

        async fn interrupt_turn(&mut self, thread_id: &str, turn_id: &str) -> anyhow::Result<()> {
            self.calls.lock().expect("calls").push(Call::Interrupt {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
            });
            Ok(())
        }

        async fn resolve_approval(
            &mut self,
            token: u64,
            decision: ApprovalDecision,
        ) -> anyhow::Result<()> {
            self.calls
                .lock()
                .expect("calls")
                .push(Call::ResolveApproval { token, decision });
            Ok(())
        }

        async fn next_event(&mut self) -> Option<BackendEvent> {
            self.events.pop_front()
        }
    }

    fn test_bridge() -> Bridge<FakeBackend, Vec<u8>> {
        let (backend, _) = FakeBackend::new();
        Bridge::new(backend, Vec::new())
    }

    async fn new_session(bridge: &mut Bridge<FakeBackend, Vec<u8>>) -> String {
        let frames = bridge
            .dispatch_request(json!(2), "session/new", json!({"cwd": "E:/work", "mcpServers": []}))
            .await;
        assert_eq!(frames.len(), 1);
        frames[0]["result"]["sessionId"]
            .as_str()
            .expect("sessionId")
            .to_string()
    }

    #[tokio::test]
    async fn initialize_advertises_protocol_v1() {
        let mut bridge = test_bridge();
        let frames = bridge
            .dispatch_request(
                json!(1),
                "initialize",
                json!({"protocolVersion": 1, "clientCapabilities": {}, "clientInfo": {"name": "odyBox", "version": "0.1.0"}}),
            )
            .await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["id"], json!(1));
        assert_eq!(frames[0]["result"]["protocolVersion"], json!(1));
        assert_eq!(frames[0]["result"]["agentInfo"]["name"], json!("ody"));
        assert_eq!(
            frames[0]["result"]["agentCapabilities"]["loadSession"],
            json!(false)
        );
    }

    #[tokio::test]
    async fn session_new_starts_thread_with_cwd() {
        let (backend, calls) = FakeBackend::new();
        let mut bridge = Bridge::new(backend, Vec::new());
        let session_id = new_session(&mut bridge).await;
        assert!(!session_id.is_empty());
        assert_eq!(
            calls.lock().expect("calls").as_slice(),
            &[Call::StartThread {
                cwd: Some("E:/work".to_string())
            }]
        );
    }

    #[tokio::test]
    async fn prompt_streams_chunks_then_answers_end_turn() {
        let mut bridge = test_bridge();
        let session_id = new_session(&mut bridge).await;

        let frames = bridge
            .dispatch_request(
                json!(3),
                "session/prompt",
                json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "hi"}]}),
            )
            .await;
        assert!(frames.is_empty(), "prompt response is deferred");

        let frames = bridge
            .on_backend_event(BackendEvent::AgentMessageDelta {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
                delta: "hello".to_string(),
            })
            .await;
        assert_eq!(
            frames,
            vec![json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": session_id,
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": "hello"},
                    },
                },
            })]
        );

        let frames = bridge
            .on_backend_event(BackendEvent::TurnCompleted {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
                status: BackendTurnStatus::Completed,
            })
            .await;
        assert_eq!(
            frames,
            vec![json!({
                "jsonrpc": "2.0",
                "id": 3,
                "result": {"stopReason": "end_turn"},
            })]
        );
    }

    #[tokio::test]
    async fn cancel_interrupts_turn_and_reports_cancelled() {
        let (backend, calls) = FakeBackend::new();
        let mut bridge = Bridge::new(backend, Vec::new());
        let session_id = new_session(&mut bridge).await;
        bridge
            .dispatch_request(
                json!(3),
                "session/prompt",
                json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "hi"}]}),
            )
            .await;

        let frames = bridge
            .dispatch_notification("session/cancel", json!({"sessionId": session_id}))
            .await;
        assert!(frames.is_empty());
        assert_eq!(
            calls.lock().expect("calls").last(),
            Some(&Call::Interrupt {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
            })
        );

        let frames = bridge
            .on_backend_event(BackendEvent::TurnCompleted {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
                status: BackendTurnStatus::Interrupted,
            })
            .await;
        assert_eq!(
            frames,
            vec![json!({
                "jsonrpc": "2.0",
                "id": 3,
                "result": {"stopReason": "cancelled"},
            })]
        );
    }

    #[tokio::test]
    async fn second_prompt_during_active_turn_is_rejected() {
        let mut bridge = test_bridge();
        let session_id = new_session(&mut bridge).await;
        bridge
            .dispatch_request(
                json!(3),
                "session/prompt",
                json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "hi"}]}),
            )
            .await;
        let frames = bridge
            .dispatch_request(
                json!(4),
                "session/prompt",
                json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "again"}]}),
            )
            .await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["id"], json!(4));
        assert!(frames[0]["error"].is_object());
    }

    #[tokio::test]
    async fn prompt_for_unknown_session_is_invalid_params() {
        let mut bridge = test_bridge();
        let frames = bridge
            .dispatch_request(
                json!(3),
                "session/prompt",
                json!({"sessionId": "nope", "prompt": [{"type": "text", "text": "hi"}]}),
            )
            .await;
        assert_eq!(frames[0]["error"]["code"], json!(jsonrpc::INVALID_PARAMS));
    }

    #[tokio::test]
    async fn turn_failure_surfaces_stashed_error_message() {
        let mut bridge = test_bridge();
        let session_id = new_session(&mut bridge).await;
        bridge
            .dispatch_request(
                json!(3),
                "session/prompt",
                json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "hi"}]}),
            )
            .await;
        bridge
            .on_backend_event(BackendEvent::TurnError {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
                message: "provider exploded".to_string(),
            })
            .await;
        let frames = bridge
            .on_backend_event(BackendEvent::TurnCompleted {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
                status: BackendTurnStatus::Failed,
            })
            .await;
        assert_eq!(frames[0]["id"], json!(3));
        assert_eq!(
            frames[0]["error"]["message"],
            json!("provider exploded")
        );
    }

    #[tokio::test]
    async fn tool_call_events_become_acp_tool_updates() {
        let mut bridge = test_bridge();
        let session_id = new_session(&mut bridge).await;
        bridge
            .dispatch_request(
                json!(3),
                "session/prompt",
                json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "hi"}]}),
            )
            .await;

        let frames = bridge
            .on_backend_event(BackendEvent::ToolCallStarted {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
                item_id: "item-9".to_string(),
                title: "cargo test".to_string(),
                kind: "execute".to_string(),
            })
            .await;
        assert_eq!(
            frames[0]["params"]["update"],
            json!({
                "sessionUpdate": "tool_call",
                "toolCallId": "item-9",
                "title": "cargo test",
                "kind": "execute",
                "status": "in_progress",
            })
        );
        assert_eq!(frames[0]["params"]["sessionId"], json!(session_id));

        let frames = bridge
            .on_backend_event(BackendEvent::ToolCallCompleted {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
                item_id: "item-9".to_string(),
                success: true,
                output: Some("ok".to_string()),
            })
            .await;
        assert_eq!(
            frames[0]["params"]["update"],
            json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "item-9",
                "status": "completed",
                "rawOutput": "ok",
            })
        );
    }

    #[tokio::test]
    async fn approval_request_roundtrips_through_acp_permission_prompt() {
        let (backend, calls) = FakeBackend::new();
        let mut bridge = Bridge::new(backend, Vec::new());
        let session_id = new_session(&mut bridge).await;
        bridge
            .dispatch_request(
                json!(3),
                "session/prompt",
                json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "hi"}]}),
            )
            .await;

        let frames = bridge
            .on_backend_event(BackendEvent::ApprovalRequested {
                token: 42,
                thread_id: "thread-1".to_string(),
                item_id: "item-9".to_string(),
                title: "cargo test".to_string(),
                kind: ApprovalKind::CommandExecution,
            })
            .await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["method"], json!("session/request_permission"));
        let outgoing_id = frames[0]["id"].as_i64().expect("numeric id");
        assert_eq!(frames[0]["params"]["sessionId"], json!(session_id));
        assert_eq!(
            frames[0]["params"]["toolCall"],
            json!({
                "toolCallId": "item-9",
                "title": "cargo test",
                "kind": "execute",
                "status": "pending",
            })
        );
        let options = frames[0]["params"]["options"]
            .as_array()
            .expect("options");
        assert_eq!(options.len(), 3);
        assert_eq!(options[0]["optionId"], json!("allow_once"));

        // Client selects "allow_always".
        let frames = bridge
            .dispatch_response(
                &json!(outgoing_id),
                Some(json!({"outcome": {"outcome": "selected", "optionId": "allow_always"}})),
                None,
            )
            .await;
        assert!(frames.is_empty());
        assert_eq!(
            calls.lock().expect("calls").last(),
            Some(&Call::ResolveApproval {
                token: 42,
                decision: ApprovalDecision::AllowAlways,
            })
        );
    }

    #[tokio::test]
    async fn cancelled_permission_maps_to_cancel_decision() {
        let (backend, calls) = FakeBackend::new();
        let mut bridge = Bridge::new(backend, Vec::new());
        new_session(&mut bridge).await;

        let frames = bridge
            .on_backend_event(BackendEvent::ApprovalRequested {
                token: 7,
                thread_id: "thread-1".to_string(),
                item_id: "item-1".to_string(),
                title: "edit file".to_string(),
                kind: ApprovalKind::FileChange,
            })
            .await;
        let outgoing_id = frames[0]["id"].as_i64().expect("numeric id");
        assert_eq!(
            frames[0]["params"]["toolCall"]["kind"],
            json!("edit")
        );

        bridge
            .dispatch_response(
                &json!(outgoing_id),
                Some(json!({"outcome": {"outcome": "cancelled"}})),
                None,
            )
            .await;
        assert_eq!(
            calls.lock().expect("calls").last(),
            Some(&Call::ResolveApproval {
                token: 7,
                decision: ApprovalDecision::Cancel,
            })
        );
    }

    #[tokio::test]
    async fn orphan_approval_is_declined_without_prompt() {
        let (backend, calls) = FakeBackend::new();
        let mut bridge = Bridge::new(backend, Vec::new());
        let frames = bridge
            .on_backend_event(BackendEvent::ApprovalRequested {
                token: 5,
                thread_id: "thread-unknown".to_string(),
                item_id: "item-1".to_string(),
                title: "cmd".to_string(),
                kind: ApprovalKind::CommandExecution,
            })
            .await;
        assert!(frames.is_empty());
        assert_eq!(
            calls.lock().expect("calls").last(),
            Some(&Call::ResolveApproval {
                token: 5,
                decision: ApprovalDecision::Cancel,
            })
        );
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let mut bridge = test_bridge();
        let frames = bridge
            .dispatch_request(json!(9), "session/load", json!({}))
            .await;
        assert_eq!(frames[0]["error"]["code"], json!(jsonrpc::METHOD_NOT_FOUND));
    }

    #[tokio::test]
    async fn events_for_unknown_threads_are_dropped() {
        let mut bridge = test_bridge();
        let frames = bridge
            .on_backend_event(BackendEvent::AgentMessageDelta {
                thread_id: "thread-unknown".to_string(),
                turn_id: "turn-x".to_string(),
                delta: "noise".to_string(),
            })
            .await;
        assert!(frames.is_empty());
    }
}
