//! Filesystem ledger that makes extraction idempotent.
//!
//! Deliberately database-free: a session counts as already extracted when its
//! output file exists. This keeps the whole write pipeline inside the
//! assistant memory root, so no shared schema or state table is involved.
//!
//! The trade-off is a single-process assumption: odyBox ships a single-instance
//! guard, so concurrent writers are not expected. Unlike a lease-based ledger,
//! this cannot coordinate two processes racing the same session.

use std::path::PathBuf;

use ody_protocol::ThreadId;

/// Directory holding one markdown file per already-extracted session.
pub const EXTRACTED_SUBDIR: &str = "extracted";

#[derive(Clone, Debug)]
pub struct ExtractionLedger {
    root: PathBuf,
}

impl ExtractionLedger {
    pub fn new(memory_root: impl Into<PathBuf>) -> Self {
        Self {
            root: memory_root.into(),
        }
    }

    pub fn extracted_dir(&self) -> PathBuf {
        self.root.join(EXTRACTED_SUBDIR)
    }

    /// File backing a session's extraction. Its existence *is* the state.
    pub fn extraction_path(&self, thread_id: &ThreadId) -> PathBuf {
        self.extracted_dir().join(format!("{thread_id}.md"))
    }

    pub async fn is_extracted(&self, thread_id: &ThreadId) -> bool {
        tokio::fs::try_exists(self.extraction_path(thread_id))
            .await
            .unwrap_or(false)
    }

    /// Records an extraction, overwriting any previous content for the session.
    pub async fn record(&self, thread_id: &ThreadId, body: &str) -> std::io::Result<PathBuf> {
        let dir = self.extracted_dir();
        tokio::fs::create_dir_all(&dir).await?;
        let path = self.extraction_path(thread_id);
        tokio::fs::write(&path, body).await?;
        Ok(path)
    }
}
