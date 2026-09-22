//! Contract tests for the isolation between assistant memory and Ody memory.
//!
//! These lock the three guarantees documented in `lib.rs`. They deliberately
//! avoid constructing a full `ody_core::config::Config` so they stay cheap and
//! cannot break because of unrelated config changes.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use ody_extension_api::ContextContributor;
use ody_extension_api::ExtensionData;
use ody_extension_api::ExtensionDataInit;
use ody_extension_api::PromptSlot;
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
use crate::ClaimDraft;
use crate::ClaimKind;
use crate::Confidence;
use crate::ConservativeJudge;
use crate::EvidenceDraft;
use crate::EvidenceKind;
use crate::ExtractError;
use crate::ExtractFuture;
use crate::ExtractionLedger;
use crate::ExtractionResult;
use crate::MemoryDb;
use crate::MemoryExtractor;
use crate::NewClaim;
use crate::NewEvidence;
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
use crate::opted_in_with;
use crate::process_transcript;
use crate::product_for_source;
use crate::prompts::Transcript;
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
        opted_in: true,
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
    result: ExtractionResult,
}

impl RecordingExtractor {
    fn new(result: ExtractionResult) -> Self {
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
    fn extract<'a>(&'a self, transcript: &'a Transcript) -> ExtractFuture<'a> {
        Box::pin(async move {
            self.calls
                .lock()
                .expect("calls lock")
                .push(transcript.text.clone());
            Ok(self.result.clone())
        })
    }
}

/// Always fails, standing in for a model or transport error.
struct FailingExtractor;

impl MemoryExtractor for FailingExtractor {
    fn extract<'a>(&'a self, _transcript: &'a Transcript) -> ExtractFuture<'a> {
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
    let db = MemoryDb::open_in_memory().await.expect("store");
    let thread_id = ThreadId::default();

    let extractor = RecordingExtractor::new(ExtractionResult {
        summary: "user prefers short answers".to_string(),
        claims: vec![ClaimDraft {
            kind: "preference".to_string(),
            subject: "user".to_string(),
            statement: "Prefers short answers.".to_string(),
            confidence: "high".to_string(),
            scope: None,
            decision_implication: None,
            review_in_days: None,
            supersedes: None,
            evidence: vec![EvidenceDraft {
                item: 1,
                quote: "please keep it short".to_string(),
            }],
        }],
    });

    let outcome = process_transcript(
        &extractor,
        &ConservativeJudge,
        &db,
        &ledger,
        &thread_id,
        &Transcript {
            text: "[item:1] USER: please keep it short".to_string(),
            items: BTreeSet::from([1]),
            known_claims: Vec::new(),
        },
        "2026-09-21T00:00:00+00:00",
    )
    .await;

    assert_eq!(SessionOutcome::Extracted, outcome.outcome);
    assert_eq!(1, extractor.call_count());
    assert!(ledger.is_extracted(&thread_id).await);

    let body = std::fs::read_to_string(ledger.extraction_path(&thread_id)).expect("read record");
    assert!(body.contains("user prefers short answers"));
    assert!(body.contains("Prefers short answers."));
    assert!(body.contains(&thread_id.to_string()));
    assert!(body.contains("2026-09-21T00:00:00+00:00"));

    // The record is the human-readable trace; the store is what the assistant
    // will actually be handed next time.
    let stored = db.active_claims(10).await.expect("claims");
    assert_eq!(
        1,
        stored.len(),
        "the extracted claim has to reach the store, not only the record"
    );
    assert_eq!("Prefers short answers.", stored[0].statement);
}

/// Contract: an empty transcript costs no model call and is left for next time.
#[tokio::test]
async fn empty_transcript_skips_the_model_and_stays_pending() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ledger = ExtractionLedger::new(dir.path().join(ASSISTANT_MEMORY_DIR));
    let db = MemoryDb::open_in_memory().await.expect("store");
    let thread_id = ThreadId::default();
    let extractor = RecordingExtractor::new(ExtractionResult {
        summary: "unused".to_string(),
        claims: Vec::new(),
    });

    let outcome = process_transcript(
        &extractor,
        &ConservativeJudge,
        &db,
        &ledger,
        &thread_id,
        &Transcript {
            text: "   ".to_string(),
            items: BTreeSet::new(),
            known_claims: Vec::new(),
        },
        "now",
    )
    .await;

    assert_eq!(SessionOutcome::Empty, outcome.outcome);
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
    let db = MemoryDb::open_in_memory().await.expect("store");
    let thread_id = ThreadId::default();
    let extractor = RecordingExtractor::new(ExtractionResult {
        summary: String::new(),
        claims: Vec::new(),
    });

