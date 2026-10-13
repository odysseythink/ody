//! End-to-end coverage for paginated resume: bounded initial page, row-budget
//! hydration, and scrollback top-up against a real embedded app server.

use super::*;
use ody_protocol::ThreadId;
use pretty_assertions::assert_eq;

const TEST_TIMESTAMP: &str = "2025-01-01T00-00-00";
// Mirrors `ody_rollout::SESSIONS_SUBDIR` (ody-tui does not depend on ody-core).
const SESSIONS_SUBDIR: &str = "sessions";

/// Write a synthetic rollout with `turn_count` turns (user message + agent
/// reply each) so the embedded app server can resume it through the real
/// `thread/resume` + `thread/turns/list` protocol surface.
pub(super) async fn write_multi_turn_rollout(
    ody_home: &std::path::Path,
    thread_id: ThreadId,
    turn_count: usize,
) -> std::path::PathBuf {
    use ody_protocol::protocol::AgentMessageEvent;
    use ody_protocol::protocol::EventMsg;
    use ody_protocol::protocol::RolloutItem;
    use ody_protocol::protocol::RolloutLine;
    use ody_protocol::protocol::SessionMeta;
    use ody_protocol::protocol::SessionMetaLine;
    use ody_protocol::protocol::TurnContextItem;
    use ody_protocol::protocol::UserMessageEvent;

    let dir = ody_home
        .join(SESSIONS_SUBDIR)
        .join("2025")
        .join("01")
        .join("01");
    tokio::fs::create_dir_all(&dir).await.expect("mkdir");
    let file_path = dir.join(format!("rollout-{TEST_TIMESTAMP}-{thread_id}.jsonl"));
    let mut file = tokio::fs::File::create(&file_path).await.expect("create");

    let mut lines: Vec<RolloutLine> = vec![RolloutLine {
        timestamp: TEST_TIMESTAMP.to_string(),
        item: RolloutItem::SessionMeta(SessionMetaLine {
            meta: SessionMeta {
                session_id: thread_id.into(),
                id: thread_id,
                forked_from_id: None,
                parent_thread_id: None,
                timestamp: TEST_TIMESTAMP.to_string(),
                cwd: std::path::PathBuf::from("/tmp"),
                originator: "test_originator".to_string(),
                cli_version: "test_version".to_string(),
                source: ody_protocol::protocol::SessionSource::Cli,
                thread_source: None,
                agent_nickname: None,
                agent_path: None,
                agent_role: None,
                model_provider: None,
                base_instructions: None,
                dynamic_tools: None,
                memory_mode: None,
                multi_agent_version: None,
            },
            git: None,
        }),
    }];
    for index in 0..turn_count {
        lines.push(RolloutLine {
            timestamp: TEST_TIMESTAMP.to_string(),
            item: RolloutItem::TurnContext(turn_context_item(&format!("turn-{index:03}"))),
        });
        lines.push(RolloutLine {
            timestamp: TEST_TIMESTAMP.to_string(),
            item: RolloutItem::EventMsg(EventMsg::UserMessage(UserMessageEvent {
                client_id: None,
                message: format!("user message {index}"),
                images: None,
                text_elements: Vec::new(),
                local_images: Vec::new(),
                ..Default::default()
            })),
        });
        lines.push(RolloutLine {
            timestamp: TEST_TIMESTAMP.to_string(),
            item: RolloutItem::EventMsg(EventMsg::AgentMessage(AgentMessageEvent {
                message: format!("agent reply {index}"),
                phase: None,
                memory_citation: None,
            })),
        });
    }
    for line in lines {
        let json = serde_json::to_string(&line).expect("serialize rollout line");
        use tokio::io::AsyncWriteExt as _;
        file.write_all(format!("{json}\n").as_bytes())
            .await
            .expect("write rollout line");
    }
    file_path
}

fn turn_context_item(turn_id: &str) -> ody_protocol::protocol::TurnContextItem {
    // 字段全集见 protocol/src/protocol.rs 的 `TurnContextItem`；构造样例见
    // core/src/context_manager/history_tests.rs。
    ody_protocol::protocol::TurnContextItem {
        turn_id: Some(turn_id.to_string()),
        cwd: test_path_buf("/tmp").abs(),
        workspace_roots: None,
        current_date: None,
        timezone: None,
        approval_policy: ody_protocol::protocol::AskForApproval::Never,
        sandbox_policy: ody_protocol::protocol::SandboxPolicy::new_read_only_policy(),
        permission_profile: None,
        network: None,
        file_system_sandbox_policy: None,
        model: "test-model".to_string(),
        comp_hash: None,
        personality: None,
        collaboration_mode: None,
        multi_agent_version: None,
        multi_agent_mode: None,
        realtime_active: Some(false),
        effort: None,
        summary: Default::default(),
    }
}

