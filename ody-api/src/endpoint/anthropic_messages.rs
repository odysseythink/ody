use crate::anthropic::AnthropicMessagesRequest;
use crate::auth::SharedAuthProvider;
use crate::common::ResponseStream;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::provider::Provider;
use crate::requests::Compression;
use crate::sse;
use crate::telemetry::SseTelemetry;
use http::HeaderMap;
use http::HeaderValue;
use http::Method;
use ody_client::EncodedJsonBody;
use ody_client::HttpTransport;
use ody_client::RequestCompression;
use ody_client::RequestTelemetry;
use std::sync::Arc;
use tracing::instrument;

/// Client for the Anthropic Messages wire API.
///
/// Two deployment shapes (see [`crate::anthropic`]):
/// - direct Anthropic API: `POST {base_url}/v1/messages`, SSE streaming;
/// - AWS Bedrock Claude: `POST {base_url}/model/{model}/invoke-with-response-stream`,
///   SigV4-signed (transport-level), AWS event stream framing.
pub struct AnthropicMessagesClient<T: HttpTransport> {
    session: EndpointSession<T>,
    sse_telemetry: Option<Arc<dyn SseTelemetry>>,
    bedrock: bool,
}

#[derive(Default)]
pub struct AnthropicOptions {
    pub extra_headers: HeaderMap,
    pub compression: Compression,
}

impl<T: HttpTransport> AnthropicMessagesClient<T> {
    pub fn new(transport: T, provider: Provider, auth: SharedAuthProvider, bedrock: bool) -> Self {
        Self {
            session: EndpointSession::new(transport, provider, auth),
            sse_telemetry: None,
            bedrock,
        }
    }

    pub fn with_telemetry(
        self,
        request: Option<Arc<dyn RequestTelemetry>>,
        sse: Option<Arc<dyn SseTelemetry>>,
    ) -> Self {
        Self {
            session: self.session.with_request_telemetry(request),
            sse_telemetry: sse,
            bedrock: self.bedrock,
        }
    }

    /// Request path for the Messages API shape. Bedrock embeds the model id
    /// in the path; the direct API takes it from the body only.
    fn path_for(model: &str, bedrock: bool) -> String {
        if bedrock {
            format!("model/{model}/invoke-with-response-stream")
        } else {
            "v1/messages".to_string()
        }
    }

    #[instrument(
        name = "anthropic_messages.stream_request",
        level = "info",
        skip_all,
        fields(
            http.method = "POST",
            bedrock = self.bedrock,
        )
    )]
    pub async fn stream_request(
        &self,
        request: AnthropicMessagesRequest,
        options: AnthropicOptions,
    ) -> Result<ResponseStream, ApiError> {
        let path = Self::path_for(&request.model, self.bedrock);
        let body = EncodedJsonBody::encode(&request.to_wire()).map_err(|e| {
            ApiError::Stream(format!("failed to encode anthropic messages request: {e}"))
        })?;

        let request_compression = match options.compression {
            Compression::None => RequestCompression::None,
            Compression::Zstd => RequestCompression::Zstd,
        };

        let accept = if self.bedrock {
            // Bedrock streams response events as AWS event stream frames.
            "application/vnd.amazon.eventstream"
        } else {
            "text/event-stream"
        };

        let stream_response = self
            .session
            .stream_encoded_json_with(Method::POST, &path, options.extra_headers, Some(body), |req| {
                req.headers.insert(
                    http::header::ACCEPT,
                    HeaderValue::from_static(accept),
                );
                req.compression = request_compression;
            })
            .await?;

        if self.bedrock {
            Ok(sse::spawn_anthropic_eventstream(
                stream_response,
                self.session.provider().stream_idle_timeout,
                self.sse_telemetry.clone(),
            ))
        } else {
            Ok(sse::spawn_anthropic_stream(
                stream_response,
                self.session.provider().stream_idle_timeout,
                self.sse_telemetry.clone(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_path_is_fixed() {
        assert_eq!(
            AnthropicMessagesClient::<ody_client::ReqwestTransport>::path_for("claude-1", false),
            "v1/messages"
        );
    }

    #[test]
    fn bedrock_path_embeds_model() {
        assert_eq!(
            AnthropicMessagesClient::<ody_client::ReqwestTransport>::path_for(
                "anthropic.claude-sonnet-4-5-v1:0",
                true
            ),
            "model/anthropic.claude-sonnet-4-5-v1:0/invoke-with-response-stream"
        );
    }
}
