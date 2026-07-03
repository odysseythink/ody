mod auth;
mod bearer_auth_provider;
mod chat_provider;
#[cfg(test)]
mod chat_provider_tests;
mod models_endpoint;
mod provider;

pub mod adapters;

pub use chat_provider::ChatCompletion;
pub use chat_provider::ChatEvent;
pub use chat_provider::ChatProvider;
pub use chat_provider::ChatProviderError;
pub use chat_provider::ChatRequest;
pub use chat_provider::ChatStream;
pub use chat_provider::ContentPart;
pub use chat_provider::FinishReason;
pub use chat_provider::Message;
pub use chat_provider::ProviderCapabilities as ChatProviderCapabilities;
pub use chat_provider::ProviderId;
pub use chat_provider::RawFrame;
pub use chat_provider::Role;
pub use chat_provider::ThinkingEffort;
pub use chat_provider::ToolCall;
pub use chat_provider::ToolDefinition;
pub use chat_provider::Usage;
pub use chat_provider::clamp_thinking_effort;
pub use chat_provider::transport_error;

pub use auth::auth_provider_from_auth;
pub use auth::unauthenticated_auth_provider;
pub use bearer_auth_provider::BearerAuthProvider;
pub use bearer_auth_provider::BearerAuthProvider as CoreAuthProvider;
pub use ody_protocol::account::ProviderAccount;
pub use provider::ModelProvider;
pub use provider::ModelProviderFuture;
pub use provider::ProviderAccountResult;
pub use provider::ProviderAccountState;
pub use provider::ProviderCapabilities;
pub use provider::SharedModelProvider;
pub use provider::create_model_provider;
pub use provider::create_model_provider_with_id;
