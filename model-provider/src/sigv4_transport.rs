//! SigV4 request-signing transport decorator (AWS Bedrock).
//!
//! Wraps any [`HttpTransport`] and signs each outgoing request with AWS
//! Signature Version 4 (service `bedrock`) before delegating to the inner
//! transport. Signing happens on the fully-prepared request so the signature
//! covers the exact body bytes and content headers that will be sent.

use std::time::SystemTime;

use aws_credential_types::Credentials;
use aws_sigv4::http_request::SignableBody;
use aws_sigv4::http_request::SignableRequest;
use aws_sigv4::http_request::SigningSettings;
use aws_sigv4::http_request::sign;
use aws_sigv4::sign::v4;
use bytes::Bytes;
use ody_client::HttpTransport;
use ody_client::Request;
use ody_client::RequestBody;
use ody_client::Response;
use ody_client::StreamResponse;
use ody_client::TransportError;
use ody_model_provider_info::ModelProviderAwsConfig;

/// HTTP transport that SigV4-signs every request for AWS Bedrock.
#[derive(Debug, Clone)]
pub struct SigV4Transport<T> {
    inner: T,
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
    region: String,
    service: String,
    signing_time: Option<SystemTime>,
}

impl<T> SigV4Transport<T> {
    /// Create a signing transport from Bedrock AWS configuration.
    pub fn new(inner: T, aws: &ModelProviderAwsConfig) -> Self {
        Self {
            inner,
            access_key_id: aws.access_key_id.clone(),
            secret_access_key: aws.secret_access_key.clone(),
            session_token: aws.session_token.clone(),
            region: aws.region.clone(),
            service: "bedrock".to_string(),
            signing_time: None,
        }
    }

    #[cfg(test)]
    fn with_signing_time(mut self, time: SystemTime) -> Self {
        self.signing_time = Some(time);
        self
    }

    fn sign(&self, req: Request) -> Result<Request, TransportError> {
        let prepared = req.into_prepared().map_err(TransportError::Build)?;
        let body = prepared.body.as_ref().map(body_bytes).unwrap_or_default();

        let credentials = Credentials::new(
            self.access_key_id.clone(),
            self.secret_access_key.clone(),
            self.session_token.clone(),
            None,
            "ody-bedrock",
        );
        let identity = credentials.into();
        let params: aws_sigv4::http_request::SigningParams<'_> =
            v4::SigningParams::builder()
                .identity(&identity)
                .region(&self.region)
                .name(&self.service)
                .time(self.signing_time.unwrap_or_else(SystemTime::now))
                .settings(SigningSettings::default())
                .build()
                .expect("SigV4 signing params are complete")
                .into();
        let header_refs: Vec<(&str, &str)> = prepared
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.to_str().unwrap_or_default()))
            .collect();
        let signable = SignableRequest::new(
            prepared.method.as_str(),
            prepared.url.as_str(),
            header_refs.into_iter(),
            SignableBody::Bytes(&body),
        )
        .map_err(|err| TransportError::Build(format!("SigV4 signable request: {err}")))?;
        let (instructions, _signature) = sign(signable, &params)
            .map_err(|err| TransportError::Build(format!("SigV4 signing failed: {err}")))?
            .into_parts();
        let mut headers = prepared.headers;
        for (name, value) in instructions.headers() {
            if let (Ok(name), Ok(value)) =
                (http::HeaderName::try_from(name), http::HeaderValue::try_from(value))
            {
                headers.insert(name, value);
            }
        }

        Ok(Request {
            method: prepared.method,
            url: prepared.url,
            headers,
            body: Some(RequestBody::Raw(body)),
            compression: ody_client::RequestCompression::None,
            timeout: prepared.timeout,
        })
    }
}

fn body_bytes(body: &RequestBody) -> Bytes {
    match body {
        RequestBody::EncodedJson(encoded) => Bytes::copy_from_slice(encoded.as_bytes()),
        RequestBody::Raw(bytes) => bytes.clone(),
        RequestBody::Json(value) => Bytes::from(serde_json::to_vec(value).unwrap_or_default()),
    }
}

impl<T: HttpTransport> HttpTransport for SigV4Transport<T> {
    async fn execute(&self, req: Request) -> Result<Response, TransportError> {
        self.inner.execute(self.sign(req)?).await
    }

