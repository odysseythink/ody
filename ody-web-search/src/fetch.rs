use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;

use ody_protocol::ToolName;
use ody_tools::{
    FunctionCallError, JsonToolOutput, ToolCall, ToolExecutor, ToolExecutorFuture, ToolExposure,
    ToolPayload, ToolSpec, parse_tool_input_schema,
};
use regex::Regex;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use serde::Serialize;
use serde_json::json;
use url::{Host, Url};

const DEFAULT_MAX_BYTES: usize = 512 * 1024;
const MAX_MAX_BYTES: usize = 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

pub struct WebFetchTool {
    client: reqwest::Client,
    allow_private_network: bool,
}

impl WebFetchTool {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            allow_private_network: false,
        }
    }

    #[cfg(test)]
    fn new_for_tests(client: reqwest::Client) -> Self {
        Self {
            client,
            allow_private_network: true,
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct WebFetchInput {
    url: String,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Debug, Serialize)]
struct WebFetchOutput {
    url: String,
    final_url: String,
    content_type: String,
    content: String,
    bytes_read: usize,
    truncated: bool,
}

impl ToolExecutor<ToolCall> for WebFetchTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("WebFetch")
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ody_tools::ResponsesApiTool {
            name: "WebFetch".to_string(),
            description: "Read an original public HTTP(S) source page after discovery with WebSearch. Redirects and destinations are validated, binary responses are rejected, and response size is bounded.".to_string(),
            strict: true,
            parameters: parse_tool_input_schema(&json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The public http:// or https:// source URL to read."
                    },
                    "max_bytes": {
                        "type": "integer",
                        "minimum": 4096,
                        "maximum": MAX_MAX_BYTES,
                        "description": "Maximum response bytes to read; defaults to 524288."
                    }
                },
                "required": ["url"],
                "additionalProperties": false
            }))
            .expect("WebFetch input schema is valid JSON"),
            defer_loading: None,
            output_schema: None,
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let client = self.client.clone();
        let allow_private_network = self.allow_private_network;
        Box::pin(async move {
            let arguments = match call.payload {
                ToolPayload::Function { arguments } => arguments,
                _ => {
                    return Err(FunctionCallError::Fatal(
                        "WebFetch only accepts function arguments".to_string(),
                    ));
                }
            };
            let input: WebFetchInput = serde_json::from_str(&arguments).map_err(|err| {
                FunctionCallError::Fatal(format!("invalid WebFetch input: {err}"))
            })?;
            let requested_url = parse_fetch_url(&input.url)?;
            let max_bytes = input.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
            if !(4096..=MAX_MAX_BYTES).contains(&max_bytes) {
                return Err(FunctionCallError::Fatal(format!(
                    "WebFetch max_bytes must be between 4096 and {MAX_MAX_BYTES}"
                )));
            }

            if call.guardian_approved_action_id.is_none() {
                return Err(FunctionCallError::NeedsApproval {
                    ticket: json!({
                        "action": "web_fetch",
                        "details": { "url": requested_url.as_str() }
                    }),
                });
            }

            let mut current = requested_url.clone();
            let mut response = None;
            for redirect_count in 0..=MAX_REDIRECTS {
                validate_destination(&current, allow_private_network).await?;
                let next_response = client.get(current.clone()).send().await.map_err(|err| {
                    FunctionCallError::Fatal(format!("WebFetch request failed: {err}"))
                })?;
                if next_response.status().is_redirection() {
                    if redirect_count == MAX_REDIRECTS {
                        return Err(FunctionCallError::Fatal(format!(
                            "WebFetch exceeded {MAX_REDIRECTS} redirects"
                        )));
                    }
                    let location = next_response
                        .headers()
                        .get(LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .ok_or_else(|| {
                            FunctionCallError::Fatal(
                                "WebFetch redirect has no valid Location header".to_string(),
                            )
                        })?;
                    current = current.join(location).map_err(|err| {
                        FunctionCallError::Fatal(format!("WebFetch redirect URL is invalid: {err}"))
                    })?;
                    parse_fetch_url(current.as_str())?;
                    continue;
                }
                response = Some(next_response);
                break;
            }

            let mut response = response.expect("redirect loop returns a response or an error");
            if !response.status().is_success() {
                return Err(FunctionCallError::Fatal(format!(
                    "WebFetch source returned HTTP {}",
                    response.status()
                )));
            }
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("text/plain")
                .to_string();
            if !is_textual_content_type(&content_type) {
                return Err(FunctionCallError::Fatal(format!(
                    "WebFetch only reads textual sources; received {content_type}"
                )));
            }
            let declared_length = response
                .headers()
                .get(CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<usize>().ok());
            let mut truncated = declared_length.is_some_and(|length| length > max_bytes);
            let mut body = Vec::with_capacity(declared_length.unwrap_or(max_bytes).min(max_bytes));
            while let Some(chunk) = response.chunk().await.map_err(|err| {
                FunctionCallError::Fatal(format!("WebFetch failed while reading response: {err}"))
            })? {
                let remaining = max_bytes.saturating_sub(body.len());
                if remaining == 0 {
                    truncated = true;
                    break;
                }
                if chunk.len() > remaining {
                    body.extend_from_slice(&chunk[..remaining]);
                    truncated = true;
                    break;
                }
                body.extend_from_slice(&chunk);
            }

            let bytes_read = body.len();
            let decoded = String::from_utf8_lossy(&body);
            let content = if content_type.to_ascii_lowercase().contains("html") {
                html_to_text(&decoded)
            } else {
                decoded.into_owned()
            };
            let output = WebFetchOutput {
                url: requested_url.to_string(),
                final_url: current.to_string(),
                content_type,
                content,
                bytes_read,
                truncated,
            };
            let value = serde_json::to_value(output).map_err(|err| {
                FunctionCallError::Fatal(format!("failed to serialize WebFetch output: {err}"))
            })?;
            Ok(Box::new(JsonToolOutput::new(value)) as Box<dyn ody_tools::ToolOutput>)
        })
    }
}