/// Extract the first user-message text of each turn. The app server assigns
/// synthetic `rollout-{line}` turn ids and never reads back the fixture's
/// `TurnContext.turn_id`, so cross-page ordering is asserted on message
/// content instead of literal ids.
pub(super) fn turn_user_texts(turns: &[Turn]) -> Vec<String> {
    turns
        .iter()
        .map(|turn| {
            turn.items
                .iter()
                .find_map(|item| match item {
                    ThreadItem::UserMessage { content, .. } => {
                        content.iter().find_map(|input| match input {
                            UserInput::Text { text, .. } => Some(text.clone()),
                            _ => None,
                        })
                    }
                    _ => None,
                })
                .expect("synthetic turn has a user message")
        })
        .collect()
}

#[tokio::test]
async fn resume_returns_bounded_initial_turns_page() -> Result<()> {
    let (mut app, mut app_event_rx, _op_rx) = make_test_app_with_channels().await;
    while app_event_rx.try_recv().is_ok() {}
    let mut config = app.chat_widget.config_ref().clone();
    // 小 cap 让 hydration 在初始页即停：每 turn 渲染数行，5 turn 远超 10 行预算。
    config.terminal_resize_reflow.max_rows = TerminalResizeReflowMaxRows::Limit(10);
    let thread_id = ThreadId::new();
    write_multi_turn_rollout(config.ody_home.as_path(), thread_id, /*turn_count*/ 7).await;
    let mut app_server = Box::pin(crate::start_embedded_app_server_for_picker(&config)).await?;

    let started = Box::pin(app_server.resume_thread(config, thread_id))
        .await
        .expect("resume synthetic rollout");
    assert_eq!(started.turns.len(), 5, "initial page is capped at 5 turns");
    assert!(
        app_server.has_older_history(thread_id),
        "older turns must remain pageable"
    );
    // 时间序 + 无重复 + 取最新 5 个 turn。
    let ids: Vec<&str> = started.turns.iter().map(|turn| turn.id.as_str()).collect();
    let unique_ids: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique_ids.len(), ids.len(), "turn ids must be unique");
    assert_eq!(
        turn_user_texts(&started.turns),
        vec![
            "user message 2",
            "user message 3",
            "user message 4",
            "user message 5",
            "user message 6",
        ]
    );
    Ok(())
}

#[tokio::test]
async fn hydration_respects_budget_and_top_up_reaches_oldest() -> Result<()> {
    let (mut app, mut app_event_rx, _op_rx) = make_test_app_with_channels().await;
    while app_event_rx.try_recv().is_ok() {}
    let mut config = app.chat_widget.config_ref().clone();
    // 预算：每 turn 数行，Limit(60) 强制 hydration 翻数页后停，不拉完全部 30 turn。
    config.terminal_resize_reflow.max_rows = TerminalResizeReflowMaxRows::Limit(60);
    let thread_id = ThreadId::new();
    write_multi_turn_rollout(config.ody_home.as_path(), thread_id, /*turn_count*/ 30).await;
    let mut app_server = Box::pin(crate::start_embedded_app_server_for_picker(&config)).await?;

    let started = Box::pin(app_server.resume_thread(config, thread_id))
        .await
        .expect("resume synthetic rollout");
    assert!(
        started.turns.len() > 5,
        "hydration must page past the initial page: got {}",
        started.turns.len()
    );
    assert!(
        started.turns.len() < 30,
        "budget must stop hydration before the oldest turn: got {}",
        started.turns.len()
    );
    assert!(app_server.has_older_history(thread_id));

    // 接入 App 回放通道（建立 store.turns），模拟 resume 后主视口 top-up。
    Box::pin(app.enqueue_primary_thread_session(
        started.session,
        started.turns,
        /*initial_collaboration_mask*/ None,
    ))
    .await?;
    while app_event_rx.try_recv().is_ok() {}

    let mut tui = crate::tui::test_support::make_test_tui()?;
    let mut applied_pages = 0usize;
    while app_server.has_older_history(thread_id) {
        // `handle_older_history_page` chains the next page request itself while
        // the viewport is still under the row budget, so a request may already
        // be in flight; a rejected request here just means "wait for the
        // in-flight page", not stalled pagination.
        app.request_older_history_page(&mut app_server, thread_id);
        let event = tokio::time::timeout(Duration::from_secs(5), app_event_rx.recv())
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("event channel closed"))?;
        if matches!(event, AppEvent::OlderThreadHistoryLoaded { .. }) {
            Box::pin(app.handle_event(&mut tui, &mut app_server, event)).await?;
            applied_pages += 1;
        }
        assert!(
            applied_pages <= 10,
            "top-up must terminate: too many pages applied"
        );
    }
    assert!(!app.scrollback_has_older_history || !app_server.has_older_history(thread_id));

    // 终态断言：30 个 turn 全部到达 store，时间序、无重复。
    let channel = app
        .thread_event_channels
        .get(&thread_id)
        .expect("replay channel");
    let store = channel.store.lock().await;
    assert_eq!(store.turns.len(), 30);
    let ids: Vec<&str> = store.turns.iter().map(|turn| turn.id.as_str()).collect();
    let unique_ids: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique_ids.len(), ids.len(), "turn ids must be unique");
    let expected: Vec<String> = (0..30).map(|i| format!("user message {i}")).collect();
    assert_eq!(turn_user_texts(&store.turns), expected);
    Ok(())
}

