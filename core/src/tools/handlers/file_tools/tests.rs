//! Behavioural tests for the file tools.
//!
//! These drive the pure `run`/`render` cores against a temp tree rather than a
//! full `ToolInvocation`, because what actually needs pinning is the *shape* of
//! the output: the whole point of these tools is that the default result is
//! cheap. A test that only checked "it compiles and returns Ok" would not catch
//! a regression back to dumping file contents.

use super::glob;
use super::grep;
use super::jq;
use super::lexically_normalize;
use super::read;
use std::path::Path;

fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/alpha.rs"),
        "fn alpha() {}\nlet needle = 1;\n// trailing\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/beta.rs"),
        "fn beta() {}\nlet needle = 2;\nlet needle = 3;\n",
    )
    .unwrap();
    std::fs::write(root.join("README.md"), "no match here\n").unwrap();
    dir
}

// ── grep ──────────────────────────────────────────────────────────────

/// The load-bearing behaviour of this entire change. A grep that returns
/// matching *lines* by default is `rg` with extra steps and costs the same
/// context; it has to return paths unless the model explicitly asks otherwise.
#[test]
fn grep_default_returns_paths_not_contents() {
    let dir = tree();
    let (out, _) = grep::run_for_test(
        "needle",
        None, // output_mode omitted → default
        dir.path(),
    )
    .expect("grep runs");

    assert!(out.contains("src/alpha.rs"), "expected the path: {out}");
    assert!(out.contains("src/beta.rs"), "expected the path: {out}");
    assert!(
        !out.contains("let needle"),
        "default grep must NOT return the matching line contents — that is the \
         expensive shape this tool exists to avoid: {out}"
    );
    assert!(
        !out.contains("README.md"),
        "non-matching file must not appear: {out}"
    );
}

#[test]
fn grep_content_mode_returns_lines_with_numbers() {
    let dir = tree();
    let (out, _) = grep::run_for_test("needle", Some("content"), dir.path()).expect("grep runs");
    assert!(
        out.contains("src/alpha.rs:2:let needle = 1;"),
        "content mode must return path:line:text — got: {out}"
    );
}

#[test]
fn grep_count_mode_counts_every_match_not_every_file() {
    let dir = tree();
    let (out, _) =
        grep::run_for_test("needle", Some("count_matches"), dir.path()).expect("grep runs");
    // alpha has 1 match, beta has 2.
    assert_eq!(
        out.trim(),
        "3",
        "count must be per-match, not per-file: {out}"
    );
}

#[test]
fn grep_rejects_an_invalid_regex() {
    let dir = tree();
    let error = grep::run_for_test("(unclosed", None, dir.path()).expect_err("must reject");
    assert!(
        format!("{error:?}").contains("not a valid regular expression"),
        "{error:?}"
    );
}

// ── read_file ─────────────────────────────────────────────────────────

#[test]
fn read_caps_at_max_lines_and_points_at_the_next_page() {
    let text = (1..=3000)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let (out, truncated) = read::render_for_test(&text, None, None).expect("renders");

    assert!(truncated, "a 3000-line file must report truncation");
    // Count only rendered body rows (`<number>\t<text>`); the trailing
    // truncation notice also mentions a line number and must not be counted.
    let body: Vec<&str> = out.lines().filter(|line| line.contains('\t')).collect();
    assert_eq!(body.len(), 1000, "must cap at MAX_LINES");
    assert!(
        out.contains("use offset=1001 to continue"),
        "must tell the model how to page on: {out}"
    );
    assert!(
        !out.contains("line 1001\n"),
        "must not leak past the cap: {out}"
    );
}

#[test]
fn read_truncates_an_overlong_line_in_place() {
    let long = "x".repeat(5000);
    let (out, _) = read::render_for_test(&long, None, None).expect("renders");
    assert!(
        out.contains("[line truncated]"),
        "{}",
        &out[..80.min(out.len())]
    );
    assert!(
        out.chars().filter(|c| *c == 'x').count() == 2000,
        "must keep exactly MAX_LINE_LENGTH characters"
    );
}

#[test]
fn read_negative_offset_reads_from_the_end() {
    let text = (1..=100)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let (out, truncated) = read::render_for_test(&text, Some(-3), None).expect("renders");
    assert!(!truncated);
    assert!(out.contains("line 98"), "{out}");
    assert!(out.contains("line 100"), "{out}");
    assert!(!out.contains("line 97"), "{out}");
}

#[test]
fn read_rejects_a_zero_offset() {
    let error = read::render_for_test("a\nb\n", Some(0), None).expect_err("must reject");
    assert!(format!("{error:?}").contains("1-based"), "{error:?}");
}

// ── glob ──────────────────────────────────────────────────────────────

#[test]
fn glob_rejects_a_pure_wildcard() {
    for pattern in ["*", "**/*", "**"] {
        let error = glob::reject_unanchored_for_test(pattern).expect_err("must reject");
        assert!(
            format!("{error:?}").contains("pure wildcard"),
            "pattern `{pattern}` must be rejected: {error:?}"
        );
    }
}

