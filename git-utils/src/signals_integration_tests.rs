use crate::signals::collect_git_signals;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

fn commit_file(repo: &Path, path: &str, content: &str, message: &str) {
    let full = repo.join(path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(&full, content).expect("write");
    git(repo, &["add", path]);
    git(repo, &["commit", "-m", message, "--no-gpg-sign"]);
}

#[tokio::test]
async fn collect_signals_from_real_repo() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = dir.path();
    git(repo, &["init", "--quiet"]);
    commit_file(repo, "src/parser.rs", "v1", "feat: initial parser");
    commit_file(
        repo,
        "src/parser.rs",
        "v2",
        "fix: parser crash on empty input",
    );
    commit_file(repo, "src/lexer.rs", "v1", "fix: lexer bug");
    commit_file(repo, "README.md", "readme", "docs: readme");

    let signals = collect_git_signals(repo, 90)
        .await
        .expect("git repo should yield signals");

    assert_eq!(signals.commits_scanned, 4);
    let bug_paths: Vec<&str> = signals
        .bug_fix_hotspots
        .iter()
        .map(|c| c.path.as_str())
        .collect();
    assert!(bug_paths.contains(&"src/parser.rs"));
    assert!(bug_paths.contains(&"src/lexer.rs"));
    assert!(!bug_paths.contains(&"README.md"));
}

#[tokio::test]
async fn collect_signals_returns_none_outside_git_repo() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(collect_git_signals(dir.path(), 90).await, None);
}
