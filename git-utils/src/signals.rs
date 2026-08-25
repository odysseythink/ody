//! Git history activity signals: hot files, bug-magnet files, and hidden
//! co-change coupling, computed from `git log` only (no LLM, no index).
//!
//! The collection path is split into IO (`collect_git_signals`) and pure
//! functions (`parse_git_log_for_signals`, `aggregate_signals`,
//! `render_signals_briefing`) so aggregation stays unit-testable.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

/// Caps the scan so huge repositories stay fast; 2000 commits of
/// `--name-only` output is a few hundred KB at most.
const MAX_COMMITS_SCANNED: usize = 2_000;
/// Commits touching more files than this are bulk/refactor commits whose
/// co-change pairs are noise.
const MAX_FILES_PER_COMMIT_FOR_COUPLING: usize = 40;
const MIN_SHARED_COMMITS_FOR_COUPLING: u32 = 3;
const TOP_HOTSPOTS: usize = 8;
const TOP_BUG_FIXES: usize = 8;
const TOP_CO_CHANGE: usize = 5;
const RECENT_WINDOW_DAYS: i64 = 30;
const SECS_PER_DAY: i64 = 86_400;

/// One commit's signal-relevant data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalCommit {
    pub timestamp: i64,
    pub subject: String,
    pub files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChurn {
    pub path: String,
    pub touches_30d: u32,
    pub touches_window: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoChangePair {
    pub first: String,
    pub second: String,
    pub shared_commits: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitSignals {
    pub window_days: u32,
    pub commits_scanned: usize,
    pub hotspots: Vec<FileChurn>,
    pub bug_fix_hotspots: Vec<FileChurn>,
    pub co_change_pairs: Vec<CoChangePair>,
}

/// Parses `git log --pretty=format:%x1f%ct%x1f%s --name-only` output.
pub fn parse_git_log_for_signals(text: &str) -> Vec<SignalCommit> {
    let mut commits = Vec::new();
    let mut current: Option<SignalCommit> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('\x1f') {
            if let Some(commit) = current.take() {
                commits.push(commit);
            }
            let mut parts = rest.split('\x1f');
            let timestamp = parts.next().and_then(|s| s.trim().parse::<i64>().ok());
            let subject = parts.next().unwrap_or("").trim().to_string();
            current = timestamp.map(|timestamp| SignalCommit {
                timestamp,
                subject,
                files: Vec::new(),
            });
        } else if let Some(commit) = current.as_mut() {
            let file = line.trim();
            if !file.is_empty() {
                commit.files.push(file.to_string());
            }
        }
    }
    if let Some(commit) = current.take() {
        commits.push(commit);
    }
    commits
}

/// Heuristic: does this commit subject describe a bug fix?
// ody: 纯关键词启发,不区分 conventional-commit 类型与语义;docs/test 提交未排除。
// 升级触发条件:若 bug-magnet 简报出现明显误报(如大量 "fix typo" 文档提交),
// 再引入路径过滤(排除 docs/**、*.md)或提交体重量衰减。
pub fn is_bug_fix_subject(subject: &str) -> bool {
    const BUG_FIX_WORDS: [&str; 7] = ["fix", "fixes", "fixed", "bugfix", "hotfix", "bug", "revert"];
    subject
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| BUG_FIX_WORDS.contains(&word))
}

