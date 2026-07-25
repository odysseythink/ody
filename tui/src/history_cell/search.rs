//! Web-search activity history cells.

use super::*;

fn web_search_header(completed: bool) -> &'static str {
    if completed {
        "Searched the web"
    } else {
        "Searching the web"
    }
}

fn web_search_action_detail(action: &WebSearchAction) -> String {
    match action {
        WebSearchAction::Search { query, queries, result_count } => {
            let query_text = query.clone().filter(|q| !q.is_empty()).unwrap_or_else(|| {
                let items = queries.as_ref();
                let first = items
                    .and_then(|queries| queries.first())
                    .cloned()
                    .unwrap_or_default();
                if items.is_some_and(|queries| queries.len() > 1) && !first.is_empty() {
                    format!("{first} ...")
                } else {
                    first
                }
            });
            if let Some(count) = result_count {
                let count_text = match *count {
                    0 => "no results".to_string(),
                    1 => "1 result".to_string(),
                    n => format!("{n} results"),
                };
                if query_text.is_empty() {
                    count_text
                } else {
                    format!("{query_text} — {count_text}")
                }
            } else {
                query_text
            }
        }
        WebSearchAction::OpenPage { url } => url.clone().unwrap_or_default(),
        WebSearchAction::FindInPage { url, pattern } => match (pattern, url) {
            (Some(pattern), Some(url)) => format!("'{pattern}' in {url}"),
            (Some(pattern), None) => format!("'{pattern}'"),
            (None, Some(url)) => url.clone(),
            (None, None) => String::new(),
        },
        WebSearchAction::Other => String::new(),
    }
}

fn web_search_detail(action: Option<&WebSearchAction>, query: &str) -> String {
    let detail = action.map(web_search_action_detail).unwrap_or_default();
    if detail.is_empty() {
        query.to_string()
    } else {
        detail
    }
}

#[derive(Debug)]
pub(crate) struct WebSearchCell {
    call_id: String,
    query: String,
    action: Option<WebSearchAction>,
    start_time: Instant,
    completed: bool,
    animations_enabled: bool,
}

impl WebSearchCell {
    pub(crate) fn new(
        call_id: String,
        query: String,
        action: Option<WebSearchAction>,
        animations_enabled: bool,
    ) -> Self {
        Self {
            call_id,
            query,
            action,
            start_time: Instant::now(),
            completed: false,
            animations_enabled,
        }
    }

    pub(crate) fn call_id(&self) -> &str {
        &self.call_id
    }

    pub(crate) fn update(&mut self, action: WebSearchAction, query: String) {
        self.action = Some(action);
        self.query = query;
    }

    pub(crate) fn complete(&mut self) {
        self.completed = true;
    }
}

impl HistoryCell for WebSearchCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let bullet = if self.completed {
            "•".dim()
        } else {
            activity_indicator(
                Some(self.start_time),
                MotionMode::from_animations_enabled(self.animations_enabled),
                ReducedMotionIndicator::StaticBullet,
            )
            .unwrap_or_else(|| "•".dim())
        };
        let header = web_search_header(self.completed);
        let detail = web_search_detail(self.action.as_ref(), &self.query);
        let text: Text<'static> = if detail.is_empty() {
            Line::from(vec![header.bold()]).into()
        } else {
            let separator = if self.completed { " for " } else { " " };
            Line::from(vec![header.bold(), separator.into(), detail.into()]).into()
        };
        PrefixedWrappedHistoryCell::new(text, vec![bullet, " ".into()], "  ").display_lines(width)
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        let header = web_search_header(self.completed);
        let detail = web_search_detail(self.action.as_ref(), &self.query);
        if detail.is_empty() {
            vec![Line::from(header)]
        } else {
            let separator = if self.completed { " for " } else { " " };
            vec![Line::from(format!("{header}{separator}{detail}"))]
        }
    }
}

pub(crate) fn new_active_web_search_call(
    call_id: String,
    query: String,
    animations_enabled: bool,
) -> WebSearchCell {
    WebSearchCell::new(call_id, query, /*action*/ None, animations_enabled)
}

pub(crate) fn new_web_search_call(
    call_id: String,
    query: String,
    action: WebSearchAction,
) -> WebSearchCell {
    let mut cell = WebSearchCell::new(
        call_id,
        query,
        Some(action),
        /*animations_enabled*/ false,
    );
    cell.complete();
    cell
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_includes_result_count_when_present() {
        let action = WebSearchAction::Search {
            query: Some("rust".to_string()),
            queries: None,
            result_count: Some(5),
        };
        assert_eq!(
            web_search_detail(Some(&action), "fallback"),
            "rust — 5 results"
        );
    }

    #[test]
    fn detail_uses_one_result_singular() {
        let action = WebSearchAction::Search {
            query: Some("rust".to_string()),
            queries: None,
            result_count: Some(1),
        };
        assert_eq!(
            web_search_detail(Some(&action), "fallback"),
            "rust — 1 result"
        );
    }

    #[test]
    fn detail_shows_no_results_for_zero() {
        let action = WebSearchAction::Search {
            query: Some("rust".to_string()),
            queries: None,
            result_count: Some(0),
        };
        assert_eq!(
            web_search_detail(Some(&action), "fallback"),
            "rust — no results"
        );
    }

    #[test]
    fn detail_falls_back_to_query_without_count() {
        let action = WebSearchAction::Search {
            query: Some("rust".to_string()),
            queries: None,
            result_count: None,
        };
        assert_eq!(web_search_detail(Some(&action), "fallback"), "rust");
    }

    #[test]
    fn detail_falls_back_to_passed_query_when_action_is_other() {
        assert_eq!(
            web_search_detail(Some(&WebSearchAction::Other), "fallback query"),
            "fallback query"
        );
    }
}
