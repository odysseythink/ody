use ody_protocol::ToolName;
use ody_tools::{
    FunctionCallError, JsonToolOutput, ToolCall, ToolExecutor, ToolExecutorFuture, ToolExposure,
    ToolPayload, ToolSpec, parse_tool_input_schema,
};
use serde_json::json;

use crate::provider::{SharedWebSearchProvider, WebSearchOptions, WebSearchToolOutput};

pub struct WebSearchTool {
    session_id: String,
    provider: SharedWebSearchProvider,
}

impl WebSearchTool {
    pub fn new(session_id: String, provider: SharedWebSearchProvider) -> Self {
        Self {
            session_id,
            provider,
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct WebSearchInput {
    query: String,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    include_content: Option<bool>,
}

fn format_results(results: &[crate::provider::WebSearchResult]) -> String {
    if results.is_empty() {
        return "No search results found.".to_string();
    }
    results
        .iter()
        .map(|r| format!("{}\n{}\n{}", r.title, r.url, r.snippet))
        .collect::<Vec<_>>()
        .join("\n\n")
}

impl ToolExecutor<ToolCall> for WebSearchTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("WebSearch")
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ody_tools::ResponsesApiTool {
            name: "WebSearch".to_string(),
            description: "Search the web for up-to-date information.".to_string(),
            strict: true,
            parameters: parse_tool_input_schema(&json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results to return."
                    },
                    "include_content": {
                        "type": "boolean",
                        "description": "Whether to include a content snippet for each result."
                    }
                },
                "required": ["query"]
            }))
            .expect("WebSearch input schema is valid JSON"),
            defer_loading: None,
            output_schema: None,
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let provider = self.provider.clone();
        let call_id = call.call_id.clone();
        Box::pin(async move {
            let arguments = match call.payload {
                ToolPayload::Function { arguments } => arguments,
                _ => {
                    return Err(FunctionCallError::Fatal(
                        "WebSearch only accepts function arguments".to_string(),
                    ));
                }
            };
            let input: WebSearchInput = serde_json::from_str(&arguments)
                .map_err(|e| FunctionCallError::Fatal(format!("invalid WebSearch input: {e}")))?;
            let options = WebSearchOptions {
                limit: input.limit,
                include_content: input.include_content,
                tool_call_id: Some(call_id.clone()),
            };

            match provider.search(&input.query, &options).await {
                Ok(results) => {
                    let result_count = results.len();
                    let output = WebSearchToolOutput {
                        result_count,
                        text: format_results(&results),
                    };
                    let value = serde_json::to_value(&output).map_err(|e| {
                        FunctionCallError::Fatal(format!(
                            "failed to serialize WebSearch output: {e}"
                        ))
                    })?;
                    Ok(Box::new(JsonToolOutput::new(value)) as Box<dyn ody_tools::ToolOutput>)
                }
                Err(err) => Err(FunctionCallError::Fatal(err.user_message())),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::WebSearchError;
    use crate::provider::{WebSearchProvider, WebSearchResult};
    use async_trait::async_trait;
    use ody_protocol::protocol::TruncationPolicy;
    use ody_tools::NoopTurnItemEmitter;
    use std::sync::Arc;

    #[derive(Debug)]
    struct StubProvider(Vec<WebSearchResult>);
    #[async_trait]
    impl WebSearchProvider for StubProvider {
        fn name(&self) -> &str {
            "stub"
        }
        async fn search(
            &self,
            _query: &str,
            _options: &WebSearchOptions,
        ) -> Result<Vec<WebSearchResult>, WebSearchError> {
            Ok(self.0.clone())
        }
    }

    fn sample_result() -> WebSearchResult {
        WebSearchResult {
            title: "Example".to_string(),
            url: "https://example.com".to_string(),
            snippet: "An example snippet.".to_string(),
            date: None,
            content: None,
        }
    }

    fn tool_call(arguments: &str) -> ToolCall {
        ToolCall {
            turn_id: "turn-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name: ToolName::plain("WebSearch"),
            model: "test-model".to_string(),
            truncation_policy: TruncationPolicy::Bytes(0),
            conversation_history: ody_tools::ConversationHistory::default(),
            turn_item_emitter: Arc::new(NoopTurnItemEmitter),
            environments: Vec::new(),
            payload: ToolPayload::Function {
                arguments: arguments.to_string(),
            },
        }
    }

    #[tokio::test]
    async fn returns_json_output_with_result_count_and_text() {
        let tool = WebSearchTool::new(
            "session-1".to_string(),
            Arc::new(StubProvider(vec![sample_result()])),
        );
        let output = tool
            .handle(tool_call(r#"{"query":"hello"}"#))
            .await
            .expect("should succeed");
        let value = output.code_mode_result(&ToolPayload::Function {
            arguments: String::new(),
        });
        assert_eq!(value["result_count"], 1);
        assert!(value["text"].as_str().unwrap().contains("Example"));
    }

    #[tokio::test]
    async fn returns_empty_message_for_zero_results() {
        let tool = WebSearchTool::new("session-1".to_string(), Arc::new(StubProvider(vec![])));
        let output = tool
            .handle(tool_call(r#"{"query":"hello"}"#))
            .await
            .expect("should succeed");
        let value = output.code_mode_result(&ToolPayload::Function {
            arguments: String::new(),
        });
        assert_eq!(value["result_count"], 0);
        assert_eq!(value["text"].as_str().unwrap(), "No search results found.");
    }

    #[tokio::test]
    async fn returns_fatal_error_for_provider_failure() {
        #[derive(Debug)]
        struct FailingProvider;
        #[async_trait]
        impl WebSearchProvider for FailingProvider {
            fn name(&self) -> &str {
                "failing"
            }
            async fn search(
                &self,
                _query: &str,
                _options: &WebSearchOptions,
            ) -> Result<Vec<WebSearchResult>, WebSearchError> {
                Err(WebSearchError::Auth)
            }
        }
        let tool = WebSearchTool::new("session-1".to_string(), Arc::new(FailingProvider));
        let err = match tool.handle(tool_call(r#"{"query":"hello"}"#)).await {
            Err(err) => err,
            Ok(_) => panic!("should fail"),
        };
        assert_eq!(
            err,
            FunctionCallError::Fatal(
                "Search failed: please check your web search API key.".to_string()
            )
        );
    }
}
