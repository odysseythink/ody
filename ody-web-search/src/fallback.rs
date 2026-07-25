use crate::error::WebSearchError;
use crate::provider::{
    SharedWebSearchProvider, WebSearchOptions, WebSearchProvider, WebSearchResult,
};

/// Provider that tries `primary`, and only calls `secondary` if `primary` fails.
/// Does not retry, merge results, or degrade to empty results.
#[derive(Debug)]
pub struct FallbackWebSearchProvider {
    primary: SharedWebSearchProvider,
    secondary: Option<SharedWebSearchProvider>,
}

impl FallbackWebSearchProvider {
    pub fn new(
        primary: SharedWebSearchProvider,
        secondary: Option<SharedWebSearchProvider>,
    ) -> Self {
        Self { primary, secondary }
    }
}

#[async_trait::async_trait]
impl WebSearchProvider for FallbackWebSearchProvider {
    fn name(&self) -> &str {
        "fallback"
    }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        match self.primary.search(query, options).await {
            Ok(results) => Ok(results),
            Err(primary_err) => match &self.secondary {
                Some(secondary) => secondary.search(query, options).await,
                None => Err(primary_err),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::WebSearchOptions;
    use crate::provider::WebSearchResult;
    use std::sync::Arc;

    #[derive(Debug)]
    struct OkProvider(&'static str, Vec<WebSearchResult>);
    #[async_trait::async_trait]
    impl WebSearchProvider for OkProvider {
        fn name(&self) -> &str {
            self.0
        }
        async fn search(
            &self,
            _query: &str,
            _options: &WebSearchOptions,
        ) -> Result<Vec<WebSearchResult>, WebSearchError> {
            Ok(self.1.clone())
        }
    }

    #[derive(Debug)]
    struct ErrProvider(&'static str, WebSearchError);
    #[async_trait::async_trait]
    impl WebSearchProvider for ErrProvider {
        fn name(&self) -> &str {
            self.0
        }
        async fn search(
            &self,
            _query: &str,
            _options: &WebSearchOptions,
        ) -> Result<Vec<WebSearchResult>, WebSearchError> {
            Err(self.1.clone())
        }
    }

    fn result(name: &str) -> WebSearchResult {
        WebSearchResult {
            title: name.to_string(),
            url: format!("https://{name}.example"),
            snippet: name.to_string(),
            date: None,
            content: None,
        }
    }

    #[tokio::test]
    async fn primary_success_ignores_secondary() -> Result<(), WebSearchError> {
        let primary = Arc::new(OkProvider("primary", vec![result("primary")]));
        let secondary = Arc::new(ErrProvider("secondary", WebSearchError::Timeout));
        let fallback = FallbackWebSearchProvider::new(primary, Some(secondary));
        let results = fallback.search("q", &WebSearchOptions::default()).await?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "primary");
        Ok(())
    }

    #[tokio::test]
    async fn falls_back_to_secondary_on_primary_failure() -> Result<(), WebSearchError> {
        let primary = Arc::new(ErrProvider("primary", WebSearchError::Timeout));
        let secondary = Arc::new(OkProvider("secondary", vec![result("secondary")]));
        let fallback = FallbackWebSearchProvider::new(primary, Some(secondary));
        let results = fallback.search("q", &WebSearchOptions::default()).await?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "secondary");
        Ok(())
    }

    #[tokio::test]
    async fn returns_primary_error_when_no_secondary() {
        let primary = Arc::new(ErrProvider("primary", WebSearchError::Timeout));
        let fallback = FallbackWebSearchProvider::new(primary, None);
        let result = fallback.search("q", &WebSearchOptions::default()).await;
        assert_eq!(result, Err(WebSearchError::Timeout));
    }

    #[tokio::test]
    async fn returns_secondary_error_when_both_fail() {
        let primary = Arc::new(ErrProvider("primary", WebSearchError::Timeout));
        let secondary = Arc::new(ErrProvider("secondary", WebSearchError::Auth));
        let fallback = FallbackWebSearchProvider::new(primary, Some(secondary));
        let result = fallback.search("q", &WebSearchOptions::default()).await;
        assert_eq!(result, Err(WebSearchError::Auth));
    }
}