/// Aggregates parsed commits into ranked signal lists.
pub fn aggregate_signals(commits: &[SignalCommit], now_unix: i64, window_days: u32) -> GitSignals {
    let window_start = now_unix - i64::from(window_days) * SECS_PER_DAY;
    let recent_start = now_unix - RECENT_WINDOW_DAYS * SECS_PER_DAY;

    let mut touches: HashMap<&str, (u32, u32)> = HashMap::new();
    let mut bug_fixes: HashMap<&str, (u32, u32)> = HashMap::new();
    let mut pair_counts: HashMap<(&str, &str), u32> = HashMap::new();
    let mut commits_scanned = 0;

    for commit in commits {
        if commit.timestamp < window_start {
            continue;
        }
        commits_scanned += 1;
        let is_recent = commit.timestamp >= recent_start;
        let unique_files: HashSet<&str> = commit.files.iter().map(String::as_str).collect();

        for file in &unique_files {
            let entry = touches.entry(file).or_default();
            entry.1 += 1;
            if is_recent {
                entry.0 += 1;
            }
        }

        if is_bug_fix_subject(&commit.subject) {
            for file in &unique_files {
                let entry = bug_fixes.entry(file).or_default();
                entry.1 += 1;
                if is_recent {
                    entry.0 += 1;
                }
            }
        }

        if unique_files.len() >= 2 && unique_files.len() <= MAX_FILES_PER_COMMIT_FOR_COUPLING {
            let mut sorted: Vec<&str> = unique_files.iter().copied().collect();
            sorted.sort_unstable();
            for (i, first) in sorted.iter().enumerate() {
                for second in sorted.iter().skip(i + 1) {
                    *pair_counts.entry((first, second)).or_default() += 1;
                }
            }
        }
    }

    let mut hotspots: Vec<FileChurn> = touches
        .into_iter()
        .map(|(path, (touches_30d, touches_window))| FileChurn {
            path: path.to_string(),
            touches_30d,
            touches_window,
        })
        .collect();
    hotspots.sort_by(|a, b| {
        b.touches_30d
            .cmp(&a.touches_30d)
            .then(b.touches_window.cmp(&a.touches_window))
            .then(a.path.cmp(&b.path))
    });
    hotspots.truncate(TOP_HOTSPOTS);

    let mut bug_fix_hotspots: Vec<FileChurn> = bug_fixes
        .into_iter()
        .map(|(path, (touches_30d, touches_window))| FileChurn {
            path: path.to_string(),
            touches_30d,
            touches_window,
        })
        .collect();
    bug_fix_hotspots.sort_by(|a, b| {
        b.touches_30d
            .cmp(&a.touches_30d)
            .then(b.touches_window.cmp(&a.touches_window))
            .then(a.path.cmp(&b.path))
    });
    bug_fix_hotspots.truncate(TOP_BUG_FIXES);

    let mut co_change_pairs: Vec<CoChangePair> = pair_counts
        .into_iter()
        .filter(|(_, count)| *count >= MIN_SHARED_COMMITS_FOR_COUPLING)
        .map(|((first, second), shared_commits)| CoChangePair {
            first: first.to_string(),
            second: second.to_string(),
            shared_commits,
        })
        .collect();
    co_change_pairs.sort_by(|a, b| {
        b.shared_commits
            .cmp(&a.shared_commits)
            .then(a.first.cmp(&b.first))
            .then(a.second.cmp(&b.second))
    });
    co_change_pairs.truncate(TOP_CO_CHANGE);

    GitSignals {
        window_days,
        commits_scanned,
        hotspots,
        bug_fix_hotspots,
        co_change_pairs,
    }
}

/// Renders the compact session-start briefing. Empty string when there is
/// nothing worth saying.
pub fn render_signals_briefing(signals: &GitSignals) -> String {
    if signals.commits_scanned == 0 {
        return String::new();
    }
    let mut out = format!(
        "## Git activity signals (last {} days, {} commits scanned)\n",
        signals.window_days, signals.commits_scanned
    );
    if !signals.hotspots.is_empty() {
        out.push_str("\nMost active files:\n");
        for churn in &signals.hotspots {
            out.push_str(&format!(
                "- {} — {} changes ({} in last {}d)\n",
                churn.path, churn.touches_window, churn.touches_30d, RECENT_WINDOW_DAYS
            ));
        }
    }
    if !signals.bug_fix_hotspots.is_empty() {
        out.push_str("\nBug-fix magnets (files with the most bug-fix commits):\n");
        for churn in &signals.bug_fix_hotspots {
            out.push_str(&format!(
                "- {} — {} bug-fix commits ({} recent)\n",
                churn.path, churn.touches_window, churn.touches_30d
            ));
        }
    }
    if !signals.co_change_pairs.is_empty() {
        out.push_str("\nFrequently changed together (update both or explain why not):\n");
        for pair in &signals.co_change_pairs {
            out.push_str(&format!(
                "- {} ↔ {} ({} shared commits)\n",
                pair.first, pair.second, pair.shared_commits
            ));
        }
    }
    out
}

/// Collects signals for the repository containing `cwd` over the last
/// `window_days`. Returns `None` outside a git repo or on any git failure.
// ody: 每次 session start 现扫,无缓存;大仓库由 MAX_COMMITS_SCANNED 与调用方超时兜底。
// 升级触发条件:若 session start 延迟被投诉,按 HEAD sha 做增量缓存到 state DB。
pub async fn collect_git_signals(cwd: &Path, window_days: u32) -> Option<GitSignals> {
    let probe = crate::info::run_git_command_with_timeout(&["rev-parse", "--git-dir"], cwd).await?;
    if !probe.status.success() {
        return None;
    }
    let since = format!("{window_days} days ago");
    let max_count = MAX_COMMITS_SCANNED.to_string();
    let args = [
        "log",
        "--since",
        since.as_str(),
        "--max-count",
        max_count.as_str(),
        "--pretty=format:%x1f%ct%x1f%s",
        "--name-only",
    ];
    let out = crate::info::run_git_command_with_timeout(&args, cwd).await?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let commits = parse_git_log_for_signals(&text);
    let now = chrono::Utc::now().timestamp();
    Some(aggregate_signals(&commits, now, window_days))
}
