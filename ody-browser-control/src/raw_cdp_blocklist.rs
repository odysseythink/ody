//! Raw CDP method blocklist for the `browser__execute_raw_cdp` tool.
//!
//! These methods are considered too sensitive to be invoked by the model even
//! when `BrowserUseFullCdpAccess` is enabled. The blocklist is checked before
//! the guardian approval gate so that blocked methods fail fast without
//! generating a review prompt.

/// Return `Some(pattern)` if `method` is in the raw CDP blocklist.
///
/// The comparison is case-insensitive and matches substrings so that domain
/// prefixes such as `Storage.getCookies` and `Network.getAllCookies` are both
/// caught by the `getcookies` wildcard rule.
pub fn is_raw_cdp_blocked(method: &str) -> Option<&'static str> {
    let lower = method.to_lowercase();
    let exact: &[&str] = &[
        "storage.getcookies",
        "storage.setcookies",
        "storage.deletecookies",
        "network.getallcookies",
        "network.getcookies",
        "fetch.continuerequest",
        "fetch.continueresponse",
        "fetch.fulfillrequest",
        "fetch.failrequest",
        "runtime.setcustomhandicapath", // if present in the CDP domain
    ];
    for pattern in exact {
        if lower == *pattern {
            return Some(pattern);
        }
    }
    if lower.contains("getcookies") {
        return Some("getcookies");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_storage_get_cookies() {
        assert!(is_raw_cdp_blocked("Storage.getCookies").is_some());
    }

    #[test]
    fn blocks_network_get_all_cookies() {
        assert!(is_raw_cdp_blocked("Network.getAllCookies").is_some());
    }

    #[test]
    fn blocks_fetch_continue_request() {
        assert!(is_raw_cdp_blocked("Fetch.continueRequest").is_some());
    }

    #[test]
    fn blocks_any_getcookies_substring() {
        assert!(is_raw_cdp_blocked("SomeDomain.getCookies").is_some());
    }

    #[test]
    fn allows_runtime_evaluate() {
        assert!(is_raw_cdp_blocked("Runtime.evaluate").is_none());
    }

    #[test]
    fn allows_dom_query() {
        assert!(is_raw_cdp_blocked("DOM.querySelector").is_none());
    }
}
