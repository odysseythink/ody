//! Contract tests for the isolation between assistant memory and Ody memory.
//!
//! These lock the three guarantees documented in `lib.rs`. They deliberately
//! avoid constructing a full `ody_core::config::Config` so they stay cheap and
//! cannot break because of unrelated config changes.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use ody_extension_api::ExtensionData;
use ody_extension_api::ExtensionDataInit;
use ody_extension_api::ThreadLifecycleContributor;
use ody_extension_api::ThreadResumeInput;
use ody_extension_api::ToolContributor;
use ody_protocol::protocol::InternalSessionSource;
use ody_protocol::protocol::Product;
use ody_protocol::protocol::SessionSource;
use ody_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;

use ody_protocol::ThreadId;
use ody_tools::ToolName;

use crate::ADD_NOTE_TOOL_NAME;
use crate::ASSISTANT_MEMORY_DIR;
use crate::AssistantMemoryConfig;
use crate::AssistantMemoryExtension;
use crate::ExtractError;
use crate::ExtractFuture;
use crate::ExtractedMemory;
use crate::ExtractionLedger;
use crate::MemoryExtractor;
use crate::ODY_MEMORY_DIR;
use crate::READ_TOOL_NAME;
use crate::SEARCH_TOOL_NAME;
use crate::SessionOutcome;
use crate::TOOLS_NAMESPACE;
use crate::assistant_memory_enabled;
use crate::assistant_memory_enabled_for_source;
use crate::assistant_memory_root;
use crate::backend::AssistantMemoryStore;
use crate::ody_memory_root;
use crate::odybox_session_source;
use crate::odybox_sessions;
use crate::parse_extracted_memory;
use crate::process_transcript;
use crate::product_for_source;
use crate::prompts::transcript_from_rollout;

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
    let extension = AssistantMemoryExtension::default();

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

    let extension = AssistantMemoryExtension::default();
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

    let extension = AssistantMemoryExtension::default();
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

// ---------------------------------------------------------------------------
// Write pipeline
// ---------------------------------------------------------------------------

/// Records every transcript it is asked to extract.
struct RecordingExtractor {
    calls: Arc<Mutex<Vec<String>>>,
    result: ExtractedMemory,
}

impl RecordingExtractor {
    fn new(result: ExtractedMemory) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            result,
        }
    }

    fn call_count(&self) -> usize {
        self.calls.lock().expect("calls lock").len()
    }
}

impl MemoryExtractor for RecordingExtractor {
    fn extract<'a>(&'a self, transcript: &'a str) -> ExtractFuture<'a> {
        Box::pin(async move {
            self.calls
                .lock()
                .expect("calls lock")
                .push(transcript.to_string());
            Ok(self.result.clone())
        })
    }
}

/// Always fails, standing in for a model or transport error.
struct FailingExtractor;

impl MemoryExtractor for FailingExtractor {
    fn extract<'a>(&'a self, _transcript: &'a str) -> ExtractFuture<'a> {
        Box::pin(async { Err(ExtractError::Model("boom".to_string())) })
    }
}

/// Contract: the discovery path targets odyBox sessions and is filesystem-only.
#[test]
fn session_source_matches_the_odybox_launch_flag() {
    assert_eq!(
        SessionSource::Custom("odybox".to_string()),
        odybox_session_source()
    );
    assert!(
        assistant_memory_enabled_for_source(&odybox_session_source()),
        "discovery must target exactly the source the product gate accepts"
    );
}

/// Contract: discovery is empty (not an error) when there are no sessions yet.
#[tokio::test]
async fn session_discovery_is_empty_without_a_sessions_dir() {
    let dir = tempfile::tempdir().expect("temp dir");
    let sessions = odybox_sessions(dir.path(), "", 10)
        .await
        .expect("discovery must not fail on a missing sessions dir");
    assert!(sessions.is_empty());
}

/// Contract: a session is extracted once, and its record lands in the ledger.
#[tokio::test]
async fn long_session_is_extracted_and_recorded() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join(ASSISTANT_MEMORY_DIR);
    let ledger = ExtractionLedger::new(&root);
    let thread_id = ThreadId::default();

    let extractor = RecordingExtractor::new(ExtractedMemory {
        summary: "user prefers short answers".to_string(),
        raw_memory: "## User preferences
- Prefers short answers."
            .to_string(),
    });

    let outcome = process_transcript(
        &extractor,
        &ledger,
        &thread_id,
        "USER: please keep it short",
        "2026-09-21T00:00:00+00:00",
    )
    .await;

    assert_eq!(SessionOutcome::Extracted, outcome);
    assert_eq!(1, extractor.call_count());
    assert!(ledger.is_extracted(&thread_id).await);

    let body = std::fs::read_to_string(ledger.extraction_path(&thread_id)).expect("read record");
    assert!(body.contains("user prefers short answers"));
    assert!(body.contains("Prefers short answers."));
    assert!(body.contains(&thread_id.to_string()));
    assert!(body.contains("2026-09-21T00:00:00+00:00"));
}