#[tokio::test]
async fn resume_without_cap_degrades_to_full_history() -> Result<()> {
    let (mut app, mut app_event_rx, _op_rx) = make_test_app_with_channels().await;
    while app_event_rx.try_recv().is_ok() {}
    let mut config = app.chat_widget.config_ref().clone();
    config.terminal_resize_reflow.max_rows = TerminalResizeReflowMaxRows::Disabled;
    let thread_id = ThreadId::new();
    write_multi_turn_rollout(config.ody_home.as_path(), thread_id, /*turn_count*/ 12).await;
    let mut app_server = Box::pin(crate::start_embedded_app_server_for_picker(&config)).await?;

    let started = Box::pin(app_server.resume_thread(config, thread_id))
        .await
        .expect("resume synthetic rollout");
    assert_eq!(started.turns.len(), 12);
    assert!(
        !app_server.has_older_history(thread_id),
        "full hydration must exhaust the cursor"
    );
    Ok(())
}

/// P0 performance baseline for the fullscreen-transcript roadmap
/// (`.ody-code/roadmaps/2026-10-13-tui-fullscreen-transcript-port.md`).
///
/// Measures the current Terminal-mode resume path end to end for a 1000-turn
/// session with no row cap (worst case: full history replay): resume RPC time,
/// cell formatting time, line layout time at 80 columns, and the terminal
/// replay cost (wall time, backend write calls, bytes written).
///
/// Run manually with:
/// `cargo test -p ody-tui --lib resume_baseline_1000_turns -- --ignored --nocapture`
#[tokio::test]
#[ignore = "manual performance baseline; not part of CI"]
async fn resume_baseline_1000_turns_terminal_mode() -> Result<()> {
    use std::time::Instant;

    let (mut app, mut app_event_rx, _op_rx) = make_test_app_with_channels().await;
    while app_event_rx.try_recv().is_ok() {}
    let mut config = app.chat_widget.config_ref().clone();
    config.terminal_resize_reflow.max_rows = TerminalResizeReflowMaxRows::Disabled;
    let thread_id = ThreadId::new();

    let fixture_start = Instant::now();
    write_multi_turn_rollout(config.ody_home.as_path(), thread_id, /*turn_count*/ 1000).await;
    let fixture_elapsed = fixture_start.elapsed();

    let mut app_server = Box::pin(crate::start_embedded_app_server_for_picker(&config)).await?;

    let resume_start = Instant::now();
    let started = Box::pin(app_server.resume_thread(config, thread_id))
        .await
        .expect("resume synthetic rollout");
    let resume_elapsed = resume_start.elapsed();

    let cells_start = Instant::now();
    let cells = crate::thread_transcript::turns_to_transcript_cells(
        started.session.cwd.as_path(),
        &started.turns,
        crate::thread_transcript::RawReasoningVisibility::Hidden,
    );
    let cells_elapsed = cells_start.elapsed();

    let layout_start = Instant::now();
    let lines: Vec<_> = cells
        .iter()
        .flat_map(|cell| {
            cell.display_hyperlink_lines_for_mode(/*width*/ 80, crate::history_cell::HistoryRenderMode::Rich)
        })
        .map(|line| line.line)
        .collect();
    let layout_elapsed = layout_start.elapsed();

    let backend = crate::test_backend::VT100Backend::new(/*width*/ 80, /*height*/ 24);
    let mut terminal =
        crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
    terminal.set_viewport_area(ratatui::layout::Rect::new(0, 23, 80, 1));
    let insert_start = Instant::now();
    let line_count = lines.len();
    crate::insert_history::insert_history_lines(&mut terminal, lines).expect("insert history");
    let insert_elapsed = insert_start.elapsed();

    eprintln!(
        "BASELINE turns={} fixture={fixture_elapsed:?} resume={resume_elapsed:?} \
         cells={} format={cells_elapsed:?} lines={} layout={layout_elapsed:?} \
         insert={insert_elapsed:?} writes={} bytes={}",
        started.turns.len(),
        cells.len(),
        line_count,
        terminal.backend().write_count(),
        terminal.backend().byte_count(),
    );
    Ok(())
}
