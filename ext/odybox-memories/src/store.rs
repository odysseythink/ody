//! Storage roots for assistant memory.
//!
//! Assistant memory deliberately lives beside — never inside — the Ody memory
//! workspace, so `ody-memories-write`'s git baseline, artifact sync and pruning
//! never see assistant-memory files and vice versa.

use std::path::Path;
use std::path::PathBuf;

use ody_utils_absolute_path::AbsolutePathBuf;

/// Directory name for odyBox assistant memory, relative to `$ODY_HOME`.
pub const ASSISTANT_MEMORY_DIR: &str = "odybox_memories";

/// Directory name for the Ody memory workspace, relative to `$ODY_HOME`.
///
/// Owned by `ody-memories-write`; mirrored here only so the separation can be
/// asserted in tests.
pub const ODY_MEMORY_DIR: &str = "memories";

/// Root directory for odyBox assistant memory artifacts.
pub fn assistant_memory_root(ody_home: &AbsolutePathBuf) -> PathBuf {
    ody_home.join(ASSISTANT_MEMORY_DIR).to_path_buf()
}

/// Directory holding notes written through the `add_note` tool.
///
/// Notes live beside the store rather than inside it: the user can read and edit
/// them, and the extraction pass turns them into claims that recall can find.
pub fn ad_hoc_notes_dir(memory_root: &Path) -> PathBuf {
    memory_root.join("extensions").join("ad_hoc").join("notes")
}

/// Root directory of the Ody memory workspace (not owned by this crate).
pub fn ody_memory_root(ody_home: &AbsolutePathBuf) -> PathBuf {
    ody_home.join(ODY_MEMORY_DIR).to_path_buf()
}
