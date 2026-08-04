use std::net::{Ipv4Addr, Ipv6Addr};
use url::Host;

/// Check whether a URL is allowed given the effective policy.
///
/// When `allow_local_network` is `false`, loopback, link-local, and private
/// addresses are rejected. `javascript:`, `file:`, and other non-HTTP schemes
/// are always rejected.
///
/// Returns `Ok(())` when the URL is allowed, or `Err(reason)` with a short
/// human-readable explanation.
pub fn check_url_is_allowed(url: &str, allow_local_network: bool) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("scheme '{other}' is not allowed for browser navigation")),
    }

    if allow_local_network {
        return Ok(());
    }

    if let Some(host) = parsed.host() {
        match host {
            Host::Domain(domain) => {
                if is_local_domain(domain) {
                    return Err(format!(
                        "local/private host '{domain}' is blocked (enable allow_local_network to override)"
                    ));
                }
            }
            Host::Ipv4(ip) => {
                if is_ipv4_local(ip) {
                    return Err(format!(
                        "local/private IPv4 address '{ip}' is blocked (enable allow_local_network to override)"
                    ));
                }
            }
            Host::Ipv6(ip) => {
                if is_ipv6_local(ip) {
                    return Err(format!(
                        "local/private IPv6 address '{ip}' is blocked (enable allow_local_network to override)"
                    ));
                }
            }
        }
    }

    Ok(())
}

fn is_local_domain(domain: &str) -> bool {
    let lower = domain.to_lowercase();
    lower == "localhost"
        || lower.ends_with(".localhost")
        || lower == "127.0.0.1"
        || lower == "::1"
}

fn is_ipv4_local(ip: Ipv4Addr) -> bool {
    ip.is_loopback()
        || ip.is_link_local()
        || ip.is_private()
        || ip.is_broadcast()
        || ip.is_documentation()
}

fn is_ipv6_local(ip: Ipv6Addr) -> bool {
    ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
}

/// First-layer check that prevents JS expressions from reading sensitive
/// browser storage.
///
/// Returns `Ok(())` if the expression is allowed, or `Err(reason)` with a
/// short explanation when it is blocked.
pub fn check_js_allowed(js: &str) -> Result<(), String> {
    let lower = js.to_lowercase();
    if lower.contains("document.cookie") || lower.contains("document.cookies") {
        return Err("reading document.cookie is not allowed".to_string());
    }
    if lower.contains("window.cookie") || lower.contains("window.cookies") {
        return Err("reading window.cookie is not allowed".to_string());
    }
    if lower.contains("cookie") && lower.contains("document") {
        return Err("expression contains blocked cookie access through document".to_string());
    }
    if lower.contains("localstorage")
        || lower.contains("sessionstorage")
        || lower.contains("indexeddb")
    {
        return Err("reading web storage APIs is not allowed".to_string());
    }

    // Common obfuscation / indirect access vectors used to reach sensitive APIs.
    // These are representative, not exhaustive. Guardian approval remains the
    // authoritative control.
    if lower.contains("eval(")
        || lower.contains("function(")
        || lower.contains("new function")
        || lower.contains("atob(")
        || lower.contains("settimeout(")
        || lower.contains("setinterval(")
        || lower.contains("contentwindow")
    {
        return Err(
            "expression contains obfuscation or indirect access pattern that is not allowed"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_localhost() {
        assert!(check_url_is_allowed("http://localhost/foo", false).is_err());
        assert!(check_url_is_allowed("http://localhost:8080/foo", false).is_err());
    }

    #[test]
    fn allows_public_https() {
        assert!(check_url_is_allowed("https://example.com", false).is_ok());
    }

    #[test]
    fn blocks_private_ipv4() {
        assert!(check_url_is_allowed("http://192.168.1.1", false).is_err());
        assert!(check_url_is_allowed("http://10.0.0.1", false).is_err());
        assert!(check_url_is_allowed("http://172.16.0.1", false).is_err());
    }

    #[test]
    fn blocks_loopback_ipv4() {
        assert!(check_url_is_allowed("http://127.0.0.1", false).is_err());
    }

    #[test]
    fn blocks_link_local_ipv4() {
        assert!(check_url_is_allowed("http://169.254.1.1", false).is_err());
    }

    #[test]
    fn blocks_loopback_ipv6() {
        assert!(check_url_is_allowed("http://[::1]", false).is_err());
    }

    #[test]
    fn allows_local_when_overridden() {
        assert!(check_url_is_allowed("http://127.0.0.1", true).is_ok());
        assert!(check_url_is_allowed("http://localhost", true).is_ok());
    }

    #[test]
    fn blocks_non_http_schemes() {
        assert!(check_url_is_allowed("file:///etc/passwd", false).is_err());
        assert!(check_url_is_allowed("javascript:alert(1)", false).is_err());
    }

    #[test]
    fn blocks_cookie_expression() {
        assert!(check_js_allowed("document.cookie").is_err());
    }

    #[test]
    fn blocks_storage_expressions() {
        assert!(check_js_allowed("window.localStorage.setItem('x', 1)").is_err());
        assert!(check_js_allowed("sessionStorage['x']").is_err());
        assert!(check_js_allowed("indexedDB.open('db')").is_err());
    }

    #[test]
    fn allows_benign_js() {
        assert!(check_js_allowed("1 + 1").is_ok());
        assert!(check_js_allowed("document.querySelector('h1').innerText").is_ok());
        assert!(check_js_allowed("'hello'.toUpperCase()").is_ok());
    }

    #[test]
    fn blocks_cookie_with_document_combination() {
        assert!(check_js_allowed("document['cookie']").is_err());
    }

    #[test]
    fn blocks_obfuscation_patterns() {
        assert!(check_js_allowed("eval('document.cookie')").is_err());
        assert!(check_js_allowed("setTimeout('document.cookie', 0)").is_err());
        assert!(check_js_allowed("iframe.contentWindow.document.cookie").is_err());
    }

    #[test]
    fn blocks_window_cookie() {
        assert!(check_js_allowed("window.cookie").is_err());
    }

    #[test]
    fn blocks_atob_obfuscation() {
        assert!(check_js_allowed("atob('ZG9jdW1lbnQuY29va2ll')").is_err());
    }

    #[test]
    fn allows_benign_dom_methods() {
        assert!(check_js_allowed("document.querySelectorAll('a').length").is_ok());
        assert!(check_js_allowed("window.location.href").is_ok());
    }
}