fn parse_fetch_url(value: &str) -> Result<Url, FunctionCallError> {
    let url = Url::parse(value)
        .map_err(|err| FunctionCallError::Fatal(format!("invalid WebFetch URL: {err}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(FunctionCallError::Fatal(
            "WebFetch only supports http:// and https:// URLs".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(FunctionCallError::Fatal(
            "WebFetch URLs must not contain credentials".to_string(),
        ));
    }
    if url.host().is_none() {
        return Err(FunctionCallError::Fatal(
            "WebFetch URL must contain a host".to_string(),
        ));
    }
    Ok(url)
}

async fn validate_destination(
    url: &Url,
    allow_private_network: bool,
) -> Result<(), FunctionCallError> {
    if allow_private_network {
        return Ok(());
    }
    let host = url
        .host()
        .ok_or_else(|| FunctionCallError::Fatal("WebFetch URL must contain a host".to_string()))?;
    match host {
        Host::Ipv4(address) => ensure_public_ip(IpAddr::V4(address)),
        Host::Ipv6(address) => ensure_public_ip(IpAddr::V6(address)),
        Host::Domain(domain) => {
            if domain.eq_ignore_ascii_case("localhost") || domain.ends_with(".localhost") {
                return Err(private_network_error());
            }
            let port = url.port_or_known_default().unwrap_or(80);
            let addresses = tokio::net::lookup_host((domain, port))
                .await
                .map_err(|err| {
                    FunctionCallError::Fatal(format!("WebFetch could not resolve host: {err}"))
                })?
                .map(|socket| socket.ip())
                .collect::<Vec<_>>();
            if addresses.is_empty() {
                return Err(FunctionCallError::Fatal(
                    "WebFetch host resolved to no addresses".to_string(),
                ));
            }
            for address in addresses {
                ensure_public_ip(address)?;
            }
            Ok(())
        }
    }
}

fn ensure_public_ip(address: IpAddr) -> Result<(), FunctionCallError> {
    let public = match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    };
    if public {
        Ok(())
    } else {
        Err(private_network_error())
    }
}

fn private_network_error() -> FunctionCallError {
    FunctionCallError::Fatal(
        "WebFetch blocks localhost, private, link-local, reserved, and multicast destinations"
            .to_string(),
    )
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _d] = address.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_public_ipv4(mapped);
    }
    let segments = address.segments();
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] == 0x2001 && segments[1] == 0x0db8))
}

fn is_textual_content_type(content_type: &str) -> bool {
    let content_type = content_type.to_ascii_lowercase();
    content_type.starts_with("text/")
        || content_type.contains("json")
        || content_type.contains("xml")
        || content_type.contains("javascript")
        || content_type.contains("xhtml")
        || content_type.contains("markdown")
}