    async fn stream(&self, req: Request) -> Result<StreamResponse, TransportError> {
        self.inner.stream(self.sign(req)?).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use http::header::AUTHORIZATION;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct RecordingTransport {
        captured: Arc<Mutex<Option<Request>>>,
    }

    impl HttpTransport for RecordingTransport {
        async fn execute(&self, req: Request) -> Result<Response, TransportError> {
            *self.captured.lock().expect("lock") = Some(req);
            Ok(Response {
                status: http::StatusCode::OK,
                headers: http::HeaderMap::new(),
                body: Bytes::new(),
            })
        }

        async fn stream(&self, req: Request) -> Result<StreamResponse, TransportError> {
            *self.captured.lock().expect("lock") = Some(req);
            Ok(StreamResponse {
                status: http::StatusCode::OK,
                headers: http::HeaderMap::new(),
                bytes: futures::stream::empty().boxed(),
            })
        }
    }

    fn aws_config() -> ModelProviderAwsConfig {
        ModelProviderAwsConfig {
            access_key_id: "AKIAEXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: None,
            region: "us-east-1".to_string(),
        }
    }

    fn bedrock_request() -> Request {
        Request::new(
            http::Method::POST,
            "https://bedrock-runtime.us-east-1.amazonaws.com/openai/v1/chat/completions"
                .to_string(),
        )
        .with_raw_body(Bytes::from_static(
            br#"{"model":"us.anthropic.claude-sonnet-4-5"}"#,
        ))
    }

    fn captured_request(transport: &RecordingTransport) -> Request {
        transport
            .captured
            .lock()
            .expect("lock")
            .clone()
            .expect("request was captured")
    }

    #[tokio::test]
    async fn signs_request_with_sigv4_headers() {
        let inner = RecordingTransport::default();
        let transport = SigV4Transport::new(inner.clone(), &aws_config()).with_signing_time(
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
        );

        transport
            .execute(bedrock_request())
            .await
            .expect("execute should succeed");

        let request = captured_request(&inner);
        let auth = request
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .expect("authorization header present");
        assert!(
            auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/"),
            "unexpected authorization scheme: {auth}"
        );
        assert!(
            auth.contains("/us-east-1/bedrock/aws4_request"),
            "credential scope should target bedrock in us-east-1: {auth}"
        );
        assert!(auth.contains("Signature="), "signature missing: {auth}");
        assert!(
            request.headers.contains_key("x-amz-date"),
            "x-amz-date header present"
        );
        // The payload hash is folded into the canonical request even though
        // the `x-amz-content-sha256` header itself is not emitted for the
        // `bedrock` service: a different body must yield a different signature.
        let inner_other = RecordingTransport::default();
        let transport_other =
            SigV4Transport::new(inner_other.clone(), &aws_config()).with_signing_time(
                SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
            );
        transport_other
            .execute(
                bedrock_request()
                    .with_raw_body(Bytes::from_static(br#"{"model":"other-model"}"#)),
            )
            .await
            .expect("execute should succeed");
        let auth_other = captured_request(&inner_other).headers.get(AUTHORIZATION).cloned();
        assert_ne!(
            request.headers.get(AUTHORIZATION),
            auth_other.as_ref(),
            "different bodies must produce different signatures"
        );
        let body = body_bytes(request.body.as_ref().expect("body preserved"));
        assert_eq!(
            body,
            Bytes::from_static(br#"{"model":"us.anthropic.claude-sonnet-4-5"}"#)
        );
    }

    #[tokio::test]
    async fn adds_security_token_header_when_session_token_configured() {
        let mut aws = aws_config();
        aws.session_token = Some("AQoDYXdzEPT//////////wEXAMPLE".to_string());
        let inner = RecordingTransport::default();
        let transport = SigV4Transport::new(inner.clone(), &aws);

        transport
            .execute(bedrock_request())
            .await
            .expect("execute should succeed");

        let request = captured_request(&inner);
        assert_eq!(
            request
                .headers
                .get("x-amz-security-token")
                .and_then(|value| value.to_str().ok()),
            Some("AQoDYXdzEPT//////////wEXAMPLE")
        );
    }

    #[tokio::test]
    async fn omits_security_token_header_when_no_session_token() {
        let inner = RecordingTransport::default();
        let transport = SigV4Transport::new(inner.clone(), &aws_config());

        transport
            .execute(bedrock_request())
            .await
            .expect("execute should succeed");

        let request = captured_request(&inner);
        assert!(!request.headers.contains_key("x-amz-security-token"));
    }

    #[tokio::test]
    async fn stream_requests_are_signed_too() {
        let inner = RecordingTransport::default();
        let transport = SigV4Transport::new(inner.clone(), &aws_config());

        transport
            .stream(bedrock_request())
            .await
            .expect("stream should succeed");

        let request = captured_request(&inner);
        assert!(request.headers.contains_key(AUTHORIZATION));
    }

    #[tokio::test]
    async fn signing_is_deterministic_for_fixed_time() {
        let inner_a = RecordingTransport::default();
        let inner_b = RecordingTransport::default();
        let fixed = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let transport_a =
            SigV4Transport::new(inner_a.clone(), &aws_config()).with_signing_time(fixed);
        let transport_b =
            SigV4Transport::new(inner_b.clone(), &aws_config()).with_signing_time(fixed);

        transport_a.execute(bedrock_request()).await.expect("ok");
        transport_b.execute(bedrock_request()).await.expect("ok");

        let auth_a = captured_request(&inner_a).headers.get(AUTHORIZATION).cloned();
        let auth_b = captured_request(&inner_b).headers.get(AUTHORIZATION).cloned();
        assert_eq!(auth_a, auth_b, "same request and time must sign identically");
    }
}
