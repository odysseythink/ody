use super::*;
use ody_app_server_protocol::ThreadItem;
use ody_app_server_protocol::TurnStatus;
use pretty_assertions::assert_eq;

#[test]
fn advancing_cursor_rejects_repeated_cursors() {
    let mut seen = HashSet::new();
    let first = advancing_cursor(None, Some("a".to_string()), &mut seen);
    assert_eq!(first.as_deref(), Some("a"));
    let second = advancing_cursor(first, Some("b".to_string()), &mut seen);
    assert_eq!(second.as_deref(), Some("b"));
    // A server that repeats a previously seen cursor must not loop forever.
    let repeated = advancing_cursor(second, Some("a".to_string()), &mut seen);
    assert_eq!(repeated, None);
}

#[test]
fn rendered_turns_rows_counts_cells_with_separators() {
    let cwd = std::path::Path::new("/tmp");
    let turn = |id: &str, text: &str| Turn {
        id: id.to_string(),
        items_view: TurnItemsView::Full,
        items: vec![ThreadItem::AgentMessage {
            id: format!("{id}-item"),
            text: text.to_string(),
            phase: None,
            memory_citation: None,
        }],
        status: TurnStatus::Completed,
        error: None,
        started_at: None,
        completed_at: None,
        duration_ms: None,
    };
    let zero = rendered_turns_rows(
        cwd,
        &[],
        RawReasoningVisibility::Hidden,
        HistoryRenderMode::Rich,
        80,
    );
    assert_eq!(zero, 0);
    let one = rendered_turns_rows(
        cwd,
        &[turn("t1", "hello")],
        RawReasoningVisibility::Hidden,
        HistoryRenderMode::Rich,
        80,
    );
    let two = rendered_turns_rows(
        cwd,
        &[turn("t1", "hello"), turn("t2", "hello")],
        RawReasoningVisibility::Hidden,
        HistoryRenderMode::Rich,
        80,
    );
    assert!(one > 0, "a single agent message must render rows");
    assert!(two > one, "rows must grow monotonically with more turns");
}
