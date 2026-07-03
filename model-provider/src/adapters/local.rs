//! Placeholder adapter for local providers (Ollama / LM Studio).
//!
//! This will be fully implemented in T2.2.2 by wrapping the existing
//! `ody-ollama` and `ody-lmstudio` crates.

use crate::chat_provider::{
    ChatCompletion, ChatProvider, ChatProviderError, ChatRequest, ChatStream, ProviderCapabilities,
    ProviderId,
};

/// Adapter for local model servers.
#[derive(Debug, Clone)]
pub struct LocalAdapter {
    provider_id: ProviderId,
    capabilities: ProviderCapabilities,
}

impl LocalAdapter {
    pub fn new(provider_id: ProviderId) -> Self {
        Self {
            provider_id,
            capabilities: ProviderCapabilities {
                supports_streaming: true,
                supports_tools: false,
                supports_thinking: false,
                supports_vision: false,
                supports_multiple_system_messages: true,
                supports_turn_pause: false,
                max_context_tokens: None,
                max_output_tokens: None,
                thinking_effort: vec![],
            },
        }
    }
}

#[async_trait::async_trait]
impl ChatProvider for LocalAdapter {
    fn provider_id(&self) -> ProviderId {
        self.provider_id
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    async fn chat(&self, _request: ChatRequest) -> Result<ChatStream, ChatProviderError> {
        Err(ChatProviderError::Unsupported {
            capability: "streaming chat".into(),
        })
    }

    async fn chat_complete(
        &self,
        _request: ChatRequest,
    ) -> Result<ChatCompletion, ChatProviderError> {
        Err(ChatProviderError::Unsupported {
            capability: "non-streaming chat".into(),
        })
    }
}

impl Default for LocalAdapter {
    fn default() -> Self {
        Self::new("local")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_reports_expected_id() {
        let adapter = LocalAdapter::new("ollama");
        assert_eq!(adapter.provider_id(), "ollama");
    }

    #[test]
    fn local_adapter_does_not_support_tools_by_default() {
        let adapter = LocalAdapter::new("lmstudio");
        assert!(!adapter.capabilities().supports_tools);
    }
}
