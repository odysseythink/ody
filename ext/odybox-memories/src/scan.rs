//! Discovery of odyBox sessions eligible for assistant memory extraction.
//!
//! Uses the filesystem-based rollout listing (`ody_rollout::get_threads`),
//! which filters by `SessionSource` **by value** rather than by a stored
//! string. That keeps this path clear of the serde/Display string mismatch the
//! SQL-based Ody claim path has to work around.

use std::path::Path;

use ody_protocol::protocol::Product;
use ody_protocol::protocol::SessionSource;
use ody_rollout::ThreadItem;
use ody_rollout::ThreadSortKey;

/// The session source odyBox launches its runtime with
/// (`--session-source odybox`).
pub fn odybox_session_source() -> SessionSource {
    SessionSource::Custom(Product::OdyBox.to_app_platform().to_string())
}

/// Lists odyBox sessions newest-first, capped at `limit`.
///
/// Returns an empty list when the sessions directory does not exist yet.
pub async fn odybox_sessions(
    ody_home: &Path,
    default_provider: &str,
    limit: usize,
) -> std::io::Result<Vec<ThreadItem>> {
    let sources = [odybox_session_source()];
    let page = ody_rollout::get_threads(
        ody_home,
        limit.max(1),
        None,
        ThreadSortKey::RecencyAt,
        &sources,
        None,
        None,
        default_provider,
    )
    .await?;
    Ok(page.items)
}
