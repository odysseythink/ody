//! Extraction prompt construction.

use std::collections::BTreeSet;

use ody_core::content_items_to_text;
use ody_protocol::models::ContentItem;
use ody_protocol::models::ResponseItem;
use ody_protocol::protocol::RolloutItem;

/// System prompt for a single-session assistant memory extraction.
pub const EXTRACTION_SYSTEM_PROMPT: &str =
    include_str!("../templates/assistant_memory_extraction.md");

/// Upper bound on transcript characters handed to the extraction model, so one
/// very long session cannot blow up the request.
pub const MAX_TRANSCRIPT_CHARS: usize = 60_000;

/// A rendered transcript together with the line indices the model may cite.
///
/// Indices exist so a claim can be traced back to the words in a session: the
/// model cites one, and anything citing an index that is not in this set is
/// treated as invented and dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    pub text: String,
    pub items: BTreeSet<u32>,
    /// Claims the assistant already holds, so a new statement can say which one
    /// it replaces instead of piling up next to it.
    pub known_claims: Vec<KnownClaim>,
}

/// A claim already in the store, as shown to the extraction model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownClaim {
    pub id: String,
    pub subject: String,
    pub statement: String,
}

impl Transcript {
    /// Truncates to the newest part, keeping the citable index set.
    pub fn truncated(&self) -> (Self, bool) {
        let (text, truncated) = truncate_transcript(&self.text);
        (
            Self {
                text,
                items: self.items.clone(),
                known_claims: self.known_claims.clone(),
            },
            truncated,
        )
    }

    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// Renders rollout items into a plain transcript.
///
/// Only real conversation messages survive: developer instructions are dropped
/// (they are harness boilerplate, not user intent) and tool traffic is ignored,
/// because assistant memory cares about what was said, not how it was executed.
pub fn transcript_from_rollout(items: &[RolloutItem]) -> Transcript {
    let mut lines = Vec::new();
    let mut valid = BTreeSet::new();
    let mut index: u32 = 0;

    for item in items {
        // Only real conversation turns matter; session metadata, compactions,
        // turn context and event streams are harness plumbing.
        let RolloutItem::ResponseItem(ResponseItem::Message { role, content, .. }) = item else {
            continue;
        };
        if role == "developer" {
            continue;
        }
        // Harness-injected context (AGENTS.md, environment, permissions, ...)
        // travels as ordinary user messages in the rollout. It is scaffolding,
        // not user intent, so drop those items before rendering.
        let kept: Vec<ContentItem> = content
            .iter()
            .filter(|item| !item_is_contextual_fragment(item))
            .cloned()
            .collect();
        let Some(text) = content_items_to_text(&kept) else {
            continue;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let label = match role.as_str() {
            "user" => "USER",
            "assistant" => "ASSISTANT",
            other => other,
        };
        index += 1;
        valid.insert(index);
        lines.push(format!("[item:{index}] {label}: {text}"));
    }

    Transcript {
        text: lines.join(
            "

",
        ),
        items: valid,
        // Filled in by the pipeline from the store before extraction.
        known_claims: Vec::new(),
    }
}

/// Truncates a transcript to [`MAX_TRANSCRIPT_CHARS`], keeping the newest part.
pub fn truncate_transcript(transcript: &str) -> (String, bool) {
    if transcript.chars().count() <= MAX_TRANSCRIPT_CHARS {
        return (transcript.to_string(), false);
    }
    let skip = transcript.chars().count() - MAX_TRANSCRIPT_CHARS;
    (transcript.chars().skip(skip).collect(), true)
}

/// Builds the user message sent alongside [`EXTRACTION_SYSTEM_PROMPT`].
pub fn build_extraction_input(transcript: &Transcript, truncated: bool) -> String {
    let mut message =
        String::from("Extract durable assistant memory from this odyBox conversation.\n\n");
    if truncated {
        message.push_str(
            "(Note: the transcript was truncated; only the most recent part is shown.)\n\n",
        );
    }
    message.push_str("=== TRANSCRIPT BEGINS ===\n");
    message.push_str(&transcript.text);
    message.push_str("\n=== TRANSCRIPT ENDS ===\n");
    message
}

/// Markers used by the harness when it injects context as a user message.
///
/// Mirrors `ody-core`'s contextual-user-message detection, reimplemented here
/// because that helper is `pub(crate)` and this crate must not modify `ody-core`
/// to reach it. Unknown markers degrade gracefully: the fragment is kept, which
/// is the pre-existing behaviour.
const CONTEXTUAL_FRAGMENT_MARKERS: &[&str] = &[
    "# AGENTS.md instructions",
    "<environment_context>",
    "<permissions instructions>",
    "<workspace_staging>",
    "<prototype_preview>",
    "<user_instructions>",
    "<collaboration_mode>",
];

fn item_is_contextual_fragment(item: &ContentItem) -> bool {
    let text = match item {
        ContentItem::InputText { text } | ContentItem::OutputText { text } => text,
        _ => return false,
    };
    let trimmed = text.trim_start();
    CONTEXTUAL_FRAGMENT_MARKERS
        .iter()
        .any(|marker| trimmed.starts_with(marker))
}
