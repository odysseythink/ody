//! Render persisted thread turns into history-cell building blocks.

use std::sync::Arc;

use crate::app_server_session::AppServerSession;
use crate::git_action_directives::parse_assistant_markdown;
use crate::history_cell::AgentMarkdownCell;
use crate::history_cell::HistoryCell;
use crate::history_cell::PlainHistoryCell;
use crate::history_cell::ReasoningSummaryCell;
use crate::history_cell::UserHistoryCell;
use crate::multi_agents::sub_agent_activity_summary;
use ody_app_server_protocol::DynamicToolCallOutputContentItem;
use ody_app_server_protocol::Thread;
use ody_app_server_protocol::ThreadItem;
use ody_protocol::ThreadId;
use ody_protocol::items::UserMessageItem;
use ratatui::style::Stylize as _;
use ratatui::text::Line;

pub(crate) type TranscriptCells = Vec<Arc<dyn HistoryCell>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RawReasoningVisibility {
    Hidden,
    Visible,
}

pub(crate) async fn load_session_transcript(
    app_server: &mut AppServerSession,
    thread_id: ThreadId,
    raw_reasoning_visibility: RawReasoningVisibility,
) -> std::io::Result<TranscriptCells> {
    let thread = app_server
        .thread_read(thread_id, /*include_turns*/ true)
        .await
        .map_err(std::io::Error::other)?;
    Ok(thread_to_transcript_cells(
        &thread,
        raw_reasoning_visibility,
    ))
}

pub(crate) fn thread_to_transcript_cells(
    thread: &Thread,
    raw_reasoning_visibility: RawReasoningVisibility,
) -> TranscriptCells {
    let cwd = thread.cwd.as_path();
    let mut cells: TranscriptCells = Vec::new();
    for item in thread.turns.iter().flat_map(|turn| turn.items.iter()) {
        match item {
            ThreadItem::UserMessage {
                id,
                client_id,
                content,
            } => {
                let item = UserMessageItem {
                    id: id.clone(),
                    client_id: client_id.clone(),
                    content: content
                        .iter()
                        .cloned()
                        .map(ody_app_server_protocol::UserInput::into_core)
                        .collect(),
                };
                cells.push(Arc::new(UserHistoryCell {
                    message: item.message(),
                    text_elements: item.text_elements(),
                    local_image_paths: item.local_image_paths(),
                    remote_image_urls: item.image_urls(),
                }));
            }
            ThreadItem::AgentMessage { text, .. } => {
                let parsed = parse_assistant_markdown(text, cwd);
                if !parsed.visible_markdown.trim().is_empty() {
                    cells.push(Arc::new(AgentMarkdownCell::new(
                        parsed.visible_markdown,
                        cwd,
                    )));
                }
            }
            ThreadItem::Plan {
                text,
                plan_file_path,
                ..
            } => {
                if !text.trim().is_empty() {
                    cells.push(Arc::new(crate::history_cell::new_proposed_plan(
                        text.clone(),
                        cwd,
                        plan_file_path.clone(),
                    )));
                }
            }
            ThreadItem::Reasoning {
                summary, content, ..
            } => {
                let text = if matches!(raw_reasoning_visibility, RawReasoningVisibility::Visible)
                    && !content.is_empty()
                {
                    content.join("\n\n")
                } else {
                    summary.join("\n\n")
                };
                if !text.trim().is_empty() {
                    cells.push(Arc::new(ReasoningSummaryCell::new(
                        "Reasoning".to_string(),
                        text,
                        cwd,
                        /*transcript_only*/ false,
                    )));
                }
            }
            other => {
                if let Some(cell) = fallback_transcript_cell(other) {
                    cells.push(Arc::new(cell));
                }
            }
        }
    }
    if cells.is_empty() {
        cells.push(Arc::new(PlainHistoryCell::new(vec![
            "No transcript content available".italic().dim().into(),
        ])));
    }
    cells
}

/// Build a short chip summary for a `WebSearch` dynamic tool call, matching the
/// TS `webSearchChip` behaviour: count result list items and emit
/// `N results`, `no results`, or `web result`.
fn web_search_chip_summary(content_items: &[DynamicToolCallOutputContentItem]) -> Option<String> {
    let text = content_items.iter().find_map(|item| match item {
        DynamicToolCallOutputContentItem::InputText { text } => Some(text.as_str()),
        _ => None,
    })?;

    // Rust `WebSearchTool` serialises a structured JSON output with
    // `result_count` and `text`. Use that for an accurate count when available.
    #[derive(serde::Deserialize)]
    struct WebSearchToolOutput {
        #[allow(dead_code)]
        text: String,
        result_count: usize,
    }

    if let Ok(output) = serde_json::from_str::<WebSearchToolOutput>(text) {
        if output.result_count > 0 {
            return Some(format!("{} results", output.result_count));
        }
        return Some("no results".to_string());
    }

    // Fallback to TS-style line counting for plain text outputs.
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let count = lines
        .iter()
        .filter(|l| {
            let trimmed = l.trim_start();
            trimmed.starts_with("- ")
                || trimmed.starts_with("* ")
                || trimmed
                    .split_once(". ")
                    .is_some_and(|(prefix, _)| prefix.parse::<usize>().is_ok())
        })
        .count();

    if count > 0 {
        return Some(format!("{} results", count));
    }
    if lines.is_empty() {
        return Some("no results".to_string());
    }
    Some("web result".to_string())
}

