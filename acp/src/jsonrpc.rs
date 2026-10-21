//! Minimal newline-delimited JSON-RPC 2.0 framing for the ACP wire.
//!
//! Hand-rolled on `serde_json::Value` because the ACP surface we serve is
//! narrow (odyBox is the only client) and pulling in a full JSON-RPC or ACP
//! SDK would add churn without buying safety we need.

use serde_json::Value;
use serde_json::json;

pub const PARSE_ERROR: i64 = -32700;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

/// One inbound frame read from the client.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// A request expecting a response (`id` echoed back verbatim).
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// A notification (no `id`).
    Notification { method: String, params: Value },
    /// A response to a request we sent to the client.
    Response {
        id: Value,
        result: Option<Value>,
        error: Option<Value>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum FrameError {
    /// Line was not valid JSON.
    Parse(String),
    /// JSON was valid but not a JSON-RPC frame we understand.
    Invalid(String),
}

/// Parses one ndjson line into an inbound frame.
pub fn parse_frame(line: &str) -> Result<Incoming, FrameError> {
    let value: Value =
        serde_json::from_str(line).map_err(|err| FrameError::Parse(err.to_string()))?;
    let obj = value
        .as_object()
        .ok_or_else(|| FrameError::Invalid("frame is not a JSON object".to_string()))?;

    let method = obj.get("method").and_then(Value::as_str);
    let id = obj.get("id").cloned();

    match (id, method) {
        (Some(id), Some(method)) => Ok(Incoming::Request {
            id,
            method: method.to_string(),
            params: obj.get("params").cloned().unwrap_or(Value::Null),
        }),
        (None, Some(method)) => Ok(Incoming::Notification {
            method: method.to_string(),
            params: obj.get("params").cloned().unwrap_or(Value::Null),
        }),
        (Some(id), None) => Ok(Incoming::Response {
            id,
            result: obj.get("result").cloned(),
            error: obj.get("error").cloned(),
        }),
        (None, None) => Err(FrameError::Invalid(
            "frame has neither id nor method".to_string(),
        )),
    }
}

/// Successful response to a client request.
pub fn response_result(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// Error response to a client request.
pub fn response_error(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message.into()},
    })
}

/// Server -> client notification.
pub fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

/// Server -> client request (used for `session/request_permission`).
pub fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// Serializes one frame as a single ndjson line (trailing newline included).
pub fn encode_line(frame: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(frame).unwrap_or_else(|_| b"{}".to_vec());
    bytes.push(b'\n');
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn parses_request_with_params() {
        let frame = parse_frame(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}"#,
        )
        .expect("request should parse");
        assert_eq!(
            frame,
            Incoming::Request {
                id: json!(1),
                method: "initialize".to_string(),
                params: json!({"protocolVersion": 1}),
            }
        );
    }

    #[test]
    fn parses_notification_without_id() {
        let frame = parse_frame(r#"{"jsonrpc":"2.0","method":"session/cancel","params":{}}"#)
            .expect("notification should parse");
        assert_eq!(
            frame,
            Incoming::Notification {
                method: "session/cancel".to_string(),
                params: json!({}),
            }
        );
    }

    #[test]
    fn parses_response_with_result() {
        let frame = parse_frame(r#"{"jsonrpc":"2.0","id":7,"result":{"outcome":{"outcome":"selected","optionId":"allow_once"}}}"#)
            .expect("response should parse");
        assert_eq!(
            frame,
            Incoming::Response {
                id: json!(7),
                result: Some(json!({"outcome": {"outcome": "selected", "optionId": "allow_once"}})),
                error: None,
            }
        );
    }

    #[test]
    fn parses_response_with_string_id() {
        let frame = parse_frame(r#"{"jsonrpc":"2.0","id":"perm-1","result":null}"#)
            .expect("response should parse");
        assert_eq!(
            frame,
            Incoming::Response {
                id: json!("perm-1"),
                result: Some(Value::Null),
                error: None,
            }
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            parse_frame("not json"),
            Err(FrameError::Parse(_))
        ));
        assert!(matches!(
            parse_frame(r#"{"jsonrpc":"2.0"}"#),
            Err(FrameError::Invalid(_))
        ));
    }

    #[test]
    fn encodes_single_line_frames() {
        let frame = response_result(&json!(1), json!({"protocolVersion": 1}));
        let line = String::from_utf8(encode_line(&frame)).expect("utf8");
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
        let roundtrip = parse_frame(line.trim_end()).expect("roundtrip");
        assert_eq!(
            roundtrip,
            Incoming::Response {
                id: json!(1),
                result: Some(json!({"protocolVersion": 1})),
                error: None,
            }
        );
    }
}