fn html_to_text(html: &str) -> String {
    static SCRIPT: OnceLock<Regex> = OnceLock::new();
    static STYLE: OnceLock<Regex> = OnceLock::new();
    static BLOCK: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    static BLANKS: OnceLock<Regex> = OnceLock::new();
    let without_script = SCRIPT
        .get_or_init(|| Regex::new(r"(?is)<script\b[^>]*>.*?</script\s*>").expect("valid regex"))
        .replace_all(html, " ");
    let without_style = STYLE
        .get_or_init(|| Regex::new(r"(?is)<style\b[^>]*>.*?</style\s*>").expect("valid regex"))
        .replace_all(&without_script, " ");
    let with_lines = BLOCK
        .get_or_init(|| {
            Regex::new(r"(?is)<\s*(?:br|/p|/div|/li|/h[1-6]|/tr)\b[^>]*>").expect("valid regex")
        })
        .replace_all(&without_style, "\n");
    let without_tags = TAG
        .get_or_init(|| Regex::new(r"(?is)<[^>]+>").expect("valid regex"))
        .replace_all(&with_lines, " ");
    let decoded = without_tags
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    BLANKS
        .get_or_init(|| Regex::new(r"(?m)[ \t]+|\n{3,}").expect("valid regex"))
        .replace_all(&decoded, " ")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_protocol::protocol::TruncationPolicy;
    use ody_tools::NoopTurnItemEmitter;
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn tool_call(url: &str, approved: bool) -> ToolCall {
        ToolCall {
            turn_id: "turn-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name: ToolName::plain("WebFetch"),
            model: "test-model".to_string(),
            truncation_policy: TruncationPolicy::Bytes(0),
            conversation_history: ody_tools::ConversationHistory::default(),
            turn_item_emitter: Arc::new(NoopTurnItemEmitter),
            environments: Vec::new(),
            payload: ToolPayload::Function {
                arguments: json!({ "url": url, "max_bytes": 4096 }).to_string(),
            },
            guardian_approved_action_id: approved.then(|| "approved".to_string()),
        }
    }

    #[tokio::test]
    async fn requires_guardian_approval_before_network_access() {
        let tool = WebFetchTool::new(
            crate::http_client::web_fetch_http_client().expect("HTTP client should build"),
        );
        let error = match tool
            .handle(tool_call("https://example.com/docs", false))
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("approval must be required"),
        };
        assert!(matches!(error, FunctionCallError::NeedsApproval { .. }));
    }

    #[tokio::test]
    async fn fetches_html_and_returns_readable_structured_output() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/docs"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("<html><style>hidden</style><body><h1>Official docs</h1><script>hidden()</script><p>Supported behavior.</p></body></html>", "text/html"))
            .mount(&server)
            .await;
        let tool = WebFetchTool::new_for_tests(
            crate::http_client::web_fetch_http_client().expect("HTTP client should build"),
        );
        let output = tool
            .handle(tool_call(&format!("{}/docs", server.uri()), true))
            .await
            .expect("fetch should succeed");
        let value = output.code_mode_result(&ToolPayload::Function {
            arguments: String::new(),
        });
        assert!(value["content"].as_str().unwrap().contains("Official docs"));
        assert!(
            value["content"]
                .as_str()
                .unwrap()
                .contains("Supported behavior")
        );
        let content = value["content"].as_str().unwrap();
        assert!(
            !content.contains("hidden"),
            "sanitized content: {content:?}"
        );
        assert_eq!(value["truncated"], false);
    }

    #[tokio::test]
    async fn follows_validated_redirects_and_reports_final_url() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/start"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/final"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/final"))
            .respond_with(ResponseTemplate::new(200).set_body_string("official source"))
            .mount(&server)
            .await;
        let tool = WebFetchTool::new_for_tests(
            crate::http_client::web_fetch_http_client().expect("HTTP client should build"),
        );
        let output = tool
            .handle(tool_call(&format!("{}/start", server.uri()), true))
            .await
            .expect("redirected fetch should succeed");
        let value = output.code_mode_result(&ToolPayload::Function {
            arguments: String::new(),
        });
        assert!(value["final_url"].as_str().unwrap().ends_with("/final"));
        assert_eq!(value["content"], "official source");
    }

    #[tokio::test]
    async fn truncates_response_at_requested_byte_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/large"))
            .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(5000)))
            .mount(&server)
            .await;
        let tool = WebFetchTool::new_for_tests(
            crate::http_client::web_fetch_http_client().expect("HTTP client should build"),
        );
        let output = tool
            .handle(tool_call(&format!("{}/large", server.uri()), true))
            .await
            .expect("bounded fetch should succeed");
        let value = output.code_mode_result(&ToolPayload::Function {
            arguments: String::new(),
        });
        assert_eq!(value["bytes_read"], 4096);
        assert_eq!(value["content"].as_str().unwrap().len(), 4096);
        assert_eq!(value["truncated"], true);
    }

    #[tokio::test]
    async fn blocks_private_destinations_in_production_policy() {
        let error = validate_destination(&Url::parse("http://127.0.0.1/").unwrap(), false)
            .await
            .expect_err("private address must be blocked");
        assert!(error.to_string().contains("blocks localhost"));
    }

    #[test]
    fn rejects_non_http_urls_and_credentials() {
        assert!(parse_fetch_url("file:///etc/passwd").is_err());
        assert!(parse_fetch_url("https://user:secret@example.com/").is_err());
    }
}
