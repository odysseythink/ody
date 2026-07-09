use crate::auth::SharedAuthProvider;
use crate::chat::ChatCompletionsRequest;
use crate::common::ResponseStream;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::provider::Provider;
use crate::requests::Compression;
use crate::sse::spawn_chat_stream;
use crate::telemetry::SseTelemetry;
use ody_client::EncodedJsonBody;
use ody_client::HttpTransport;
use ody_client::RequestCompression;
use ody_client::RequestTelemetry;
use http::HeaderMap;
use http::HeaderValue;
use http::Method;
use std::sync::Arc;
use tracing::instrument;

pub struct ChatCompletionsClient<T: HttpTransport> {
    session: EndpointSession<T>,
    sse_telemetry: Option<Arc<dyn SseTelemetry>>,
}

#[derive(Default)]
pub struct ChatCompletionsOptions {
    pub extra_headers: HeaderMap,
    pub compression: Compression,
}

impl<T: HttpTransport> ChatCompletionsClient<T> {
    pub fn new(transport: T, provider: Provider, auth: SharedAuthProvider) -> Self {
        Self {
            session: EndpointSession::new(transport, provider, auth),
            sse_telemetry: None,
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
        }
    }

    fn path() -> &'static str {
        "chat/completions"
    }

    #[instrument(
        name = "chat_completions.stream_request",
        level = "info",
        skip_all,
        fields(
            transport = "chat_http",
            http.method = "POST",
            api.path = "chat/completions"
        )
    )]
    pub async fn stream_request(
        &self,
        request: ChatCompletionsRequest,
        options: ChatCompletionsOptions,
    ) -> Result<ResponseStream, ApiError> {
        let vendor = request.vendor;
        log_outgoing_identity_headers_once(&self.session.provider().headers);
        let body = EncodedJsonBody::encode(&request.to_wire()).map_err(|e| {
            ApiError::Stream(format!("failed to encode chat completions request: {e}"))
        })?;

        let request_compression = match options.compression {
            Compression::None => RequestCompression::None,
            Compression::Zstd => RequestCompression::Zstd,
        };

        let stream_response = self
            .session
            .stream_encoded_json_with(
                Method::POST,
                Self::path(),
                options.extra_headers,
                Some(body),
                |req| {
                    req.headers.insert(
                        http::header::ACCEPT,
                        HeaderValue::from_static("text/event-stream"),
                    );
                    req.compression = request_compression;
                },
            )
            .await?;

        Ok(spawn_chat_stream(
            stream_response,
            self.session.provider().stream_idle_timeout,
            self.sse_telemetry.clone(),
            vendor,
        ))
    }
}

/// Log the identity headers (User-Agent, `X-Msh-*`, …) we send on chat
/// completions requests exactly once per process, so the client's presented
/// identity can be diagnosed without enabling full request tracing. Secret
/// headers (authorization / api keys) are redacted.
fn log_outgoing_identity_headers_once(headers: &HeaderMap) {
    use std::sync::Once;
    static LOG_ONCE: Once = Once::new();
    LOG_ONCE.call_once(|| {
        let redacted: Vec<(String, String)> = headers
            .iter()
            .map(|(name, value)| {
                let name = name.as_str().to_string();
                let lower = name.to_ascii_lowercase();
                let value = if lower == "authorization" || lower.contains("api-key") {
                    "<redacted>".to_string()
                } else {
                    value.to_str().unwrap_or("<non-ascii>").to_string()
                };
                (name, value)
            })
            .collect();
        tracing::debug!(
            target: "ody_api::chat",
            headers = ?redacted,
            "chat completions outgoing provider headers (logged once)"
        );
    });
}