    let outcome = process_transcript(
        &extractor,
        &ConservativeJudge,
        &db,
        &ledger,
        &thread_id,
        &Transcript {
            text: "[item:1] USER: what time is it".to_string(),
            items: BTreeSet::from([1]),
            known_claims: Vec::new(),
        },
        "now",
    )
    .await;

    assert_eq!(SessionOutcome::Empty, outcome.outcome);
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
    let db = MemoryDb::open_in_memory().await.expect("store");
    let thread_id = ThreadId::default();

    let outcome = process_transcript(
        &FailingExtractor,
        &ConservativeJudge,
        &db,
        &ledger,
        &thread_id,
        &Transcript {
            text: "[item:1] USER: something durable".to_string(),
            items: BTreeSet::from([1]),
            known_claims: Vec::new(),
        },
        "now",
    )
    .await;

    assert_eq!(SessionOutcome::Failed, outcome.outcome);
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

    let transcript = transcript_from_rollout(&items).text;

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
    assert!(transcript_from_rollout(&items).text.trim().is_empty());
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

/// Contract 2d: a closed gate injects nothing, so an Ody thread never sees
/// assistant-memory text.
#[tokio::test]
async fn closed_gate_injects_no_memory_block() {
    let session_store = ExtensionData::new("session");
    let thread_store = ExtensionData::new_with_init("thread", ExtensionDataInit::new());

    let extension = AssistantMemoryExtension::default();
    let fragments = extension
        .contribute_thread_context(&session_store, &thread_store)
        .await;

    assert_eq!(fragments.len(), 0);
}

/// Contract 2e: an open gate injects what the user kept, as developer policy.
#[tokio::test]
async fn open_gate_injects_stored_claims() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join(ASSISTANT_MEMORY_DIR);
    let db = MemoryDb::open(&root).await.expect("memory store");
    db.insert_claim(
        NewClaim {
            kind: ClaimKind::Preference,
            subject: "user".to_string(),
            statement: "user prefers short replies".to_string(),
            confidence: Confidence::High,
            scope: None,
            decision_implication: None,
            review_due: None,
            valid_from: None,
        },
        vec![NewEvidence {
            kind: EvidenceKind::Fact,
            occurred_at: chrono::Utc::now(),
            source_thread: "t-1".to_string(),
            source_locator: "item:1".to_string(),
            excerpt: "原话".to_string(),
        }],
    )
    .await
    .expect("claim");

    let session_store = ExtensionData::new("session");
    let thread_store = gated_thread_store(root);
    let extension = AssistantMemoryExtension::default();

    let fragments = extension
        .contribute_thread_context(&session_store, &thread_store)
        .await;

    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].slot(), PromptSlot::DeveloperPolicy);
    assert!(fragments[0].text().contains("user prefers short replies"));
}

/// Contract: memory is opt-in. Anything but an explicit yes stays off, so a
/// missing flag, a typo, or a stale config can never start collecting.
#[test]
fn opt_in_requires_an_explicit_yes() {
    assert!(!opted_in_with(None));
    assert!(!opted_in_with(Some("")));
    assert!(!opted_in_with(Some("true")));
    assert!(!opted_in_with(Some("0")));
    assert!(opted_in_with(Some("1")));
}

/// Contract: a thread that is an odyBox thread but has not opted in injects
/// nothing, even when claims exist on disk.
#[tokio::test]
async fn opted_out_thread_injects_no_memory_block() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join(ASSISTANT_MEMORY_DIR);
    let db = MemoryDb::open(&root).await.expect("store");
    db.insert_claim(
        NewClaim {
            kind: ClaimKind::Preference,
            subject: "user".to_string(),
            statement: "user prefers short replies".to_string(),
            confidence: Confidence::High,
            scope: None,
            decision_implication: None,
            review_due: None,
            valid_from: None,
        },
        vec![NewEvidence {
            kind: EvidenceKind::Fact,
            occurred_at: chrono::Utc::now(),
            source_thread: "t-1".to_string(),
            source_locator: "item:1".to_string(),
            excerpt: "原话".to_string(),
        }],
    )
    .await
    .expect("claim");

    let thread_store = ExtensionData::new_with_init("thread", ExtensionDataInit::new());
    thread_store.insert(AssistantMemoryConfig {
        product: Product::OdyBox,
        memory_root: root,
        opted_in: false,
    });
    let session_store = ExtensionData::new("session");
    let extension = AssistantMemoryExtension::default();

    let fragments = extension
        .contribute_thread_context(&session_store, &thread_store)
        .await;

    assert_eq!(0, fragments.len());
}
