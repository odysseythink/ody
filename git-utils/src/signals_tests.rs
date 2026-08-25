use crate::signals::aggregate_signals;
use crate::signals::is_bug_fix_subject;
use crate::signals::parse_git_log_for_signals;
use crate::signals::render_signals_briefing;
use pretty_assertions::assert_eq;

const DAY: i64 = 86_400;
const NOW: i64 = 1_800_000_000;

fn log_block(timestamp: i64, subject: &str, files: &[&str]) -> String {
    let mut block = format!("\x1f{timestamp}\x1f{subject}\n");
    for file in files {
        block.push_str(file);
        block.push('\n');
    }
    block.push('\n');
    block
}

#[test]
fn parse_log_extracts_commit_timestamp_subject_and_files() {
    let text = format!(
        "{}{}",
        log_block(
            NOW - 10 * DAY,
            "fix: repair crash",
            &["src/a.rs", "src/b.rs"]
        ),
        log_block(NOW - 80 * DAY, "feat: add thing", &["src/c.rs"]),
    );
    let commits = parse_git_log_for_signals(&text);
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].timestamp, NOW - 10 * DAY);
    assert_eq!(commits[0].subject, "fix: repair crash");
    assert_eq!(commits[0].files, vec!["src/a.rs", "src/b.rs"]);
    assert_eq!(commits[1].files, vec!["src/c.rs"]);
}

#[test]
fn parse_log_skips_malformed_blocks() {
    let text = "garbage line\n\n".to_string() + &log_block(NOW, "feat: ok", &["x.rs"]);
    let commits = parse_git_log_for_signals(&text);
    assert_eq!(commits.len(), 1);
}

#[test]
fn bug_fix_subject_detection() {
    assert!(is_bug_fix_subject("fix: parser crash"));
    assert!(is_bug_fix_subject("fix(parser): crash"));
    assert!(is_bug_fix_subject("Fix typo"));
    assert!(is_bug_fix_subject("hotfix release"));
    assert!(is_bug_fix_subject("revert \"feat: add thing\""));
    assert!(is_bug_fix_subject("bugfix for login"));
    assert!(!is_bug_fix_subject("feat: add fixture support"));
    assert!(!is_bug_fix_subject("prefix: rename module"));
}

#[test]
fn aggregate_ranks_hotspots_with_recent_activity_first() {
    let text = {
        let mut t = String::new();
        // old.rs: many touches but all old (90d window edge)
        for i in 0..10 {
            t.push_str(&log_block(
                NOW - (80 - i) * DAY,
                "chore: bump",
                &["src/old.rs"],
            ));
        }
        // hot.rs: fewer total but recent
        for i in 0..3 {
            t.push_str(&log_block(
                NOW - (i + 1) * DAY,
                "feat: work",
                &["src/hot.rs"],
            ));
        }
        t
    };
    let commits = parse_git_log_for_signals(&text);
    let signals = aggregate_signals(&commits, NOW, 90);

    assert_eq!(signals.commits_scanned, 13);
    let hot = signals
        .hotspots
        .iter()
        .find(|h| h.path == "src/hot.rs")
        .expect("hot.rs in hotspots");
    assert_eq!(hot.touches_30d, 3);
    assert_eq!(hot.touches_window, 3);
    let old = signals
        .hotspots
        .iter()
        .find(|h| h.path == "src/old.rs")
        .expect("old.rs in hotspots");
    assert_eq!(old.touches_30d, 0);
    assert_eq!(old.touches_window, 10);
    // Recent activity outranks stale churn.
    assert_eq!(signals.hotspots[0].path, "src/hot.rs");
}

#[test]
fn aggregate_counts_bug_fix_touches_separately() {
    let text = format!(
        "{}{}{}",
        log_block(NOW - DAY, "fix: crash in parser", &["src/parser.rs"]),
        log_block(
            NOW - 2 * DAY,
            "fix: another parser bug",
            &["src/parser.rs", "src/lib.rs"]
        ),
        log_block(NOW - 3 * DAY, "feat: parser feature", &["src/parser.rs"]),
    );
    let commits = parse_git_log_for_signals(&text);
    let signals = aggregate_signals(&commits, NOW, 90);

    let bug = signals
        .bug_fix_hotspots
        .iter()
        .find(|h| h.path == "src/parser.rs")
        .expect("parser.rs bug-fix entry");
    assert_eq!(bug.touches_30d, 2);
    assert_eq!(bug.touches_window, 2);
    // Feature commit does not count as a bug fix.
    assert_eq!(signals.bug_fix_hotspots.len(), 2);
}

#[test]
fn aggregate_finds_co_change_pairs_with_threshold() {
    let mut text = String::new();
    for i in 0..4 {
        text.push_str(&log_block(
            NOW - (i + 1) * DAY,
            "feat: coupled change",
            &["src/a.rs", "src/b.rs", "src/c.rs"],
        ));
    }
    // a+b keep changing together; c only twice more.
    for i in 0..2 {
        text.push_str(&log_block(
            NOW - (10 + i) * DAY,
            "feat: more",
            &["src/a.rs", "src/b.rs"],
        ));
    }
    let commits = parse_git_log_for_signals(&text);
    let signals = aggregate_signals(&commits, NOW, 90);

    let top = &signals.co_change_pairs[0];
    assert_eq!(top.first, "src/a.rs");
    assert_eq!(top.second, "src/b.rs");
    assert_eq!(top.shared_commits, 6);
    // Pairs below the threshold of 3 shared commits are excluded.
    assert!(
        signals
            .co_change_pairs
            .iter()
            .all(|p| p.shared_commits >= 3)
    );
}

#[test]
fn aggregate_skips_huge_commits_for_co_change() {
    let files: Vec<String> = (0..60).map(|i| format!("src/file_{i}.rs")).collect();
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let text = log_block(NOW - DAY, "mega refactor", &refs);
    let commits = parse_git_log_for_signals(&text);
    let signals = aggregate_signals(&commits, NOW, 90);
    assert!(signals.co_change_pairs.is_empty());
}

#[test]
fn briefing_is_compact_and_contains_sections() {
    let mut text = String::new();
    for i in 0..5 {
        text.push_str(&log_block(
            NOW - (i + 1) * DAY,
            "fix: crash",
            &["src/parser.rs", "src/lexer.rs"],
        ));
    }
    let commits = parse_git_log_for_signals(&text);
    let signals = aggregate_signals(&commits, NOW, 90);
    let briefing = render_signals_briefing(&signals);

    assert!(briefing.contains("Git activity signals"));
    assert!(briefing.contains("5 commits"));
    assert!(briefing.contains("src/parser.rs"));
    assert!(briefing.contains("bug-fix"));
    assert!(briefing.contains("src/lexer.rs"));
    // Compact: a handful of signal lines, not a data dump.
    assert!(briefing.lines().count() <= 25);
}

#[test]
fn empty_history_renders_nothing() {
    let signals = aggregate_signals(&[], NOW, 90);
    assert_eq!(render_signals_briefing(&signals), "");
}
