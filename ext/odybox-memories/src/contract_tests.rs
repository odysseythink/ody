//! Contract tests for the isolation between assistant memory and Ody memory.
//!
//! These lock the three guarantees documented in `lib.rs`. They deliberately
//! avoid constructing a full `ody_core::config::Config` so they stay cheap and
//! cannot break because of unrelated config changes.

use std::path::PathBuf;

use ody_extension_api::ExtensionData;
use ody_extension_api::ExtensionDataInit;
use ody_extension_api::ToolContributor;
use ody_protocol::protocol::InternalSessionSource;
use ody_protocol::protocol::Product;
use ody_protocol::protocol::SessionSource;
use ody_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;

use ody_tools::ToolName;

use crate::ADD_NOTE_TOOL_NAME;
use crate::ASSISTANT_MEMORY_DIR;
use crate::AssistantMemoryConfig;
use crate::AssistantMemoryExtension;
use crate::ODY_MEMORY_DIR;
use crate::READ_TOOL_NAME;
use crate::SEARCH_TOOL_NAME;
use crate::TOOLS_NAMESPACE;
use crate::assistant_memory_enabled;
use crate::assistant_memory_enabled_for_source;
use crate::assistant_memory_root;
use crate::backend::AssistantMemoryStore;
use crate::ody_memory_root;
use crate::product_for_source;

/// Session sources that must keep the existing Ody memory behaviour, i.e. must
/// never see assistant memory.
fn ody_owned_sources() -> Vec<SessionSource> {
    vec![
        SessionSource::Cli,
        SessionSource::VSCode,
        SessionSource::Exec,
        SessionSource::Mcp,
        SessionSource::Unknown,
        SessionSource::Custom("atlas".to_string()),
        SessionSource::Custom("some-other-host".to_string()),
        SessionSource::Internal(InternalSessionSource::MemoryConsolidation),
    ]
}

fn test_ody_home() -> AbsolutePathBuf {
    let path: PathBuf = std::env::temp_dir().join("odybox-memories-contract-test");
    AbsolutePathBuf::try_from(path).expect("temp dir should be an absolute path")
}

/// Contract 1: only `Product::OdyBox` opens the gate.
#[test]
fn only_odybox_product_opens_the_gate() {
    let odybox = SessionSource::Custom("odybox".to_string());
    assert_eq!(
        Some(Product::OdyBox),
        product_for_source(&odybox),
        "the odyBox session source must resolve to Product::OdyBox"
    );
    assert!(
        assistant_memory_enabled_for_source(&odybox),
        "odyBox sessions must enable assistant memory"
    );

    for source in ody_owned_sources() {
        assert!(
            !assistant_memory_enabled_for_source(&source),
            "source {source:?} must not enable assistant memory"
        );
    }

    assert!(
        !assistant_memory_enabled(None),
        "sources with no product mapping must not enable assistant memory"
    );
    assert!(
        !assistant_memory_enabled(Some(Product::Ody)),
        "Product::Ody must not enable assistant memory"
    );
    assert!(
        !assistant_memory_enabled(Some(Product::Atlas)),
        "Product::Atlas must not enable assistant memory"
    );
    assert!(assistant_memory_enabled(Some(Product::OdyBox)));
}

/// Contract 2a: a thread with no gate state exposes no tools.
#[test]
fn closed_gate_exposes_no_tools() {
    let session_store = ExtensionData::new("session");
    let thread_store = ExtensionData::new("thread");
    let extension = AssistantMemoryExtension;

    assert_eq!(
        Vec::<String>::new(),
        extension
            .tools(&session_store, &thread_store)
            .iter()
            .map(|tool| tool.tool_name().to_string())
            .collect::<Vec<_>>(),
        "an ungated thread must expose no assistant memory tools"
    );
}

/// Contract 2b: unrelated thread-store state must not be mistaken for the gate.
#[test]
fn unrelated_thread_state_does_not_open_the_gate() {
    let session_store = ExtensionData::new("session");
    let thread_store = ExtensionData::new("thread");
    thread_store.insert(String::from("not an assistant memory gate"));

    let extension = AssistantMemoryExtension;
    assert!(
        extension.tools(&session_store, &thread_store).is_empty(),
        "only AssistantMemoryConfig may open the gate"
    );
}

