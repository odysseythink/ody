use std::collections::HashSet;

const FALLBACK_MODEL_METADATA_WARNING_PREFIX: &str = "Model metadata for `";
const FALLBACK_MODEL_METADATA_WARNING_TAIL: &str = " not found.";

#[derive(Default)]
pub(super) struct WarningDisplayState {
    fallback_model_metadata_slugs: HashSet<String>,
}

impl WarningDisplayState {
    pub(super) fn should_display(&mut self, message: &str) -> bool {
        fallback_model_metadata_warning_slug(message)
            .is_none_or(|slug| self.fallback_model_metadata_slugs.insert(slug.to_string()))
    }
}

/// Extract the slug from a fallback-metadata warning. The message suffix may
/// grow extra guidance over time, so only the stable head is matched:
/// `Model metadata for `<slug>` not found.…`.
fn fallback_model_metadata_warning_slug(message: &str) -> Option<&str> {
    let rest = message.strip_prefix(FALLBACK_MODEL_METADATA_WARNING_PREFIX)?;
    let (slug, tail) = rest.split_once('`')?;
    tail.starts_with(FALLBACK_MODEL_METADATA_WARNING_TAIL)
        .then_some(slug)
}
