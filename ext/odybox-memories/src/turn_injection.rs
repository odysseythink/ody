//! Turn-scoped memory injection.
//!
//! Memory reaches the model once per user turn rather than once per thread. A
//! standing block cannot answer "is any of this relevant to what I was just
//! asked", and memory that only arrives at thread start cannot reflect anything
//! learned since — which is most of a long conversation.
//!
//! Everything here is best effort: a missing store, a broken query, or an empty
//! recall must never cost the user their turn.

use std::path::Path;

use ody_extension_api::ContextualUserFragment;
use ody_protocol::user_input::UserInput;

use crate::MemoryDb;
use crate::budget_tokens_for_window;
use crate::recall::recall_for_turn;

/// Longest query drawn from one user message, in characters.
pub const MAX_QUERY_CHARS: usize = 400;

/// Fragment markers. They let a later read of the transcript tell injected memory
/// from the user's own words.
const START_MARKER: &str = "<assistant_memory>";
const END_MARKER: &str = "</assistant_memory>";

/// The recalled block, carried as a contextual user fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantMemoryFragment {
    block: String,
}

impl AssistantMemoryFragment {
    pub fn new(block: impl Into<String>) -> Self {
        Self {
            block: block.into(),
        }
    }

    /// The model-visible text, without the fragment markers.
    pub fn block(&self) -> &str {
        &self.block
    }
}

impl ContextualUserFragment for AssistantMemoryFragment {
    fn role(&self) -> &'static str {
        "user"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (START_MARKER, END_MARKER)
    }

    fn body(&self) -> String {
        self.block.clone()
    }
}

/// Builds the recall query from the user's own words.
///
/// Only text parts carry terms — an image or a mention has none. A long message
/// keeps both ends, because the request is as likely to follow a paste as to open
/// with it.
pub fn query_from_user_input(user_input: &[UserInput]) -> Option<String> {
    let mut text = String::new();
    for item in user_input {
        if let UserInput::Text { text: part, .. } = item {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(part);
        }
    }
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(truncate_for_query(text))
}

fn truncate_for_query(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= MAX_QUERY_CHARS {
        return text.to_string();
    }
    let half = MAX_QUERY_CHARS / 2;
    let head = chars[..half].iter().collect::<String>();
    let tail = chars[chars.len() - half..].iter().collect::<String>();
    format!("{head} {tail}")
}

/// Recalls memory relevant to this turn, or `None` when there is none.
pub async fn turn_memory_fragment(
    memory_root: &Path,
    user_input: &[UserInput],
    model_context_window: Option<i64>,
) -> Option<AssistantMemoryFragment> {
    let query = query_from_user_input(user_input)?;
    let db = match MemoryDb::open(memory_root).await {
        Ok(db) => db,
        Err(err) => {
            tracing::warn!(error = %err, "assistant memory: could not open the store for recall");
            return None;
        }
    };

    let budget = budget_tokens_for_window(model_context_window);
    // No attempt is made to subtract the standing thread-level block from this
    // one. Excluding whatever that block would select today would also exclude a
    // claim learned five minutes ago, which is precisely the memory this path
    // exists to deliver. A claim that appears in both blocks costs a line of
    // prompt; a memory that never appears at all costs correctness.
    match recall_for_turn(&db, &query, budget).await {
        Ok(outcome) if !outcome.is_empty() => Some(AssistantMemoryFragment::new(outcome.rendered)),
        Ok(_) => None,
        Err(err) => {
            tracing::warn!(error = %err, "assistant memory: recall failed for this turn");
            None
        }
    }
}

#[cfg(test)]
#[path = "turn_injection_tests.rs"]
mod tests;
