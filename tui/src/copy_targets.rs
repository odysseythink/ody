//! Semantic `/copy` targets over committed transcript cells.
//!
//! Each target carries its original Markdown payload (whole response, user
//! message, fenced code block, or blockquote), so copying never depends on
//! terminal width, rendered bullet prefixes, or re-wrapped text.

use std::sync::Arc;

use crate::history_cell::AgentMarkdownCell;
use crate::history_cell::HistoryCell;
use crate::history_cell::ProposedPlanCell;
use crate::history_cell::UserHistoryCell;
use crate::markdown::CopyTarget as MarkdownCopyTarget;
use crate::markdown::extract_copy_targets;

/// The semantic kind of a `/copy` navigation target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CopyTargetKind {
    WholeResponse,
    UserMessage,
    CodeBlock,
    Blockquote,
}

impl CopyTargetKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::WholeResponse => "Whole response",
            Self::UserMessage => "User message",
            Self::CodeBlock => "Code block",
            Self::Blockquote => "Blockquote",
        }
    }
}

/// One navigable copy target: which cell it belongs to and what gets copied.
#[derive(Clone, Debug)]
pub(crate) struct CopyTarget {
    pub(crate) cell_index: usize,
    pub(crate) kind: CopyTargetKind,
    pub(crate) source: String,
}

/// Flatten every navigable copy target across the transcript, in display
/// order (older first). Cells without an original-text payload contribute
/// nothing.
pub(crate) fn copy_targets(cells: &[Arc<dyn HistoryCell>]) -> Vec<CopyTarget> {
    cells
        .iter()
        .enumerate()
        .flat_map(|(cell_index, cell)| cell_targets(cell, cell_index))
        .collect()
}

fn cell_targets(cell: &Arc<dyn HistoryCell>, cell_index: usize) -> Vec<CopyTarget> {
    let any = cell.as_any();
    let (whole_kind, source) = if let Some(agent) = any.downcast_ref::<AgentMarkdownCell>() {
        (CopyTargetKind::WholeResponse, agent.markdown_source())
    } else if let Some(plan) = any.downcast_ref::<ProposedPlanCell>() {
        (CopyTargetKind::WholeResponse, plan.plan_markdown())
    } else if let Some(user) = any.downcast_ref::<UserHistoryCell>() {
        (CopyTargetKind::UserMessage, user.message.as_str())
    } else {
        return Vec::new();
    };
    let mut targets = vec![CopyTarget {
        cell_index,
        kind: whole_kind,
        source: source.to_string(),
    }];
    targets.extend(
        extract_copy_targets(source)
            .into_iter()
            .map(|target| {
                let (kind, content) = match target {
                    MarkdownCopyTarget::Code { content, .. } => {
                        (CopyTargetKind::CodeBlock, content)
                    }
                    MarkdownCopyTarget::Quote(content) => (CopyTargetKind::Blockquote, content),
                };
                CopyTarget {
                    cell_index,
                    kind,
                    source: content.to_string(),
                }
            }),
    );
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::text::Line;
    use std::path::Path;

    fn agent(source: &str) -> Arc<dyn HistoryCell> {
        Arc::new(AgentMarkdownCell::new(
            source.to_string(),
            Path::new("/tmp"),
        ))
    }

    fn user(message: &str) -> Arc<dyn HistoryCell> {
        Arc::new(UserHistoryCell {
            message: message.to_string(),
            text_elements: Vec::new(),
            local_image_paths: Vec::new(),
            remote_image_urls: Vec::new(),
        })
    }

    #[derive(Debug)]
    struct OtherCell;

    impl HistoryCell for OtherCell {
        fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
            Vec::new()
        }

        fn raw_lines(&self) -> Vec<Line<'static>> {
            Vec::new()
        }

        fn transcript_lines(&self, _width: u16) -> Vec<Line<'static>> {
            Vec::new()
        }
    }

    fn kinds(targets: &[CopyTarget]) -> Vec<CopyTargetKind> {
        targets.iter().map(|target| target.kind).collect()
    }

    #[test]
    fn agent_cell_yields_whole_response_code_and_quote_targets() {
        let source = "# Title\n\nsome prose\n\n```rust\nfn main() {}\n```\n\n> quoted line\n";
        let targets = copy_targets(&[agent(source)]);

        assert_eq!(
            kinds(&targets),
            vec![
                CopyTargetKind::WholeResponse,
                CopyTargetKind::CodeBlock,
                CopyTargetKind::Blockquote,
            ]
        );
        // The whole response is the untouched Markdown source.
        assert_eq!(targets[0].source, source);
        // Code blocks copy only their content, without fences or info string.
        assert_eq!(targets[1].source, "fn main() {}\n");
        // Quotes copy their content with the `>` markers stripped.
        assert_eq!(targets[2].source, "quoted line\n");
    }

    #[test]
    fn user_cell_yields_message_and_embedded_code_targets() {
        let message = "please review\n\n```sh\nls -la\n```\n";
        let targets = copy_targets(&[user(message)]);

        assert_eq!(
            kinds(&targets),
            vec![CopyTargetKind::UserMessage, CopyTargetKind::CodeBlock]
        );
        assert_eq!(targets[0].source, message);
        assert_eq!(targets[1].source, "ls -la\n");
    }

    #[test]
    fn cells_without_source_payload_contribute_no_targets() {
        let targets = copy_targets(&[Arc::new(OtherCell)]);

        assert!(targets.is_empty());
    }

    #[test]
    fn targets_remember_their_cell_index_across_the_transcript() {
        let cells: Vec<Arc<dyn HistoryCell>> = vec![
            Arc::new(OtherCell),
            user("first question"),
            agent("first answer"),
            user("second question"),
        ];
        let targets = copy_targets(&cells);

        assert_eq!(
            targets
                .iter()
                .map(|target| (target.cell_index, target.kind))
                .collect::<Vec<_>>(),
            vec![
                (1, CopyTargetKind::UserMessage),
                (2, CopyTargetKind::WholeResponse),
                (3, CopyTargetKind::UserMessage),
            ]
        );
    }

    #[test]
    fn indented_code_blocks_are_not_copy_targets() {
        // Aligns with upstream: only fenced blocks become code targets.
        let targets = copy_targets(&[agent("prose\n\n    indented code\n")]);

        assert_eq!(kinds(&targets), vec![CopyTargetKind::WholeResponse]);
    }

    #[test]
    fn plan_cell_behaves_like_a_response() {
        let plan = "## Plan\n\n1. step one\n\n```\nsnippet\n```\n";
        let cell: Arc<dyn HistoryCell> = Arc::new(crate::history_cell::new_proposed_plan(
            plan.to_string(),
            Path::new("/tmp"),
            /*plan_file_path*/ None,
        ));
        let targets = copy_targets(&[cell]);

        assert_eq!(
            kinds(&targets),
            vec![CopyTargetKind::WholeResponse, CopyTargetKind::CodeBlock]
        );
        assert_eq!(targets[0].source, plan);
        assert_eq!(targets[1].source, "snippet\n");
    }

    #[test]
    fn empty_transcript_has_no_targets() {
        assert!(copy_targets(&[]).is_empty());
    }
}