/// Contract: an empty transcript costs no model call and is left for next time.
#[tokio::test]
async fn empty_transcript_skips_the_model_and_stays_pending() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ledger = ExtractionLedger::new(dir.path().join(ASSISTANT_MEMORY_DIR));
    let thread_id = ThreadId::default();
    let extractor = RecordingExtractor::new(ExtractedMemory {
        summary: "unused".to_string(),
        raw_memory: "unused".to_string(),
    });

    let outcome = process_transcript(
        &extractor, &ledger, &thread_id, "   
  ", "now",
    )
    .await;

    assert_eq!(SessionOutcome::Empty, outcome);
    assert_eq!(
        0,
        extractor.call_count(),
        "no model call for an empty session"
    );
    assert!(
        !ledger.is_extracted(&thread_id).await,
        "a session with nothing said must stay pending, it may still be in progress"
    );
}

/// Contract: when the model finds nothing durable, the session is marked done so
/// it is not re-extracted on every start.
#[tokio::test]
async fn nothing_worth_keeping_is_recorded_and_not_retried() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ledger = ExtractionLedger::new(dir.path().join(ASSISTANT_MEMORY_DIR));
    let thread_id = ThreadId::default();
    let extractor = RecordingExtractor::new(ExtractedMemory {
        summary: String::new(),
        raw_memory: String::new(),
    });

    let outcome = process_transcript(
        &extractor,
        &ledger,
        &thread_id,
        "USER: what time is it",
        "now",
    )
    .await;

    assert_eq!(SessionOutcome::Empty, outcome);
    assert_eq!(1, extractor.call_count());
    assert!(
        ledger.is_extracted(&thread_id).await,
        "an empty verdict must still be recorded, otherwise it is retried forever"
    );
}

/// Contract: a failed extraction is not recorded, so the session retries later.
#[tokio::test]
async fn failed_extraction_is_not_recorded_so_it_retries() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ledger = ExtractionLedger::new(dir.path().join(ASSISTANT_MEMORY_DIR));
    let thread_id = ThreadId::default();

    let outcome = process_transcript(
        &FailingExtractor,
        &ledger,
        &thread_id,
        "USER: something durable",
        "now",
    )
    .await;

    assert_eq!(SessionOutcome::Failed, outcome);
    assert!(
        !ledger.is_extracted(&thread_id).await,
        "a failed session must stay pending so a later run can retry it"
    );
}

/// Manual diagnostic: point at a real `$ODY_HOME` and print what discovery
/// finds. Ignored by default; run with
/// `ODY_HOME_TO_DIAGNOSE=... cargo test -p ody-odybox-memories -- --ignored --nocapture`
#[tokio::test]
#[ignore]
async fn diagnose_discovery_against_a_real_ody_home() {
    let Ok(home) = std::env::var("ODY_HOME_TO_DIAGNOSE") else {
        panic!("set ODY_HOME_TO_DIAGNOSE to the $ODY_HOME to inspect");
    };
    let home = std::path::PathBuf::from(home);

    let sessions_dir = home.join(ody_rollout::SESSIONS_SUBDIR);
    println!("ody_home      = {}", home.display());
    println!(
        "sessions dir  = {} (exists: {})",
        sessions_dir.display(),
        sessions_dir.exists()
    );

    match odybox_sessions(&home, "", 50).await {
        Ok(items) => {
            println!("discovered {} odyBox session(s)", items.len());
            for item in &items {
                println!(
                    "  thread_id={:?} source={:?} cwd={:?}",
                    item.thread_id, item.source, item.cwd
                );
                println!("     path={}", item.path.display());
            }
        }
        Err(err) => println!("discovery failed: {err}"),
    }
}

// ---------------------------------------------------------------------------
// Extraction output parsing
// ---------------------------------------------------------------------------

