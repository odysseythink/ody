use std::fmt::Display;

use super::ContextualUserFragment;

/// User-visible record of one flow skill execution, stored as a user-role
/// conversation item so the result is part of the durable session history
/// (M1.3). Success records the flow outputs; failure records the error
/// detail (a `WarningEvent` is emitted alongside for immediate visibility).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FlowResultMessage {
    pub(crate) flow_name: String,
    pub(crate) detail: String,
    pub(crate) succeeded: bool,
}

impl FlowResultMessage {
    pub(crate) fn success(
        flow_name: impl Into<String>,
        outputs: &serde_json::Map<String, serde_json::Value>,
    ) -> Self {
        let rendered = serde_json::to_string_pretty(outputs).unwrap_or_else(|_| "{}".to_string());
        Self {
            flow_name: flow_name.into(),
            detail: format!("Outputs:\n{rendered}"),
            succeeded: true,
        }
    }

    pub(crate) fn failure(flow_name: impl Into<String>, error: impl Display) -> Self {
        Self {
            flow_name: flow_name.into(),
            detail: error.to_string(),
            succeeded: false,
        }
    }
}

impl ContextualUserFragment for FlowResultMessage {
    fn role(&self) -> &'static str {
        "user"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<flow_result>", "</flow_result>")
    }

    fn body(&self) -> String {
        let outcome = if self.succeeded { "completed" } else { "failed" };
        format!("\nFlow '{}' {}. {}\n", self.flow_name, outcome, self.detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn success_body_lists_pretty_outputs() {
        let message = FlowResultMessage::success(
            "demo",
            &serde_json::Map::from_iter([("game".to_string(), json!("chess"))]),
        );
        let rendered = message.render();
        assert!(rendered.starts_with("<flow_result>"));
        assert!(rendered.ends_with("</flow_result>"));
        assert!(rendered.contains("Flow 'demo' completed."));
        assert!(rendered.contains("\"game\": \"chess\""));
        assert!(FlowResultMessage::matches_text(&rendered));
    }

    #[test]
    fn failure_body_carries_error_detail() {
        let message = FlowResultMessage::failure("demo", "flow host error: boom");
        let rendered = message.render();
        assert!(rendered.contains("Flow 'demo' failed."));
        assert!(rendered.contains("boom"));
    }
}
