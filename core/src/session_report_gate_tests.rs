//! Tests for the session-report gate. Decision tests are pure; scan and state
//! tests use a real git repository (skipped when git is unavailable, matching
//! the gate's fail-open contract).

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use pretty_assertions::assert_eq;

use super::BlockReport;
use super::Decision;
use super::GateSettings;
use super::GateState;
use super::WorkspaceScan;
use super::decide;
use super::evaluate;
use super::find_repos;
use super::read_state;
use super::sanitize_scope;
use super::scan_workspace;
use super::state_file_path;
use super::workspace_scope;

const SESSION_A: &str = "session-a";
const SESSION_B: &str = "session-b";

fn settings() -> GateSettings {
    GateSettings {
        cooldown: std::time::Duration::from_secs(300),
        min_files_changed: 1,
        max_repos: 25,
    }
}

fn scan<const N: usize>(fingerprint: u64, heads: &str, dirty_files: usize, changed: [&str; N]) -> WorkspaceScan {
    WorkspaceScan {
        fingerprint,
        heads: heads.to_string(),
        dirty_files,
        changed: changed.iter().map(|s| s.to_string()).collect(),
    }
}

fn state(session_id: &str, fingerprint: u64, heads: &str, fired_at_unix: u64) -> GateState {
    GateState {
        session_id: session_id.to_string(),
        fingerprint,
        heads: heads.to_string(),
        fired_at_unix,
    }
}

#[test]
fn no_baseline_records_one_and_allows() {
    let scan = scan(1, "h1|", 2, ["repo"]);
    assert_eq!(
        decide(None, &scan, SESSION_A, 1_000, &settings()),
        Decision::Allow
    );
}

#[test]
fn new_session_never_blamed_for_pre_existing_changes() {
    let previous = state(SESSION_A, 1, "h1|", 0);
    let scan = scan(2, "h1|", 3, ["repo"]);
    assert_eq!(
        decide(Some(&previous), &scan, SESSION_B, 1_000, &settings()),
        Decision::Allow
    );
}

#[test]
fn unchanged_fingerprint_allows() {
    let previous = state(SESSION_A, 1, "h1|", 900);
    let scan = scan(1, "h1|", 2, ["repo"]);
    assert_eq!(
        decide(Some(&previous), &scan, SESSION_A, 1_000, &settings()),
        Decision::Allow
    );
}

#[test]
fn heads_changed_blocks_even_with_clean_tree() {
    let previous = state(SESSION_A, 1, "h1|", 0);
    let scan = scan(2, "h1|h2|", 0, []);
    assert!(matches!(
        decide(Some(&previous), &scan, SESSION_A, 10_000, &settings()),
        Decision::Block(_)
    ));
}

#[test]
fn dirty_below_threshold_with_same_heads_allows() {
    let previous = state(SESSION_A, 1, "h1|", 0);
    // A tiny dirty-only change (e.g. a file touched twice back and forth) with
    // no commit landed and dirty count below min_files: not worth a report.
    let mut settings = settings();
    settings.min_files_changed = 3;
    let scan = scan(2, "h1|", 1, ["repo"]);
    assert_eq!(
        decide(Some(&previous), &scan, SESSION_A, 10_000, &settings),
        Decision::Allow
    );
}

#[test]
fn cooldown_suppresses_repeat_block() {
    let previous = state(SESSION_A, 1, "h1|", 9_000);
    let scan = scan(2, "h1|", 4, ["repo"]);
    // 100s elapsed < 300s cooldown: allow, and the pending change stays pending.
    assert_eq!(
        decide(Some(&previous), &scan, SESSION_A, 9_100, &settings()),
        Decision::Allow
    );
}

#[test]
fn block_lists_changed_repo_names() {
    let previous = state(SESSION_A, 1, "h1|", 0);
    let scan = scan(2, "h1|", 2, ["beta", "alpha", "beta"]);
    let Decision::Block(BlockReport { block_reason, changed }) =
        decide(Some(&previous), &scan, SESSION_A, 10_000, &settings())
    else {
        panic!("expected block");
    };
    assert_eq!(changed, vec!["alpha".to_string(), "beta".to_string()]);
    assert!(block_reason.contains("alpha, beta"));
    assert!(block_reason.contains("under ten lines"));
}

#[test]
fn sanitize_scope_strips_path_separators() {
    assert_eq!(sanitize_scope("my/repo\\name"), "my_repo_name");
    assert_eq!(sanitize_scope("..."), "...");
    assert_eq!(sanitize_scope("***"), "___");
    assert_eq!(sanitize_scope(""), "workspace");
}