/// Contract 3: assistant memory has its own root, disjoint from Ody's.
#[test]
fn assistant_memory_root_never_collides_with_ody_memory_root() {
    let ody_home = test_ody_home();
    let assistant_root = assistant_memory_root(&ody_home);
    let ody_root = ody_memory_root(&ody_home);

    assert_ne!(
        assistant_root, ody_root,
        "assistant memory must not share the Ody memory workspace root"
    );
    assert!(
        !assistant_root.starts_with(&ody_root),
        "assistant memory root {} must not live inside the Ody memory workspace {}",
        assistant_root.display(),
        ody_root.display()
    );
    assert!(
        !ody_root.starts_with(&assistant_root),
        "the Ody memory workspace {} must not live inside the assistant memory root {}",
        ody_root.display(),
        assistant_root.display()
    );

    assert_eq!(
        ody_home.join(ASSISTANT_MEMORY_DIR).to_path_buf(),
        assistant_root
    );
    assert_eq!(ody_home.join(ODY_MEMORY_DIR).to_path_buf(), ody_root);
}

/// Builds a thread store whose gate is open, pointed at `memory_root`.
fn gated_thread_store(memory_root: PathBuf) -> ExtensionData {
    let thread_store = ExtensionData::new_with_init("thread", ExtensionDataInit::new());
    thread_store.insert(AssistantMemoryConfig {
        product: Product::OdyBox,
        memory_root,
    });
    thread_store
}

/// Contract 2c: an open gate exposes the assistant tool set, in its own namespace.
#[test]
fn open_gate_exposes_the_assistant_tool_set() {
    let session_store = ExtensionData::new("session");
    let thread_store =
        gated_thread_store(std::env::temp_dir().join("odybox-assistant-memory-tools"));

    let extension = AssistantMemoryExtension;
    let tools = extension.tools(&session_store, &thread_store);
    let mut actual: Vec<ToolName> = tools.iter().map(|tool| tool.tool_name()).collect();
    actual.sort();
    let mut expected = vec![
        ToolName::namespaced(TOOLS_NAMESPACE, ADD_NOTE_TOOL_NAME),
        ToolName::namespaced(TOOLS_NAMESPACE, READ_TOOL_NAME),
        ToolName::namespaced(TOOLS_NAMESPACE, SEARCH_TOOL_NAME),
    ];
    expected.sort();
    assert_eq!(expected, actual);

    // Namespace separation: every tool lives in the assistant namespace, never
    // in the Ody memory namespace (`memories`).
    for tool in &tools {
        assert_eq!(Some(TOOLS_NAMESPACE), tool.tool_name().namespace.as_deref());
    }
    assert_ne!("memories", TOOLS_NAMESPACE);
}

/// Contract 3 (runtime): the tools operate inside the assistant root, and a
/// path-escape attempt is rejected rather than followed.
#[tokio::test]
async fn assistant_tools_stay_inside_the_assistant_root() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join(ASSISTANT_MEMORY_DIR);
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("MEMORY.md"), "# Title\nalpha preference\nbeta\n").expect("seed");

    // A file outside the root, reachable only by escaping it.
    std::fs::write(dir.path().join("outside.md"), "secret outside content\n")
        .expect("seed outside");

    let store = AssistantMemoryStore::new(&root);

    let read = store
        .read(Some("MEMORY.md"), 1, None, 10_000)
        .await
        .expect("read inside root");
    assert!(read.content.contains("alpha preference"));
    assert_eq!("MEMORY.md", read.path);

    for escape in ["../outside.md", "../../outside.md", "/etc/passwd"] {
        let result = store.read(Some(escape), 1, None, 10_000).await;
        assert!(result.is_err(), "escape '{escape}' must be rejected");
    }

    let found = store
        .search(&["alpha".to_string()], None, 0, false, 10)
        .await
        .expect("search");
    assert_eq!(1, found.matches.len());
    assert!(found.matches[0].content.contains("alpha preference"));
}

/// Contract 3 (runtime): notes land under the assistant root's ad-hoc folder.
#[tokio::test]
async fn assistant_notes_are_written_under_the_assistant_root() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join(ASSISTANT_MEMORY_DIR);
    let store = AssistantMemoryStore::new(&root);

    let filename = "2026-09-21T10-00-00-prefers-short-answers.md";
    store
        .add_note(filename, "Prefer shorter answers.")
        .await
        .expect("write note");

    let written = root
        .join("extensions")
        .join("ad_hoc")
        .join("notes")
        .join(filename);
    assert!(
        written.is_file(),
        "note must be written under the assistant root"
    );

    // The same note cannot be written twice.
    assert!(
        store.add_note(filename, "again").await.is_err(),
        "duplicate note filenames must be rejected"
    );

    // Path separators and bad timestamps are rejected outright.
    for bad in [
        "../escape.md",
        "not-a-timestamp.md",
        "2026-09-21T10-00-00-Bad_Slug.md",
    ] {
        assert!(
            store.add_note(bad, "x").await.is_err(),
            "'{bad}' must be rejected"
        );
    }
}
