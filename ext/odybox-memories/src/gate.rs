//! Product gate for assistant memory.
//!
//! The gate is derived purely from the session source, so it needs no `Config`
//! and cannot drift with user configuration reloads.

use ody_protocol::protocol::Product;
use ody_protocol::protocol::SessionSource;

/// Resolves the owning product for a session source, if it maps to a known one.
///
/// Thin wrapper over [`SessionSource::restriction_product`] so this crate has a
/// single, testable seam and does not depend on that method directly.
pub fn product_for_source(source: &SessionSource) -> Option<Product> {
    source.restriction_product()
}

/// Whether assistant memory is enabled for `product`.
///
/// Only [`Product::OdyBox`] opts in. `Some(Product::Ody)` (plain `ody`,
/// `vscode`, `exec`, `mcp`, unknown) and `None` (internal / sub-agent threads)
/// both stay off, which is what keeps the existing Ody memory path untouched.
pub fn assistant_memory_enabled(product: Option<Product>) -> bool {
    matches!(product, Some(Product::OdyBox))
}

/// Environment flag odyBox sets once the user has switched memory on.
pub const ENABLED_ENV: &str = "ODYBOX_ASSISTANT_MEMORY";

/// Whether the user opted in. Memory is opt-in: anything but an explicit yes is
/// a no, so a missing flag, a typo, or a stale process keeps memory off.
pub fn opted_in() -> bool {
    opted_in_with(std::env::var(ENABLED_ENV).ok().as_deref())
}

/// The decision on its own, so it can be tested without touching the process
/// environment.
pub fn opted_in_with(value: Option<&str>) -> bool {
    value == Some("1")
}

/// Convenience wrapper: decide straight from a session source.
pub fn assistant_memory_enabled_for_source(source: &SessionSource) -> bool {
    assistant_memory_enabled(product_for_source(source))
}
