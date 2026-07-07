mod custom_ca;
pub mod default_client;
mod error;
mod outbound_proxy;
mod request;
mod retry;
mod sse;
mod telemetry;
mod transport;

pub use crate::custom_ca::BuildCustomCaTransportError;
/// Test-only subprocess hook for custom CA coverage.
///
/// This stays public only so the `custom_ca_probe` binary target can reuse the shared helper. It
/// is hidden from normal docs because ordinary callers should use
/// [`build_reqwest_client_with_custom_ca`] instead.
#[doc(hidden)]
pub use crate::custom_ca::build_reqwest_client_for_subprocess_tests;
pub use crate::custom_ca::build_reqwest_client_with_custom_ca;
pub use crate::custom_ca::maybe_build_rustls_client_config_with_custom_ca;
pub use crate::default_client::OdyHttpClient;
pub use crate::default_client::OdyRequestBuilder;
pub use crate::default_client::ResidencyRequirement;
pub use crate::default_client::SetOriginatorError;
pub use crate::default_client::Originator;
pub use crate::default_client::USER_AGENT_SUFFIX;
pub use crate::default_client::DEFAULT_ORIGINATOR;
pub use crate::default_client::ODY_INTERNAL_ORIGINATOR_OVERRIDE_ENV_VAR;
pub use crate::default_client::RESIDENCY_HEADER_NAME;
pub use crate::default_client::create_client;
pub use crate::default_client::build_reqwest_client;
pub use crate::default_client::try_build_reqwest_client;
pub use crate::default_client::build_default_auth_reqwest_client;
pub use crate::default_client::create_default_auth_client;
pub use crate::default_client::default_headers;
pub use crate::default_client::get_ody_user_agent;
pub use crate::default_client::originator;
pub use crate::default_client::is_first_party_originator;
pub use crate::default_client::is_first_party_chat_originator;
pub use crate::default_client::set_default_originator;
pub use crate::default_client::set_default_client_residency_requirement;
pub use crate::error::StreamError;
pub use crate::error::TransportError;
pub use crate::outbound_proxy::AuthRouteConfig;
pub use crate::outbound_proxy::BuildRouteAwareHttpClientError;
pub use crate::outbound_proxy::ClientRouteClass;
pub use crate::outbound_proxy::OutboundProxyConfig;
pub use crate::outbound_proxy::RouteFailureClass;
pub use crate::outbound_proxy::build_reqwest_client_for_route;
pub use crate::request::EncodedJsonBody;
pub use crate::request::PreparedRequestBody;
pub use crate::request::Request;
pub use crate::request::RequestBody;
pub use crate::request::RequestCompression;
pub use crate::request::Response;
pub use crate::retry::RetryOn;
pub use crate::retry::RetryPolicy;
pub use crate::retry::backoff;
pub use crate::retry::run_with_retry;
pub use crate::sse::sse_stream;
pub use crate::telemetry::RequestTelemetry;
pub use crate::transport::ByteStream;
pub use crate::transport::HttpTransport;
pub use crate::transport::ReqwestTransport;
pub use crate::transport::StreamResponse;
