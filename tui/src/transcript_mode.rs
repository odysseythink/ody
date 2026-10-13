//! Effective transcript ownership for this running TUI, independent of backend feature updates.
//!
//! Terminal restrictions take precedence over the configured ownership preference. The launch-time
//! mode survives session changes so native scrollback never receives retained-only updates.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TranscriptMode {
    Owned,
    Terminal,
}

impl TranscriptMode {
    pub(crate) fn resolve(owned_enabled: bool, alternate_screen_enabled: bool) -> Self {
        if owned_enabled && alternate_screen_enabled {
            Self::Owned
        } else {
            Self::Terminal
        }
    }

    pub(crate) fn is_owned(self) -> bool {
        self == Self::Owned
    }
}

#[cfg(test)]
mod tests {
    use super::TranscriptMode;
    use pretty_assertions::assert_eq;

    #[test]
    fn resolve_requires_both_owned_preference_and_alt_screen() {
        assert_eq!(
            TranscriptMode::resolve(/*owned_enabled*/ true, /*alternate_screen_enabled*/ true),
            TranscriptMode::Owned
        );
        assert_eq!(
            TranscriptMode::resolve(/*owned_enabled*/ true, /*alternate_screen_enabled*/ false),
            TranscriptMode::Terminal
        );
        assert_eq!(
            TranscriptMode::resolve(/*owned_enabled*/ false, /*alternate_screen_enabled*/ true),
            TranscriptMode::Terminal
        );
        assert_eq!(
            TranscriptMode::resolve(/*owned_enabled*/ false, /*alternate_screen_enabled*/ false),
            TranscriptMode::Terminal
        );
    }
}
