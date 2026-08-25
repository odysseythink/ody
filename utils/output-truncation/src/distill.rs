//! Command-aware output distillation.
//!
//! Recognizes high-noise command outputs (pytest, cargo, git log) and rewrites
//! them into a compact form that keeps failures, errors, and verdicts while
//! dropping progress noise. Returns `None` for unrecognized commands or small
//! outputs, in which case callers fall back to plain truncation.
//!
//! Detection is token-based: a keyword only counts when it sits in command
//! position (start of the line or right after a shell control operator), so
//! `grep -rn pytest docs/` is not mistaken for a pytest run.

use crate::approx_token_count;

/// Minimum output size before distillation is worth doing at all.
const MIN_DISTILL_LINES: usize = 40;
const MIN_DISTILL_BYTES: usize = 4 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistillFilter {
    Pytest,
    Cargo,
    GitLog,
}

impl DistillFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pytest => "pytest",
            Self::Cargo => "cargo",
            Self::GitLog => "git-log",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DistillOutcome {
    pub text: String,
    pub filter: DistillFilter,
    pub original_token_count: usize,
    pub original_line_count: usize,
}

/// Attempts command-aware distillation of `output` produced by `command_line`.
///
/// Returns `None` when the command is not recognized or the output is small
/// enough that plain truncation (or no truncation) is the better answer.
pub fn distill_command_output(command_line: &str, output: &str) -> Option<DistillOutcome> {
    if output.lines().count() < MIN_DISTILL_LINES || output.len() < MIN_DISTILL_BYTES {
        return None;
    }
    let filter = detect_filter(command_line)?;
    let body = match filter {
        DistillFilter::Pytest => distill_pytest(output),
        DistillFilter::Cargo => distill_cargo(output),
        DistillFilter::GitLog => distill_git_log(output),
    };
    let original_token_count = approx_token_count(output);
    let original_line_count = output.lines().count();
    let text = format!(
        "[ody:distill filter={} original=~{} tokens, {} lines; progress noise removed, failures kept verbatim]\n{}",
        filter.label(),
        original_token_count,
        original_line_count,
        body,
    );
    Some(DistillOutcome {
        text,
        filter,
        original_token_count,
        original_line_count,
    })
}

fn detect_filter(command_line: &str) -> Option<DistillFilter> {
    let tokens: Vec<&str> = command_line.split_whitespace().collect();
    let mut expect_command = true;
    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i];
        if matches!(token, "&&" | "||" | ";" | "|" | "|&") {
            expect_command = true;
            i += 1;
            continue;
        }
        if !expect_command {
            i += 1;
            continue;
        }
        expect_command = false;
        // ody: 只识别 pytest/cargo/git log 的直接调用及 sudo/env 前缀;`python -m pytest`
        // 等其他封装形式暂不识别(后果仅是回退到普通截断)。升级触发条件:真实会话中
        // 出现高频未识别封装形式时再扩展 wrapper 白名单。
        match token {
            "sudo" | "command" | "time" => {
                expect_command = true;
                i += 1;
            }
            "env" => {
                // Skip VAR=value pairs, then the next token is the command.
                i += 1;
                while i < tokens.len() && tokens[i].contains('=') && !tokens[i].starts_with('-') {
                    i += 1;
                }
                expect_command = true;
            }
            "pytest" => return Some(DistillFilter::Pytest),
            "cargo" => {
                if matches!(
                    tokens.get(i + 1),
                    Some(&"build" | &"test" | &"check" | &"clippy" | &"run" | &"bench" | &"doc")
                ) {
                    return Some(DistillFilter::Cargo);
                }
                i += 1;
            }
            "git" => {
                if matches!(tokens.get(i + 1), Some(&"log")) {
                    return Some(DistillFilter::GitLog);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn distill_pytest(output: &str) -> String {
    let mut kept: Vec<&str> = Vec::with_capacity(output.lines().count() / 2);
    for line in output.lines() {
        if is_pytest_progress_line(line) {
            continue;
        }
        kept.push(line);
    }
    collapse_blank_runs(&kept)
}

/// Matches pytest's per-test progress lines for non-failing outcomes, both in
/// verbose form (`tests/x.py::test_y PASSED  [ 50%]`) and dot form
/// (`....F....  [ 42%]`). Failing verdict lines are always kept.
fn is_pytest_progress_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.contains("::") {
        return trimmed.ends_with(']')
            && trimmed.contains('%')
            && (trimmed.contains(" PASSED")
                || trimmed.contains(" SKIPPED")
                || trimmed.contains(" XFAIL")
                || trimmed.contains(" XPASS"));
    }
    // Dot-form progress: only progress characters plus an optional [NN%] tail.
    let head = match trimmed.find('[') {
        Some(idx) if trimmed.ends_with(']') && trimmed[idx..].contains('%') => {
            trimmed[..idx].trim_end()
        }
        _ => trimmed,
    };
    head.len() >= 4
        && head
            .chars()
            .all(|c| matches!(c, '.' | 'F' | 's' | 'x' | 'E' | ' '))
        && head.contains('.')
}

fn distill_cargo(output: &str) -> String {
    let mut kept: Vec<&str> = Vec::with_capacity(output.lines().count() / 2);
    for line in output.lines() {
        let trimmed = line.trim_start();
        if CARGO_NOISE_PREFIXES
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
        {
            continue;
        }
        if is_cargo_passed_test_line(trimmed) {
            continue;
        }
        kept.push(line);
    }
    collapse_blank_runs(&kept)
}

const CARGO_NOISE_PREFIXES: [&str; 11] = [
    "Compiling ",
    "Downloading ",
    "Downloaded ",
    "Updating ",
    "Locking ",
    "Adding ",
    "Fresh ",
    "Finished ",
    "Running ",
    "Documenting ",
    "Checking ",
];

fn is_cargo_passed_test_line(trimmed: &str) -> bool {
    trimmed.starts_with("test ") && trimmed.ends_with(" ... ok")
}

fn distill_git_log(output: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut sha: Option<String> = None;
    let mut subject_taken = false;
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("commit ") {
            let short: String = rest.trim().chars().take(8).collect();
            sha = Some(short);
            subject_taken = false;
            continue;
        }
        if sha.is_none() {
            // Outside any commit block (e.g. `git log --oneline`): keep as-is.
            out.push(line.to_string());
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("Author:")
            || trimmed.starts_with("Date:")
            || trimmed.starts_with("Merge:")
        {
            continue;
        }
        if !subject_taken {
            subject_taken = true;
            if let Some(sha) = &sha {
                out.push(format!("{sha} {trimmed}"));
            }
        }
        // Body lines beyond the subject are dropped.
    }
    collapse_blank_runs_owned(&out)
}

fn collapse_blank_runs(lines: &[&str]) -> String {
    let mut out = String::with_capacity(lines.iter().map(|l| l.len() + 1).sum());
    let mut blank = false;
    for line in lines {
        if line.trim().is_empty() {
            if blank {
                continue;
            }
            blank = true;
        } else {
            blank = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn collapse_blank_runs_owned(lines: &[String]) -> String {
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    collapse_blank_runs(&refs)
}
