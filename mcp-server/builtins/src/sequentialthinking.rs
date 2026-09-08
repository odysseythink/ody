//! Sequential thinking MCP server.
//!
//! Port of the official `@modelcontextprotocol/server-sequential-thinking`
//! (<https://github.com/modelcontextprotocol/servers/tree/main/src/sequentialthinking>).
//! The tool surface and response shape match the current official version:
//! thought history is append-only, `totalThoughts` is raised automatically
//! when `thoughtNumber` exceeds it, and the tool returns a JSON summary.

use std::collections::BTreeMap;
use std::sync::Mutex;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::tool;
use rmcp::tool_handler;
use rmcp::tool_router;
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt, transport::stdio};
use serde::Deserialize;
use serde::Deserializer;
use serde_json::json;

fn flexible_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    struct BoolVisitor;
    impl serde::de::Visitor<'_> for BoolVisitor {
        type Value = bool;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a boolean or \"true\"/\"false\" string")
        }
        fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<bool, E> {
            Ok(v)
        }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<bool, E> {
            match v.to_lowercase().as_str() {
                "true" => Ok(true),
                "false" => Ok(false),
                _ => Err(E::custom(format!(
                    "Expected boolean or \"true\"/\"false\" string, received \"{v}\""
                ))),
            }
        }
    }
    deserializer.deserialize_any(BoolVisitor)
}

fn flexible_opt_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    struct OptBoolVisitor;
    impl serde::de::Visitor<'_> for OptBoolVisitor {
        type Value = Option<bool>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an optional boolean or \"true\"/\"false\" string")
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Option<bool>, E> {
            Ok(None)
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<Option<bool>, E> {
            Ok(None)
        }
        fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Option<bool>, E> {
            Ok(Some(v))
        }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Option<bool>, E> {
            match v.to_lowercase().as_str() {
                "true" => Ok(Some(true)),
                "false" => Ok(Some(false)),
                _ => Err(E::custom(format!(
                    "Expected boolean or \"true\"/\"false\" string, received \"{v}\""
                ))),
            }
        }
    }
    deserializer.deserialize_option(OptBoolVisitor)
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThoughtData {
    /// Your current thinking step, which can include regular analytical steps, revisions of previous thoughts, questions about previous decisions, realizations about needing more analysis, changes in approach, hypothesis generation, and hypothesis verification
    pub thought: String,
    /// Whether another thought step is needed
    #[serde(deserialize_with = "flexible_bool")]
    pub next_thought_needed: bool,
    /// Current thought number (numeric value, e.g., 1, 2, 3)
    pub thought_number: u32,
    /// Estimated total thoughts needed (numeric value, e.g., 5, 10); can be adjusted up or down as you progress
    pub total_thoughts: u32,
    /// Whether this revises previous thinking
    #[serde(default, deserialize_with = "flexible_opt_bool")]
    pub is_revision: Option<bool>,
    /// If is_revision is true, which thought number is being reconsidered
    pub revises_thought: Option<u32>,
    /// If branching, which thought number is the branching point
    pub branch_from_thought: Option<u32>,
    /// Identifier for the current branch (if any)
    pub branch_id: Option<String>,
    /// If reaching end but realizing more thoughts needed
    #[serde(default, deserialize_with = "flexible_opt_bool")]
    pub needs_more_thoughts: Option<bool>,
}

impl ThoughtData {
    fn validate(&self) -> Result<(), McpError> {
        if self.thought_number < 1 {
            return Err(McpError::invalid_params(
                "thoughtNumber must be >= 1",
                Some(json!({ "thoughtNumber": self.thought_number })),
            ));
        }
        if self.total_thoughts < 1 {
            return Err(McpError::invalid_params(
                "totalThoughts must be >= 1",
                Some(json!({ "totalThoughts": self.total_thoughts })),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct SequentialThinkingServer {
    #[allow(dead_code)] // read by the tool_router/tool_handler macro glue
    tool_router: ToolRouter<Self>,
    state: std::sync::Arc<Mutex<ThoughtState>>,
}

#[derive(Default)]
struct ThoughtState {
    history: Vec<ThoughtData>,
    branches: BTreeMap<String, Vec<ThoughtData>>,
}

#[tool_router]
impl SequentialThinkingServer {
    #[tool(description = "A detailed tool for dynamic and reflective problem-solving through thoughts. This tool helps analyze problems through a flexible thinking process that can adapt and evolve. Each thought can build on, question, or revise previous insights as understanding deepens. Use for breaking down complex problems into steps, planning with room for revision, analysis that might need course correction, and tasks that need to maintain context over multiple steps. You can adjust totalThoughts up or down, revise previous thoughts, branch into alternative paths, and add more thoughts even after reaching what seemed like the end. Only set nextThoughtNeeded to false when truly done.")]
    async fn sequentialthinking(
        &self,
        Parameters(mut input): Parameters<ThoughtData>,
    ) -> Result<CallToolResult, McpError> {
        input.validate()?;

        // Adjust totalThoughts if thoughtNumber exceeds it.
        if input.thought_number > input.total_thoughts {
            input.total_thoughts = input.thought_number;
        }

        let mut state = self.state.lock().expect("thought state poisoned");
        state.history.push(input.clone());
        if let (Some(_from), Some(branch_id)) = (input.branch_from_thought, &input.branch_id) {
            state
                .branches
                .entry(branch_id.clone())
                .or_default()
                .push(input.clone());
        }
        let branches: Vec<String> = state.branches.keys().cloned().collect();
        let history_len = state.history.len();
        drop(state);

        let summary = json!({
            "thoughtNumber": input.thought_number,
            "totalThoughts": input.total_thoughts,
            "nextThoughtNeeded": input.next_thought_needed,
            "branches": branches,
            "thoughtHistoryLength": history_len,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&summary).expect("summary serialization"),
        )]))
    }
}

#[tool_handler]
impl ServerHandler for SequentialThinkingServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Sequential Thinking MCP server. Tool: sequentialthinking (dynamic and reflective problem-solving through a structured thinking process).".to_string())
    }
}

/// Serve the sequential thinking MCP server over stdio.
pub async fn run() -> anyhow::Result<()> {
    let server = SequentialThinkingServer {
        tool_router: SequentialThinkingServer::tool_router(),
        state: Default::default(),
    };
    let server = server.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("serving error: {:?}", e);
    })?;
    server.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_string_booleans() {
        let json = r#"{
            "thought": "step 1",
            "nextThoughtNeeded": "true",
            "thoughtNumber": 1,
            "totalThoughts": 3
        }"#;
        let thought: ThoughtData = serde_json::from_str(json).unwrap();
        assert!(thought.next_thought_needed);
    }

    #[test]
    fn rejects_invalid_bool_string() {
        let json = r#"{
            "thought": "step 1",
            "nextThoughtNeeded": "yes",
            "thoughtNumber": 1,
            "totalThoughts": 3
        }"#;
        assert!(serde_json::from_str::<ThoughtData>(json).is_err());
    }
}