#[test]
fn scope_uses_repo_name_for_single_repo() {
    let repos = vec![PathBuf::from("/tmp/hub/myrepo")];
    assert_eq!(workspace_scope(Path::new("/tmp/hub"), &repos), "myrepo");
}

#[test]
fn scope_uses_cwd_name_for_repo_hub() {
    let repos = vec![
        PathBuf::from("/tmp/hub/one"),
        PathBuf::from("/tmp/hub/two"),
    ];
    assert_eq!(workspace_scope(Path::new("/tmp/hub"), &repos), "hub");
}

// --- Real-git tests -------------------------------------------------------

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// Bare-bones repo: init, one commit.
fn init_repo(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "-b", "main"]);
    git(path, &["config", "user.email", "gate@test"]);
    git(path, &["config", "user.name", "gate"]);
    std::fs::write(path.join("file.txt"), "one\n").unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-m", "init"]);
}

#[test]
fn scan_sees_clean_repo_then_dirty_repo() {
    if !git_available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("demo");
    init_repo(&repo);

    let repos = find_repos(&repo, 25);
    assert_eq!(repos, vec![repo.canonicalize().unwrap()]);

    let clean = scan_workspace(&repos);
    assert_eq!(clean.dirty_files, 0);
    assert!(clean.changed.is_empty());

    std::fs::write(repo.join("file.txt"), "two\n").unwrap();
    let dirty = scan_workspace(&repos);
    assert_eq!(dirty.dirty_files, 1);
    assert_eq!(dirty.changed, vec!["demo".to_string()]);
    assert_ne!(clean.fingerprint, dirty.fingerprint);
}

#[test]
fn scan_content_hash_catches_second_edit() {
    if !git_available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("demo");
    init_repo(&repo);
    let repos = find_repos(&repo, 25);

    std::fs::write(repo.join("file.txt"), "two\n").unwrap();
    let first = scan_workspace(&repos);
    std::fs::write(repo.join("file.txt"), "three\n").unwrap();
    let second = scan_workspace(&repos);
    // Same file, same dirty count — only the content hash sees the edit.
    assert_eq!(first.dirty_files, second.dirty_files);
    assert_ne!(first.fingerprint, second.fingerprint);
}

#[test]
fn find_repos_discovers_sibling_repos() {
    if !git_available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let hub = temp.path().join("hub");
    let one = hub.join("one");
    let two = hub.join("two");
    init_repo(&one);
    init_repo(&two);

    let repos = find_repos(&hub, 25);
    assert_eq!(repos.len(), 2);

    let capped = find_repos(&hub, 1);
    assert_eq!(capped.len(), 1);
}

#[test]
fn find_repos_returns_empty_outside_git() {
    if !git_available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let repos = find_repos(temp.path(), 25);
    assert!(repos.is_empty());
}

#[test]
fn evaluate_first_stop_baselines_then_blocks_on_change() {
    if !git_available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let ody_home = temp.path().join("ody-home");
    let repo = temp.path().join("demo");
    init_repo(&repo);
    let settings = settings();

    // First stop of the session: baseline recorded, stop allowed.
    assert_eq!(
        evaluate(&repo, &ody_home, SESSION_A, &settings),
        Decision::Allow
    );
    let state_path = state_file_path(&ody_home, "demo");
    let stored = read_state(&state_path).expect("baseline persisted");
    assert_eq!(stored.session_id, SESSION_A);

    // No changes: still allowed, and the baseline is untouched.
    assert_eq!(
        evaluate(&repo, &ody_home, SESSION_A, &settings),
        Decision::Allow
    );

    // Work lands: block once.
    std::fs::write(repo.join("file.txt"), "two\n").unwrap();
    let decision = evaluate(&repo, &ody_home, SESSION_A, &settings);
    let Decision::Block(report) = decision else {
        panic!("expected block, got {decision:?}");
    };
    assert_eq!(report.changed, vec!["demo".to_string()]);
    let stored = read_state(&state_path).expect("state persisted");
    assert!(stored.fired_at_unix > 0);

    // The model produced its report but committed nothing: the baseline was
    // advanced at block time, so the next stop is allowed (no loop).
    assert_eq!(
        evaluate(&repo, &ody_home, SESSION_A, &settings),
        Decision::Allow
    );
}

#[test]
fn evaluate_allows_outside_git() {
    let temp = tempfile::tempdir().unwrap();
    let ody_home = temp.path().join("ody-home");
    assert_eq!(
        evaluate(temp.path(), &ody_home, SESSION_A, &settings()),
        Decision::Allow
    );
}
