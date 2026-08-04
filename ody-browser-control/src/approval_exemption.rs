//! Approval exemption rules for browser tools.
//!
//! These rules are intentionally conservative and only apply to `navigate`.
//! They are evaluated by the tool layer before the guardian approval gate so
//! that safe local/test URLs do not generate a user-visible review prompt.

use std::path::Path;

/// Return `Some(reason)` if `url` is exempt from the guardian approval gate.
///
/// Exemptions are only granted for `navigate` and are intentionally narrow:
/// - loopback hosts (`localhost`, `127.0.0.1`, `::1`)
/// - `file://` URLs whose path is inside `cwd` (when `cwd` is provided)
/// - `data:` URLs whose decoded body is shorter than 1 KiB
/// - test builds (`#[cfg(test)]`)
///
/// When a URL is exempt, the caller should still log the auto-approval and
/// proceed with the operation. This function does not authorize the URL itself
/// for navigation; callers should still run `check_url_is_allowed` afterwards.
pub fn is_approval_exempt(url: &str, cwd: Option<&Path>, is_test: bool) -> Option<&'static str> {
    if is_test {
        return Some("test build");
    }

    if let Some(loopback) = loopback_exemption(url) {
        return Some(loopback);
    }

    if let Some(file) = file_cwd_exemption(url, cwd) {
        return Some(file);
    }

    if let Some(data) = data_uri_exemption(url) {
        return Some(data);
    }

    None
}

fn loopback_exemption(url: &str) -> Option<&'static str> {
    let Ok(parsed) = url::Url::parse(url) else {
        return None;
    };
    match parsed.host()? {
        url::Host::Domain(domain) => {
            let lower = domain.to_lowercase();
            if lower == "localhost"
                || lower == "127.0.0.1"
                || lower == "::1"
                || lower.ends_with(".localhost")
            {
                return Some("loopback host");
            }
        }
        url::Host::Ipv4(ip) => {
            if ip.is_loopback() {
                return Some("loopback host");
            }
        }
        url::Host::Ipv6(ip) => {
            if ip.is_loopback() {
                return Some("loopback host");
            }
        }
    }
    None
}

fn file_cwd_exemption(url: &str, cwd: Option<&Path>) -> Option<&'static str> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "file" {
        return None;
    }
    let path = parsed.to_file_path().ok()?;
    let cwd = cwd?;
    if path.starts_with(cwd) {
        return Some("file URL under thread cwd");
    }
    None
}

fn data_uri_exemption(url: &str) -> Option<&'static str> {
    if !url.to_lowercase().starts_with("data:") {
        return None;
    }
    // data URLs are often `data:text/html,<body>...`. We only need a coarse
    // length check; the payload is already in the string.
    if url.len() > 1024 {
        return None;
    }
    Some("short data URI")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_loopback_localhost() {
        assert!(
            is_approval_exempt("http://localhost:8080/index.html", None, false).is_some()
        );
    }

    #[test]
    fn test_loopback_ipv4() {
        assert!(is_approval_exempt("http://127.0.0.1/foo", None, false).is_some());
        assert!(is_approval_exempt("http://127.0.0.1:8080/foo", None, false).is_some());
    }

    #[test]
    fn test_loopback_ipv6() {
        assert!(is_approval_exempt("http://[::1]/bar", None, false).is_some());
        assert!(is_approval_exempt("http://[::1]:8080/bar", None, false).is_some());
        assert!(is_approval_exempt("http://[0:0:0:0:0:0:0:1]/bar", None, false).is_some());
    }

    #[test]
    fn test_public_url_not_exempt() {
        assert!(is_approval_exempt("https://example.com", None, false).is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn test_file_url_under_cwd_is_exempt_unix() {
        let cwd = PathBuf::from("/tmp/workspace");
        let url = "file:///tmp/workspace/index.html";
        assert!(is_approval_exempt(url, Some(&cwd), false).is_some());
    }

    #[cfg(windows)]
    #[test]
    fn test_file_url_under_cwd_is_exempt_windows() {
        let cwd = PathBuf::from(r"C:\tmp\workspace");
        let url = "file:///C:/tmp/workspace/index.html";
        assert!(is_approval_exempt(url, Some(&cwd), false).is_some());
    }

    #[test]
    fn test_file_url_outside_cwd_not_exempt() {
        #[cfg(not(windows))]
        let cwd = PathBuf::from("/tmp/workspace");
        #[cfg(not(windows))]
        let url = "file:///etc/passwd";
        #[cfg(windows)]
        let cwd = PathBuf::from(r"C:\tmp\workspace");
        #[cfg(windows)]
        let url = "file:///C:/Windows/System32/notepad.exe";
        assert!(is_approval_exempt(url, Some(&cwd), false).is_none());
    }

    #[test]
    fn test_short_data_uri_exempt() {
        let url = "data:text/html,<h1>hi</h1>";
        assert!(is_approval_exempt(url, None, false).is_some());
    }

    #[test]
    fn test_long_data_uri_not_exempt() {
        let body = "a".repeat(1025);
        let url = format!("data:text/html,{body}");
        assert!(is_approval_exempt(&url, None, false).is_none());
    }

    #[test]
    fn test_test_build_is_exempt() {
        assert!(is_approval_exempt("https://example.com", None, true).is_some());
    }
}
