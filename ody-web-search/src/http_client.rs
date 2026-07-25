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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_http_client_builds() {
        let _client = default_http_client();
        // `reqwest::Client` does not expose its timeout in this version;
        // building without panic is the only meaningful assertion.
    }
}
