//! Database query tool.
use std::collections::HashMap;

use ody_protocol::ToolName;
use ody_tools::{
    FunctionCallError, JsonToolOutput, ToolCall, ToolExecutor, ToolExecutorFuture, ToolExposure,
    ToolPayload, ToolSpec, parse_tool_input_schema,
};
use serde_json::json;

use crate::provider::SharedDatabaseProvider;

pub struct DatabaseQueryTool {
    session_id: String,
    primary: String,
    connections: HashMap<String, SharedDatabaseProvider>,
}

impl DatabaseQueryTool {
    pub fn new(
        session_id: String,
        primary: String,
        connections: HashMap<String, SharedDatabaseProvider>,
    ) -> Self {
        Self {
            session_id,
            primary,
            connections,
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct DatabaseQueryInput {
    query: String,
    #[serde(default)]
    connection: Option<String>,
}

fn format_result(result: &crate::provider::DatabaseQueryResult) -> String {
    if result.columns.is_empty() && result.rows.is_empty() {
        return "Query executed successfully. No rows returned.".to_string();
    }
    let mut lines = Vec::with_capacity(result.rows.len() + 2);
    lines.push(result.columns.join(" | "));
    lines.push("-".repeat(lines[0].len()));
    for row in &result.rows {
        lines.push(row.join(" | "));
    }
    lines.join("\n")
}

impl ToolExecutor<ToolCall> for DatabaseQueryTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("DatabaseQuery")
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ody_tools::ResponsesApiTool {
            name: "DatabaseQuery".to_string(),
            description: "Execute a SQL query against a configured database connection. Use the SQL dialect for the selected preset; PostgreSQL table discovery should query information_schema.tables or pg_catalog rather than SHOW DATABASES/SHOW TABLES.".to_string(),
            strict: true,
            parameters: parse_tool_input_schema(&json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "SQL query to execute."
                    },
                    "connection": {
                        "type": "string",
                        "description": "Named connection preset to use (for example, 'metricmind_db'); omit this to use the configured primary connection. Do not pass the literal string 'primary' unless it is the name of a saved preset."
                    }
                },
                "required": ["query"]
            }))
            .expect("DatabaseQuery input schema is valid JSON"),
            defer_loading: None,
            output_schema: None,
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let connections = self.connections.clone();
        let primary = self.primary.clone();
        let _session_id = self.session_id.clone();
        Box::pin(async move {
            let arguments = match call.payload {
                ToolPayload::Function { arguments } => arguments,
                _ => {
                    return Err(FunctionCallError::Fatal(
                        "DatabaseQuery only accepts function arguments".to_string(),
                    ));
                }
            };
            let input: DatabaseQueryInput = serde_json::from_str(&arguments)
                .map_err(|e| FunctionCallError::Fatal(format!("invalid DatabaseQuery input: {e}")))?;
            let provider = connections
                .get(input.connection.as_deref().unwrap_or(&primary))
                .ok_or_else(|| {
                    let selected = input.connection.as_deref().unwrap_or(&primary);
                    FunctionCallError::RespondToModel(format!(
                        "Database connection '{selected}' is not configured. Omit 'connection' to use the configured default, or provide a saved connection preset name."
                    ))
                })?;

            match provider.query(&input.query).await {
                Ok(result) => {
                    let text = format_result(&result);
                    let value = json!({
                        "columns": result.columns,
                        "rows": result.rows,
                        "text": text,
                    });
                    Ok(Box::new(JsonToolOutput::new(value)) as Box<dyn ody_tools::ToolOutput>)
                }
                Err(err) => Err(FunctionCallError::RespondToModel(err.user_message())),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::DatabaseError;
    use ody_protocol::protocol::TruncationPolicy;
    use ody_tools::NoopTurnItemEmitter;
    use std::sync::Arc;

    #[derive(Debug)]
    struct StubProvider;

    #[async_trait::async_trait]
    impl crate::provider::DatabaseProvider for StubProvider {
        fn name(&self) -> &str {
            "stub"
        }

        async fn query(&self, _sql: &str) -> Result<crate::provider::DatabaseQueryResult, DatabaseError> {
            Ok(crate::provider::DatabaseQueryResult {
                columns: vec!["id".to_string(), "name".to_string()],
                rows: vec![vec!["1".to_string(), "Alice".to_string()]],
            })
        }
    }

    fn tool_call(arguments: &str) -> ToolCall {
        ToolCall {
            turn_id: "turn-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name: ToolName::plain("DatabaseQuery"),
            model: "test-model".to_string(),
            truncation_policy: TruncationPolicy::Bytes(0),
            conversation_history: ody_tools::ConversationHistory::default(),
            turn_item_emitter: Arc::new(NoopTurnItemEmitter),
            environments: Vec::new(),
            payload: ToolPayload::Function {
                arguments: arguments.to_string(),
            },
            guardian_approved_action_id: None,
        }
    }

    #[tokio::test]
    async fn returns_json_output_with_columns_and_rows() {
        let mut connections = HashMap::new();
        connections.insert("primary".to_string(), Arc::new(StubProvider) as SharedDatabaseProvider);
        let tool = DatabaseQueryTool::new("session-1".to_string(), "primary".to_string(), connections);
        let output = tool
            .handle(tool_call(r#"{"query":"SELECT * FROM users"}"#))
            .await
            .expect("should succeed");
        let value = output.code_mode_result(&ToolPayload::Function {
            arguments: String::new(),
        });
        assert_eq!(value["columns"].as_array().unwrap().len(), 2);
        assert_eq!(value["rows"].as_array().unwrap().len(), 1);
        assert!(value["text"].as_str().unwrap().contains("Alice"));
    }

    #[tokio::test]
    async fn returns_model_error_for_unknown_connection() {
        let connections = HashMap::new();
        let tool = DatabaseQueryTool::new("session-1".to_string(), "primary".to_string(), connections);
        let err = match tool
            .handle(tool_call(r#"{"query":"SELECT 1","connection":"missing"}"#))
            .await
        {
            Err(err) => err,
            Ok(_) => panic!("should fail"),
        };
        assert!(
            matches!(err, FunctionCallError::RespondToModel(ref msg) if msg.contains("missing")),
            "expected missing connection error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn returns_model_error_for_query_failure() {
        #[derive(Debug)]
        struct FailingProvider;

        #[async_trait::async_trait]
        impl crate::provider::DatabaseProvider for FailingProvider {
            fn name(&self) -> &str {
                "failing"
            }

            async fn query(
                &self,
                _sql: &str,
            ) -> Result<crate::provider::DatabaseQueryResult, DatabaseError> {
                Err(DatabaseError::Query("syntax error".to_string()))
            }
        }

        let mut connections = HashMap::new();
        connections.insert(
            "primary".to_string(),
            Arc::new(FailingProvider) as SharedDatabaseProvider,
        );
        let tool = DatabaseQueryTool::new("session-1".to_string(), "primary".to_string(), connections);
        let err = match tool.handle(tool_call(r#"{"query":"SHOW DATABASES"}"#)).await {
            Err(err) => err,
            Ok(_) => panic!("should return the query error to the model"),
        };
        assert!(
            matches!(err, FunctionCallError::RespondToModel(ref msg) if msg.contains("syntax error")),
            "expected query error, got {err:?}"
        );
    }
}