#[test]
fn glob_accepts_an_anchored_pattern() {
    for pattern in ["**/*.rs", "src/**/*.rs", "core/src/**/handlers/*.rs"] {
        glob::reject_unanchored_for_test(pattern)
            .unwrap_or_else(|err| panic!("`{pattern}` should be accepted: {err:?}"));
    }
}

#[test]
fn glob_matches_by_extension() {
    let dir = tree();
    let (out, _) = glob::run_for_test("**/*.rs", dir.path()).expect("glob runs");
    assert!(out.contains("alpha.rs"), "{out}");
    assert!(out.contains("beta.rs"), "{out}");
    assert!(!out.contains("README.md"), "{out}");
}

// ── path confinement ──────────────────────────────────────────────────

/// The confinement check normalizes `..` lexically. If it ever regresses to
/// trusting the raw path, `../../etc/passwd` walks straight out of the
/// workspace.
#[test]
fn parent_traversal_is_normalized_away() {
    let normalized = lexically_normalize(Path::new("/repo/src/../../etc/passwd"));
    assert_eq!(normalized, Path::new("/etc/passwd"));
    assert!(
        !normalized.starts_with("/repo"),
        "a traversal that escapes the root must not still look like it is inside it"
    );
}

// ── jq ────────────────────────────────────────────────────────────────

#[test]
fn jq_selects_matching_jsonl_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"response_item\",\"turn_id\":\"a\"}\n{\"type\":\"event\",\"turn_id\":\"a\"}\n{\"type\":\"response_item\",\"turn_id\":\"b\"}\n",
    )
    .unwrap();
    let (out, _) = jq::run_for_test("select(.type == \"response_item\")", &path).unwrap();
    assert!(
        out.contains(r#"{"type":"response_item","turn_id":"a"}"#),
        "{}",
        out
    );
    assert!(
        out.contains(r#"{"type":"response_item","turn_id":"b"}"#),
        "{}",
        out
    );
    assert!(
        !out.contains("\"type\":\"event\""),
        "event should be filtered out: {}",
        out
    );
}

#[test]
fn jq_extracts_nested_field() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested.json");
    std::fs::write(&path, "{\"payload\":{\"turn_id\":\"x\",\"attempts\":3}}").unwrap();
    let (out, _) = jq::run_for_test(".payload.turn_id", &path).unwrap();
    assert_eq!(out.trim(), "\"x\"", "{}", out);
}

#[test]
fn jq_maps_field_from_each_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.jsonl");
    std::fs::write(&path, "{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n").unwrap();
    let (out, _) = jq::run_for_test(".n", &path).unwrap();
    assert_eq!(out.trim(), "1\n2\n3", "{}", out);
}

#[test]
fn jq_rejects_invalid_filter() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.json");
    std::fs::write(&path, "{}").unwrap();
    let err = jq::run_for_test("select(", &path).expect_err("invalid filter must be rejected");
    assert!(
        format!("{err:?}").contains("is not valid")
            || format!("{err:?}").contains("could not be compiled"),
        "{err:?}"
    );
}

#[test]
fn jq_rejects_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.json");
    std::fs::write(&path, "not json").unwrap();
    let err = jq::run_for_test(".", &path).expect_err("invalid JSON must be rejected");
    assert!(format!("{err:?}").contains("not valid JSON"), "{err:?}");
}

#[test]
fn jq_array_mode_wraps_results() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.jsonl");
    std::fs::write(&path, "{\"n\":1}\n{\"n\":2}\n").unwrap();
    let (out, _) = jq::run_with_options_for_test(".n", &path, None, None, Some("array")).unwrap();
    assert_eq!(out.trim(), "[1,2]", "{}", out);
}

#[test]
fn jq_offset_and_limit_pages_results() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.jsonl");
    std::fs::write(&path, "{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n{\"n\":4}\n").unwrap();
    // 1-based offset: start at result 2, limit 1 -> only result 2.
    let (out, truncated) =
        jq::run_with_options_for_test(".n", &path, Some(2), Some(1), None).unwrap();

    // The output includes the pagination notice because only 1 of 4 results fit.
    assert!(out.starts_with("2"), "expected result 2 first: {out}");
    assert!(
        out.contains("showing 1 of 4"),
        "expected pagination notice: {out}"
    );
    assert!(truncated, "must signal more results exist");
}

#[test]
fn jq_reports_empty_results() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.jsonl");
    std::fs::write(&path, "{\"n\":1}\n{\"n\":2}\n").unwrap();
    let (out, _) = jq::run_for_test("select(.n > 5)", &path).unwrap();
    assert!(
        out.contains("No results"),
        "empty filter results should be reported clearly: {out}"
    );
}

#[test]
fn jq_rejects_oversized_file_with_clear_message() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.json");
    // Start a JSON object, then pad past MAX_BYTES so the truncation lands mid-value.
    let mut content = String::from(r#"{"data":"#);
    content.push_str(&"x".repeat(200 * 1024));
    std::fs::write(&path, content).unwrap();
    let err =
        jq::run_for_test(".data", &path).expect_err("oversized truncated JSON must be rejected");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("larger than") || msg.contains("100 KiB"),
        "expected size-related error, got: {msg}"
    );
}