fn fallback_transcript_cell(item: &ThreadItem) -> Option<PlainHistoryCell> {
    let lines = match item {
        ThreadItem::HookPrompt { fragments, .. } => fragments
            .iter()
            .map(|fragment| {
                vec![
                    "hook prompt: ".dim(),
                    fragment.text.trim().to_string().into(),
                ]
                .into()
            })
            .collect::<Vec<_>>(),
        ThreadItem::CommandExecution {
            command,
            status,
            aggregated_output,
            exit_code,
            ..
        } => {
            let mut lines: Vec<Line<'static>> =
                vec![vec!["$ ".dim(), command.clone().into()].into()];
            lines.push(
                format!(
                    "status: {status:?}{}",
                    exit_code
                        .map(|code| format!(" · exit {code}"))
                        .unwrap_or_default()
                )
                .dim()
                .into(),
            );
            if let Some(output) = aggregated_output.as_deref()
                && !output.trim().is_empty()
            {
                lines.extend(
                    output
                        .lines()
                        .map(|line| vec!["  ".dim(), line.trim_end().to_string().dim()].into()),
                );
            }
            lines
        }
        ThreadItem::FileChange {
            changes, status, ..
        } => vec![
            format!("file changes: {status:?} · {} changes", changes.len())
                .dim()
                .into(),
        ],
        ThreadItem::McpToolCall {
            server,
            tool,
            status,
            ..
        } => vec![
            format!("mcp tool: {server}/{tool} · {status:?}")
                .dim()
                .into(),
        ],
        ThreadItem::DynamicToolCall {
            namespace,
            tool,
            status,
            content_items,
            ..
        } => {
            if namespace.is_none() && tool == "WebSearch" {
                if let Some(chip) =
                    web_search_chip_summary(content_items.as_deref().unwrap_or_default())
                {
                    return Some(PlainHistoryCell::new(vec![
                        format!("WebSearch · {chip}").dim().into(),
                    ]));
                }
            }
            let name = namespace
                .as_ref()
                .map(|namespace| format!("{namespace}/{tool}"))
                .unwrap_or_else(|| tool.clone());
            vec![format!("tool: {name} · {status:?}").dim().into()]
        }
        ThreadItem::CollabAgentToolCall { tool, status, .. } => {
            vec![format!("agent tool: {tool:?} · {status:?}").dim().into()]
        }
        ThreadItem::SubAgentActivity {
            kind, agent_path, ..
        } => {
            vec![sub_agent_activity_summary(*kind, agent_path).dim().into()]
        }
        ThreadItem::ImageView { path, .. } => {
            vec![format!("image: {}", path.as_path().display()).dim().into()]
        }
        ThreadItem::ImageGeneration {
            status, saved_path, ..
        } => {
            let saved = saved_path
                .as_ref()
                .map(|path| format!(" · {}", path.as_path().display()))
                .unwrap_or_default();
            vec![format!("image generation: {status}{saved}").dim().into()]
        }
        ThreadItem::EnteredReviewMode { review, .. } => {
            vec![vec!["review started: ".dim(), review.clone().into()].into()]
        }
        ThreadItem::ExitedReviewMode { review, .. } => {
            vec![vec!["review finished: ".dim(), review.clone().into()].into()]
        }
        ThreadItem::ContextCompaction { .. } => {
            vec!["context compacted".dim().into()]
        }
        ThreadItem::UserMessage { .. }
        | ThreadItem::AgentMessage { .. }
        | ThreadItem::Plan { .. }
        | ThreadItem::Reasoning { .. }
        | ThreadItem::Sleep { .. } => return None,
    };
    (!lines.is_empty()).then(|| PlainHistoryCell::new(lines))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_app_server_protocol::DynamicToolCallStatus;
    use ody_utils_absolute_path::AbsolutePathBuf;
    use serde_json::json;

    fn input_text(text: &str) -> Vec<DynamicToolCallOutputContentItem> {
        vec![DynamicToolCallOutputContentItem::InputText {
            text: text.to_string(),
        }]
    }

    #[test]
    fn web_search_chip_summary_from_structured_json() {
        let text = json!({
            "result_count": 3usize,
            "text": "Example\nhttps://example.com\nSnippet\n\n"
        })
        .to_string();
        assert_eq!(
            web_search_chip_summary(&input_text(&text)),
            Some("3 results".to_string())
        );
    }

    #[test]
    fn web_search_chip_summary_no_results_from_structured_json() {
        let text = json!({
            "result_count": 0usize,
            "text": "No search results found."
        })
        .to_string();
        assert_eq!(
            web_search_chip_summary(&input_text(&text)),
            Some("no results".to_string())
        );
    }

    #[test]
    fn web_search_chip_summary_counts_markdown_list_items() {
        let text = "1. First result\n2. Second result\n\n* bullet\n";
        assert_eq!(
            web_search_chip_summary(&input_text(text)),
            Some("3 results".to_string())
        );
    }

    #[test]
    fn web_search_chip_summary_no_results_for_empty_output() {
        assert_eq!(
            web_search_chip_summary(&input_text("")),
            Some("no results".to_string())
        );
    }

    #[test]
    fn web_search_chip_summary_web_result_for_plain_text() {
        assert_eq!(
            web_search_chip_summary(&input_text("some plain text output")),
            Some("web result".to_string())
        );
    }

    #[test]
    fn web_search_chip_summary_missing_for_non_text_content() {
        let items = vec![DynamicToolCallOutputContentItem::InputImage {
            image_url: "https://example.com/image.png".to_string(),
        }];
        assert_eq!(web_search_chip_summary(&items), None);
    }

    fn web_search_thread_item(content_items: Vec<DynamicToolCallOutputContentItem>) -> ThreadItem {
        ThreadItem::DynamicToolCall {
            id: "call-1".to_string(),
            namespace: None,
            tool: "WebSearch".to_string(),
            arguments: json!({"query": "hello"}),
            status: DynamicToolCallStatus::Completed,
            content_items: Some(content_items),
            success: Some(true),
            duration_ms: Some(123),
        }
    }

    #[test]
    fn fallback_transcript_cell_renders_web_search_chip() {
        let item = web_search_thread_item(input_text(
            &json!({"result_count": 2usize, "text": "..."}).to_string(),
        ));
        let cell = fallback_transcript_cell(&item).expect("should render");
        let lines = cell.display_lines(80);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].to_string(), "WebSearch · 2 results");
    }

    #[test]
    fn fallback_transcript_cell_renders_web_search_no_results() {
        let item = web_search_thread_item(input_text(
            &json!({"result_count": 0usize, "text": "No search results found."}).to_string(),
        ));
        let cell = fallback_transcript_cell(&item).expect("should render");
        let lines = cell.display_lines(80);
        assert_eq!(lines[0].to_string(), "WebSearch · no results");
    }

    #[test]
    fn fallback_transcript_cell_falls_back_for_non_web_search_dynamic_tool() {
        let item = ThreadItem::DynamicToolCall {
            id: "call-2".to_string(),
            namespace: None,
            tool: "OtherTool".to_string(),
            arguments: json!({}),
            status: DynamicToolCallStatus::Completed,
            content_items: None,
            success: Some(true),
            duration_ms: None,
        };
        let cell = fallback_transcript_cell(&item).expect("should render");
        let lines = cell.display_lines(80);
        assert_eq!(lines[0].to_string(), "tool: OtherTool · Completed");
    }

    #[test]
    fn thread_to_transcript_cells_includes_web_search_chip() {
        let thread = Thread {
            id: "thread-1".to_string(),
            session_id: "session-1".to_string(),
            forked_from_id: None,
            parent_thread_id: None,
            preview: String::new(),
            ephemeral: false,
            model_provider: "kimi".to_string(),
            created_at: 0,
            updated_at: 0,
            recency_at: None,
            status: ody_app_server_protocol::ThreadStatus::Idle,
            path: None,
            cwd: AbsolutePathBuf::from_absolute_path(std::env::current_dir().unwrap()).unwrap(),
            cli_version: "0.0.0".to_string(),
            source: ody_app_server_protocol::SessionSource::Cli,
            thread_source: None,
            agent_nickname: None,
            agent_role: None,
            git_info: None,
            name: None,
            turns: vec![ody_app_server_protocol::Turn {
                id: "turn-1".to_string(),
                items: vec![web_search_thread_item(input_text(
                    &json!({"result_count": 5usize, "text": "..."}).to_string(),
                ))],
                items_view: ody_app_server_protocol::TurnItemsView::Full,
                status: ody_app_server_protocol::TurnStatus::Completed,
                error: None,
                started_at: None,
                completed_at: None,
                duration_ms: None,
            }],
        };
        let cells = thread_to_transcript_cells(&thread, RawReasoningVisibility::Hidden);
        assert_eq!(cells.len(), 1);
        let lines = cells[0].display_lines(80);
        assert_eq!(lines[0].to_string(), "WebSearch · 5 results");
    }
}
