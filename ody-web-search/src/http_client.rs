use std::time::Duration;

/// Default HTTP client used by all web search providers.
///
/// Falls back to `reqwest::Client::new()` if the custom builder fails, so we never panic.
pub fn default_http_client() -> reqwest::Client {
    match reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            tracing::warn!("failed to build custom reqwest client, using default: {err}");
            reqwest::Client::new()
        }
    }
}

/// HTTP client for fetching source pages. Redirects are handled manually so every destination can
/// be checked against the public-network policy before a request is sent.
pub fn web_fetch_http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Ody-WebFetch/1.0")
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_http_client_builds() {
        let _client = default_http_client();
        // `reqwest::Client` does not expose its timeout in this version;
        // building without panic is the only meaningful assertion.
    }

    #[test]
    fn web_fetch_http_client_builds() {
        let _client = web_fetch_http_client().expect("WebFetch client should build");
    }
}
