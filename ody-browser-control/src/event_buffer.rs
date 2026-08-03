use std::sync::Arc;

use chromiumoxide::cdp::browser_protocol::log::LogEntry;
use chromiumoxide::cdp::browser_protocol::network::{
    EventRequestWillBeSent, EventResponseReceived,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::config::BrowserControlConfig;
use crate::error::BrowserControlError;

/// A single console log entry captured from the page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleEntry {
    pub level: String,
    pub text: String,
    pub source: String,
    pub timestamp: f64,
    pub url: Option<String>,
    pub line_number: Option<i64>,
}

/// A single network activity summary captured from the page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkEntry {
    pub request_id: String,
    pub url: String,
    pub method: Option<String>,
    pub status: Option<i64>,
    pub status_text: Option<String>,
    pub resource_type: Option<String>,
    pub timestamp: f64,
    pub request_headers_size: usize,
    pub response_headers_size: usize,
    pub from_cache: Option<bool>,
}

/// Snapshot of buffered console and network entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogsSnapshot {
    pub console: Vec<ConsoleEntry>,
    pub network: Vec<NetworkEntry>,
}

enum BufferedEntry {
    Console(ConsoleEntry),
    Network(NetworkEntry),
}

struct StoredEntry {
    kind: BufferedEntry,
    byte_size: usize,
}

/// Thread-safe rolling buffer for CDP console and network events.
pub struct EventBuffer {
    entries: Vec<StoredEntry>,
    total_bytes: usize,
    max_entries: usize,
    max_bytes: usize,
    max_console_bytes: usize,
}

impl EventBuffer {
    pub fn new(config: &BrowserControlConfig) -> Self {
        Self {
            entries: Vec::new(),
            total_bytes: 0,
            max_entries: config.max_event_entries,
            max_bytes: config.max_event_buffer_bytes,
            max_console_bytes: config.max_console_message_bytes,
        }
    }

    pub fn snapshot(&self) -> LogsSnapshot {
        let mut console = Vec::new();
        let mut network = Vec::new();
        for stored in &self.entries {
            match &stored.kind {
                BufferedEntry::Console(c) => console.push(c.clone()),
                BufferedEntry::Network(n) => network.push(n.clone()),
            }
        }
        LogsSnapshot { console, network }
    }

    fn push(&mut self, kind: BufferedEntry) {
        let byte_size = estimate_entry_bytes(&kind);
        self.entries.push(StoredEntry { kind, byte_size });
        self.total_bytes = self.total_bytes.saturating_add(byte_size);
        self.evict_if_needed();
    }

    pub fn push_console(&mut self, entry: &LogEntry) {
        let mut text = entry.text.clone();
        if text.len() > self.max_console_bytes {
            let truncated = format!(
                "{}... [truncated {} bytes]",
                &text[..self.max_console_bytes.saturating_sub(32).min(text.len())],
                text.len() - self.max_console_bytes.saturating_sub(32).min(text.len())
            );
            text = truncated;
        }

        self.push(BufferedEntry::Console(ConsoleEntry {
            level: format!("{:?}", entry.level),
            text,
            source: format!("{:?}", entry.source),
            timestamp: *entry.timestamp.inner(),
            url: entry.url.clone(),
            line_number: entry.line_number,
        }));
    }

    pub fn push_network_request(&mut self, event: &EventRequestWillBeSent) {
        let request_headers_size = estimate_headers_size(&event.request.headers);
        self.push(BufferedEntry::Network(NetworkEntry {
            request_id: event.request_id.as_ref().to_string(),
            url: event.request.url.clone(),
            method: Some(event.request.method.clone()),
            status: None,
            status_text: None,
            resource_type: event.r#type.as_ref().map(|t| format!("{:?}", t)),
            timestamp: *event.timestamp.inner(),
            request_headers_size,
            response_headers_size: 0,
            from_cache: None,
        }));
    }

