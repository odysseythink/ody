//! Network log redaction for the browser control event buffer.

use std::collections::HashMap;

use crate::event_buffer::NetworkEntry;

const REDACTED_HEADERS: &[&str] = &[
    "authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "x-auth-token",
    "x-csrf-token",
    "x-requested-with",
];

const REDACTED_VALUE: &str = "[REDACTED]";

/// Redact sensitive headers in `entry` and remove response bodies.
///
/// This is the storage-time redaction layer: every `NetworkEntry` that leaves
/// the event buffer is scrubbed before it is returned to the model. The
/// original Chromium oxide events are not kept.
pub fn redact_network_entry(entry: &mut NetworkEntry) {
    // 1. Network entries are summaries; do not keep response bodies.
    entry.response_body = None;
    if entry
        .request_body
        .as_ref()
        .map(|b| b.len())
        .unwrap_or(0)
        > 1024
    {
        entry.request_body = Some("[truncated]".to_string());
    }

    // 2. Redact sensitive request and response headers.
    if let Some(headers) = entry.request_headers.as_mut() {
        redact_headers(headers);
    }
    if let Some(headers) = entry.response_headers.as_mut() {
        redact_headers(headers);
    }
}

fn redact_headers(headers: &mut HashMap<String, String>) {
    for key in REDACTED_HEADERS {
        let lower_key = key.to_lowercase();
        for (header_name, value) in headers.iter_mut() {
            if header_name.to_lowercase() == lower_key {
                *value = REDACTED_VALUE.to_string();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn entry_with_headers() -> NetworkEntry {
        NetworkEntry {
            request_id: "1".to_string(),
            url: "https://example.com/api".to_string(),
            method: Some("GET".to_string()),
            status: Some(200),
            status_text: Some("OK".to_string()),
            resource_type: Some("xhr".to_string()),
            timestamp: 0.0,
            request_body: None,
            response_body: Some("secret".to_string()),
            request_headers: Some(HashMap::from([
                ("Authorization".to_string(), "Bearer xyz".to_string()),
                ("Accept".to_string(), "application/json".to_string()),
            ])),
            response_headers: Some(HashMap::from([
                ("Set-Cookie".to_string(), "session=abc".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ])),
            request_headers_size: 0,
            response_headers_size: 0,
            from_cache: None,
        }
    }

    #[test]
    fn redacts_sensitive_request_headers() {
        let mut entry = entry_with_headers();
        redact_network_entry(&mut entry);
        let req = entry.request_headers.unwrap();
        assert_eq!(req.get("Authorization").unwrap(), "[REDACTED]");
        assert_eq!(req.get("Accept").unwrap(), "application/json");
    }

    #[test]
    fn redacts_sensitive_response_headers() {
        let mut entry = entry_with_headers();
        redact_network_entry(&mut entry);
        let res = entry.response_headers.unwrap();
        assert_eq!(res.get("Set-Cookie").unwrap(), "[REDACTED]");
        assert_eq!(res.get("Content-Type").unwrap(), "application/json");
    }

    #[test]
    fn removes_response_body() {
        let mut entry = entry_with_headers();
        redact_network_entry(&mut entry);
        assert!(entry.response_body.is_none());
    }

    #[test]
    fn truncates_long_request_body() {
        let mut entry = entry_with_headers();
        entry.request_body = Some("a".repeat(2000));
        redact_network_entry(&mut entry);
        assert_eq!(entry.request_body, Some("[truncated]".to_string()));
    }
}
