//! Per-provider dialect hooks for the Chat Completions wire protocol.
//!
//! DeepSeek and GLM speak (essentially) standard OpenAI Chat Completions, so
//! they use the generic behavior. Kimi (Moonshot) layers several proprietary
//! extensions on top; those are applied here. See `super::ChatCompletionsRequest`.

use serde_json::Value;

use super::ChatCompletionsRequest;

/// Which provider dialect to emit on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatVendor {
    /// Standard OpenAI-compatible Chat Completions (DeepSeek, GLM, generic).
    #[default]
    Generic,
    /// Moonshot Kimi, with proprietary extensions.
    Kimi,
    DeepSeek,
    Glm,
}

impl ChatVendor {
    /// Resolve the dialect for a provider, based on its id/name and base URL.
    pub fn from_provider(provider: &str, base_url: Option<&str>) -> Self {
        match provider.to_ascii_lowercase().as_str() {
            "kimi" | "moonshot" => return ChatVendor::Kimi,
            "deepseek" => return ChatVendor::DeepSeek,
            "glm" | "zhipu" | "bigmodel" => return ChatVendor::Glm,
            _ => {}
        }
        if let Some(base_url) = base_url {
            if base_url.contains("moonshot") {
                return ChatVendor::Kimi;
            }
            if base_url.contains("deepseek") {
                return ChatVendor::DeepSeek;
            }
            if base_url.contains("bigmodel") {
                return ChatVendor::Glm;
            }
        }
        ChatVendor::Generic
    }

    /// Whether this provider emits a `reasoning_content` field for thinking
    /// output in streamed deltas.
    pub fn supports_reasoning_content(self) -> bool {
        matches!(
            self,
            ChatVendor::Kimi | ChatVendor::DeepSeek | ChatVendor::Glm | ChatVendor::Generic
        )
    }

    /// Whether this provider accepts a `reasoning_content` field on *inbound*
    /// assistant messages, i.e. whether the model's own thinking can be
    /// replayed to it on the next turn ("interleaved thinking").
    ///
    /// Deliberately narrower than [`Self::supports_reasoning_content`], which
    /// describes what a provider *emits*: a provider that streams thinking out
    /// does not necessarily accept it back. `Generic` is excluded because an
    /// unknown OpenAI-compatible gateway may reject the unknown field.
    pub fn accepts_reasoning_content(self) -> bool {
        matches!(
            self,
            ChatVendor::Kimi | ChatVendor::DeepSeek | ChatVendor::Glm
        )
    }

    /// Whether to send the top-level `reasoning_effort` request field. GLM does
    /// not accept it (thinking is controlled by the model / response side).
    pub fn emits_reasoning_effort(self) -> bool {
        !matches!(self, ChatVendor::Glm)
    }

    /// The request field name for the output-token cap. GLM uses the legacy
    /// `max_tokens`; the others prefer `max_completion_tokens`.
    pub fn max_tokens_field(self) -> &'static str {
        match self {
            ChatVendor::Glm => "max_tokens",
            _ => "max_completion_tokens",
        }
    }

    /// Sanitize a tool call id to satisfy provider-specific constraints. Kimi
    /// rejects ids longer than 64 characters.
    pub fn sanitize_tool_call_id(self, id: &str) -> String {
        match self {
            ChatVendor::Kimi => sanitize_tool_call_id(id, 64),
            _ => id.to_string(),
        }
    }

    /// Provider-specific tool conversion. Returns `Some` to override the
    /// generic conversion in [`super::convert_tool`]. Kimi maps `$`-prefixed
    /// tool names onto its `builtin_function` wire form.
    pub fn convert_tool(self, tool: &Value) -> Option<Value> {
        if self != ChatVendor::Kimi {
            return None;
        }
        let name = tool.as_object()?.get("name")?.as_str()?;
        let builtin = name.strip_prefix('$')?;
        Some(serde_json::json!({
            "type": "builtin_function",
            "function": { "name": builtin },
        }))
    }

    /// Normalize a function tool's `parameters` schema. Kimi rejects schemas
    /// with missing `type` fields or unresolved `$ref`s, so they are rewritten.
    pub fn normalize_tool_parameters(self, parameters: &Value) -> Value {
        if self != ChatVendor::Kimi {
            return parameters.clone();
        }
        match parameters.as_object() {
            Some(obj) => Value::Object(super::kimi_schema::normalize_kimi_tool_schema(obj.clone())),
            None => parameters.clone(),
        }
    }

    /// Apply provider-specific mutations to the fully built request body.
    ///
    /// GLM (Zhipu) thinking models default to thinking **ON** when the request
    /// carries no thinking control, which streams a long `reasoning_content`
    /// trace and roughly doubles latency (measured: a one-line prompt took 12.8s
    /// with thinking vs 6.7s without; large designs blow past the review
    /// timeout). ody already treats GLM as non-thinking (see
    /// [`Self::supports_thinking` in the adapter] and [`Self::emits_reasoning_effort`]),
    /// so make the wire match that stance by explicitly disabling it. Mirrors
    /// ody-code's GLM provider, which sends `thinking: { type: "disabled" }` when
    /// its thinking effort is off (`packages/kosong/src/providers/glm.ts`).
    pub fn apply_request(self, body: &mut Value, request: &ChatCompletionsRequest) {
        let _ = request;
        if self == ChatVendor::Glm
            && let Some(object) = body.as_object_mut()
        {
            object.insert("thinking".into(), serde_json::json!({ "type": "disabled" }));
        }
    }
}

/// Truncate a tool call id to `max` characters while keeping it deterministic.
fn sanitize_tool_call_id(id: &str, max: usize) -> String {
    if id.chars().count() <= max {
        return id.to_string();
    }
    id.chars().take(max).collect()
}