    pub fn push_network_response(&mut self, event: &EventResponseReceived) {
        let response_headers_size = estimate_headers_size(&event.response.headers);
        let request_headers_size = event
            .response
            .request_headers
            .as_ref()
            .map(estimate_headers_size)
            .unwrap_or(0);
        self.push(BufferedEntry::Network(NetworkEntry {
            request_id: event.request_id.as_ref().to_string(),
            url: event.response.url.clone(),
            method: None,
            status: Some(event.response.status),
            status_text: Some(event.response.status_text.clone()),
            resource_type: Some(format!("{:?}", event.r#type)),
            timestamp: *event.timestamp.inner(),
            request_headers_size,
            response_headers_size,
            from_cache: event.response.from_disk_cache.or(event.response.from_service_worker),
        }));
    }

    fn evict_if_needed(&mut self) {
        while self.entries.len() > self.max_entries
            || self.total_bytes > self.max_bytes
        {
            if !self.entries.is_empty() {
                let stored = self.entries.remove(0);
                self.total_bytes = self.total_bytes.saturating_sub(stored.byte_size);
            } else {
                break;
            }
        }
    }
}

fn estimate_headers_size(headers: &chromiumoxide::cdp::browser_protocol::network::Headers) -> usize {
    serde_json::to_string(headers.inner()).map(|s| s.len()).unwrap_or(0)
}

fn estimate_entry_bytes(entry: &BufferedEntry) -> usize {
    match entry {
        BufferedEntry::Console(c) => c.text.len().saturating_add(128),
        BufferedEntry::Network(n) => {
            n.url.len().saturating_add(256)
        }
    }
}

/// Subscribe to console and network events on the given page and store them in `buffer`.
///
/// Returns the spawned listener tasks so the caller can abort them on page close.
pub async fn subscribe(
    page: &chromiumoxide::page::Page,
    buffer: Arc<Mutex<EventBuffer>>,
) -> Result<Vec<JoinHandle<()>>, BrowserControlError> {
    use chromiumoxide::cdp::browser_protocol::{log, network};

    page.execute(log::EnableParams::default())
        .await
        .map_err(|e| BrowserControlError::from_command_error("Log.enable", e))?;
    page.execute(network::EnableParams::default())
        .await
        .map_err(|e| BrowserControlError::from_command_error("Network.enable", e))?;

    let mut tasks = Vec::new();

    let mut console_stream = page
        .event_listener::<log::EventEntryAdded>()
        .await
        .map_err(|e| BrowserControlError::from_command_error("console listener", e))?;
    let console_buffer = buffer.clone();
    tasks.push(tokio::spawn(async move {
        while let Some(event) = console_stream.next().await {
            let mut guard = console_buffer.lock().await;
            guard.push_console(&event.entry);
        }
    }));

    let mut request_stream = page
        .event_listener::<network::EventRequestWillBeSent>()
        .await
        .map_err(|e| BrowserControlError::from_command_error("network request listener", e))?;
    let request_buffer = buffer.clone();
    tasks.push(tokio::spawn(async move {
        while let Some(event) = request_stream.next().await {
            let mut guard = request_buffer.lock().await;
            guard.push_network_request(&event);
        }
    }));

    let mut response_stream = page
        .event_listener::<network::EventResponseReceived>()
        .await
        .map_err(|e| BrowserControlError::from_command_error("network response listener", e))?;
    let response_buffer = buffer.clone();
    tasks.push(tokio::spawn(async move {
        while let Some(event) = response_stream.next().await {
            let mut guard = response_buffer.lock().await;
            guard.push_network_response(&event);
        }
    }));

    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use chromiumoxide::cdp::browser_protocol::log::{LogEntryLevel, LogEntrySource};
    use chromiumoxide::cdp::js_protocol::runtime::Timestamp;
    use super::*;

    fn make_log_entry(text: &str) -> LogEntry {
        LogEntry {
            source: LogEntrySource::Javascript,
            level: LogEntryLevel::Info,
            text: text.to_string(),
            category: None,
            timestamp: Timestamp::new(0.0),
            url: None,
            line_number: None,
            stack_trace: None,
            network_request_id: None,
            worker_id: None,
            args: None,
        }
    }

    #[test]
    fn console_message_truncation() {
        let config = BrowserControlConfig {
            max_console_message_bytes: 16,
            ..Default::default()
        };
        let mut buffer = EventBuffer::new(&config);
        buffer.push_console(&make_log_entry("hello world this is long"));
        let snap = buffer.snapshot();
        assert!(snap.console[0].text.len() <= 64);
        assert!(snap.console[0].text.contains("truncated"));
    }

    #[test]
    fn entry_count_eviction() {
        let config = BrowserControlConfig {
            max_event_entries: 2,
            ..Default::default()
        };
        let mut buffer = EventBuffer::new(&config);
        buffer.push_console(&make_log_entry("first"));
        buffer.push_console(&make_log_entry("second"));
        buffer.push_console(&make_log_entry("third"));
        let snap = buffer.snapshot();
        assert_eq!(snap.console.len(), 2);
        assert_eq!(snap.console[0].text, "second");
        assert_eq!(snap.console[1].text, "third");
    }

    #[test]
    fn byte_eviction_drops_oldest() {
        let config = BrowserControlConfig {
            max_event_entries: 1000,
            max_event_buffer_bytes: 200,
            ..Default::default()
        };
        let mut buffer = EventBuffer::new(&config);
        for i in 0..10 {
            buffer.push_console(&make_log_entry(&format!("entry-{i}-padding")));
        }
        let snap = buffer.snapshot();
        // Only the newest entries should remain after the byte limit is hit.
        assert!(
            snap.console.len() < 10,
            "expected eviction, got {} entries",
            snap.console.len()
        );
    }
}