/// Contract: parsing tolerates the shapes a chat-completions provider returns.
///
/// This is the failure that broke the first end-to-end run: `output_schema` is
/// a Responses-API feature, so a `wire_api = "chat"` provider ignores it and
/// answers with fenced or prose-wrapped JSON.
#[test]
fn extraction_parsing_tolerates_wrapped_json() {
    let bare = r#"{"summary":"s","raw_memory":"m"}"#;

    let cases = [
        ("bare", bare.to_string()),
        (
            "fenced_json",
            format!(
                "```json
{bare}
```"
            ),
        ),
        (
            "fenced_bare",
            format!(
                "```
{bare}
```"
            ),
        ),
        (
            "prose_around",
            format!(
                "Here is the memory:
{bare}
Hope that helps."
            ),
        ),
        (
            "padded",
            format!(
                "

  {bare}  

"
            ),
        ),
    ];

    for (label, raw) in cases {
        let parsed = parse_extracted_memory(&raw)
            .unwrap_or_else(|err| panic!("case '{label}' should parse but failed: {err}"));
        assert_eq!("s", parsed.summary, "case '{label}'");
        assert_eq!("m", parsed.raw_memory, "case '{label}'");
    }

    let err = parse_extracted_memory("I could not find anything worth remembering.")
        .expect_err("prose with no JSON must be a parse error");
    assert!(
        matches!(err, ExtractError::Parse(_)),
        "expected a Parse error, got {err:?}"
    );
}

/// Contract: a verdict of "nothing worth keeping" parses cleanly.
#[test]
fn extraction_parsing_accepts_the_empty_verdict() {
    let parsed = parse_extracted_memory(r#"{"summary":"","raw_memory":""}"#)
        .expect("the empty verdict must parse");
    assert!(parsed.is_empty());
}

// ---------------------------------------------------------------------------
// Transcript rendering
// ---------------------------------------------------------------------------

fn message(role: &str, text: &str) -> ody_protocol::protocol::RolloutItem {
    ody_protocol::protocol::RolloutItem::ResponseItem(ody_protocol::models::ResponseItem::Message {
        id: None,
        role: role.to_string(),
        content: vec![ody_protocol::models::ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    })
}

/// Contract: harness-injected context must not be mistaken for user intent.
///
/// The first end-to-end run showed `# AGENTS.md instructions for <cwd>` sitting
/// at the top of the transcript, which would have had the extraction model
/// reading project conventions as if the user had asked for them.
#[test]
fn harness_injected_fragments_are_excluded_from_the_transcript() {
    let items = vec![
        message(
            "user",
            "# AGENTS.md instructions for D:/workspace/demo

<INSTRUCTIONS>
repo conventions
</INSTRUCTIONS>",
        ),
        message(
            "user",
            "<environment_context>
<cwd>D:/demo</cwd>
</environment_context>",
        ),
        message(
            "user",
            "<permissions instructions>
sandbox_mode is danger-full-access
</permissions instructions>",
        ),
        message("user", "帮我把这个接口的鉴权重构一下"),
        message("assistant", "好的,我先看一下现有的鉴权代码。"),
        message("developer", "you are a coding agent"),
    ];

    let transcript = transcript_from_rollout(&items);

    assert!(
        transcript.contains("帮我把这个接口的鉴权重构一下"),
        "real user intent must survive: {transcript}"
    );
    assert!(
        transcript.contains("ASSISTANT: 好的"),
        "assistant text must survive"
    );
    for marker in [
        "AGENTS.md instructions",
        "environment_context",
        "permissions instructions",
        "coding agent",
    ] {
        assert!(
            !transcript.contains(marker),
            "harness fragment '{marker}' leaked into the transcript: {transcript}"
        );
    }
}

/// Contract: a session with only injected context renders as empty, so the
/// pipeline skips it instead of spending a model call.
#[test]
fn transcript_of_only_injected_context_is_empty() {
    let items = vec![
        message(
            "user",
            "# AGENTS.md instructions for D:/demo

<INSTRUCTIONS>
x
</INSTRUCTIONS>",
        ),
        message("developer", "harness boilerplate"),
    ];
    assert!(transcript_from_rollout(&items).trim().is_empty());
}

/// Contract: resuming a thread that never captured pipeline context is a no-op.
///
/// `ThreadResumeInput` carries neither `Config` nor `SessionSource`, so resume
/// can only act on state captured at thread start in this process. After a
/// restart the store is empty and resume must do nothing rather than guess —
/// the next thread start still scans every unprocessed session.
#[tokio::test]
async fn resume_without_captured_context_is_a_noop() {
    let extension = AssistantMemoryExtension::default();
    let session_store = ExtensionData::new("session");
    let thread_store = ExtensionData::new("thread");

    extension
        .on_thread_resume(ThreadResumeInput {
            session_store: &session_store,
            thread_store: &thread_store,
        })
        .await;

    assert_eq!(
        None,
        thread_store.get::<AssistantMemoryConfig>(),
        "resume must not fabricate gate state it never captured"
    );
}

/// Contract: resuming an ungated (non-odyBox) thread stays a no-op even when the
/// store already holds unrelated state.
#[tokio::test]
async fn resume_with_unrelated_state_is_a_noop() {
    let extension = AssistantMemoryExtension::default();
    let session_store = ExtensionData::new("session");
    let thread_store = ExtensionData::new("thread");
    thread_store.insert(String::from("unrelated"));

    extension
        .on_thread_resume(ThreadResumeInput {
            session_store: &session_store,
            thread_store: &thread_store,
        })
        .await;

    assert_eq!(None, thread_store.get::<AssistantMemoryConfig>());
}
