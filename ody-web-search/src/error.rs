use reqwest::StatusCode;
use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq)]
pub enum WebSearchError {
    #[error("search network error: {message}")]
    Network { message: String },
    #[error("search timed out")]
    Timeout,
    #[error("search authentication failed: check your API key")]
    Auth,
    #[error("search rate limited: please retry later")]
    RateLimited,
    #[error("search provider error ({code}): {message}")]
    Provider { code: String, message: String },
    #[error("search unexpected error: {message}")]
    Unexpected { message: String },
}

impl WebSearchError {
    pub fn from_reqwest(err: &reqwest::Error) -> Self {
        if err.is_timeout() {
            Self::Timeout
        } else {
            Self::Network {
                message: err.to_string(),
            }
        }
    }

    pub fn from_http_status(status: StatusCode, body: &str) -> Self {
        match status.as_u16() {
            401 | 403 => Self::Auth,
            429 => Self::RateLimited,
            408 | 504 => Self::Timeout,
            500..=599 => Self::Provider {
                code: status.as_u16().to_string(),
                message: body.to_string(),
            },
            _ => Self::Network {
                message: format!("HTTP {}: {}", status, body),
            },
        }
    }

    /// Generic, model-facing message. Does not leak internal classification details.
    pub fn user_message(&self) -> String {
        match self {
            Self::Auth => "Search failed: please check your web search API key.".to_string(),
            _ => "Search temporarily unavailable. Please retry later.".to_string(),
        }
    }

    /// Internal classification prefix for tracing/structured logs only.
    pub fn log_message(&self) -> String {
        match self {
            Self::Network { message } => format!("Search failed (network): {message}"),
            Self::Timeout => "Search timed out.".to_string(),
            Self::Auth => "Search failed (authentication): check your API key.".to_string(),
            Self::RateLimited => "Search failed (rate limited): please retry later.".to_string(),
            Self::Provider { code, message } => {
                format!("Search failed ({}): {}", code, message)
            }
            Self::Unexpected { message } => format!("Search failed: {message}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_status_classifies_auth_rate_limit_and_timeout() {
        assert_eq!(
            WebSearchError::from_http_status(StatusCode::UNAUTHORIZED, ""),
            WebSearchError::Auth
        );
        assert_eq!(
            WebSearchError::from_http_status(StatusCode::FORBIDDEN, ""),
            WebSearchError::Auth
        );
        assert_eq!(
            WebSearchError::from_http_status(StatusCode::TOO_MANY_REQUESTS, ""),
            WebSearchError::RateLimited
        );
        assert_eq!(
            WebSearchError::from_http_status(StatusCode::REQUEST_TIMEOUT, ""),
            WebSearchError::Timeout
        );
        assert_eq!(
            WebSearchError::from_http_status(StatusCode::GATEWAY_TIMEOUT, ""),
            WebSearchError::Timeout
        );
        assert_eq!(
            WebSearchError::from_http_status(StatusCode::INTERNAL_SERVER_ERROR, "boom"),
            WebSearchError::Provider {
                code: "500".to_string(),
                message: "boom".to_string(),
            }
        );
    }

    #[test]
    fn user_message_is_generic_except_for_auth() {
        assert_eq!(
            WebSearchError::Auth.user_message(),
            "Search failed: please check your web search API key."
        );
        assert_eq!(
            WebSearchError::Timeout.user_message(),
            "Search temporarily unavailable. Please retry later."
        );
        assert_eq!(
            WebSearchError::Network {
                message: "dns".to_string()
            }
            .user_message(),
            "Search temporarily unavailable. Please retry later."
        );
    }
}
