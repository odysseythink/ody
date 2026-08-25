use crate::distill::DistillFilter;
use crate::distill::distill_command_output;
use pretty_assertions::assert_eq;

fn pytest_output(passed_lines: usize) -> String {
    let mut out = String::from(
        "============================= test session starts ==============================\n\
         platform darwin -- Python 3.12.4, pytest-8.2.0, pluggy-1.5.0\n\
         rootdir: /repo\n\
         collected 3 items\n\
         \n",
    );
    for i in 0..passed_lines {
        out.push_str(&format!(
            "tests/test_a.py::test_ok_{i} PASSED                                  [{:3}%]\n",
            (i + 1) * 10
        ));
    }
    out.push_str(
        "tests/test_a.py::test_two FAILED                                    [100%]\n\
         \n\
         =================================== FAILURES ===================================\n\
         _________________________________ test_two _________________________________\n\
         \n\
             def test_two():\n\
         >       assert 1 == 2\n\
         E       assert 1 == 2\n\
         \n\
         tests/test_a.py:10: AssertionError\n\
         =========================== short test summary info ============================\n\
         FAILED tests/test_a.py::test_two - assert 1 == 2\n\
         ========================= 1 failed, 2 passed in 0.12s ==========================\n",
    );
    out
}

#[test]
fn unknown_command_returns_none() {
    assert_eq!(
        distill_command_output("grep foo bar.rs", &"x".repeat(20_000)),
        None
    );
}

#[test]
fn small_output_is_not_distilled() {
    let out = pytest_output(2);
    assert!(out.lines().count() < 40);
    assert_eq!(distill_command_output("pytest", &out), None);
}

#[test]
fn pytest_removes_passed_progress_lines_but_keeps_failures_and_summary() {
    let out = pytest_output(60);
    let distilled =
        distill_command_output("pytest -q", &out).expect("pytest output should distill");

    assert_eq!(distilled.filter, DistillFilter::Pytest);
    assert!(distilled.text.contains("[ody:distill filter=pytest"));
    // Failure detail and verdicts survive.
    assert!(distilled.text.contains("tests/test_a.py::test_two FAILED"));
    assert!(distilled.text.contains("E       assert 1 == 2"));
    assert!(
        distilled
            .text
            .contains("FAILED tests/test_a.py::test_two - assert 1 == 2")
    );
    assert!(distilled.text.contains("1 failed, 2 passed"));
    // Passing progress lines are gone.
    assert!(!distilled.text.contains("test_ok_0 PASSED"));
    assert!(!distilled.text.contains("test_ok_59 PASSED"));
    // Distilled output must be materially smaller than the original.
    assert!(distilled.text.len() * 2 < out.len());
}

#[test]
fn pytest_failure_only_output_stays_intact() {
    let mut out = String::new();
    for i in 0..120 {
        out.push_str(&format!(
            "FAILED tests/test_x.py::test_case_{i} - boom with some detail\n"
        ));
    }
    out.push_str("========================== 120 failed in 1.00s ===========================\n");
    let distilled = distill_command_output("pytest tests/test_x.py", &out)
        .expect("failure-heavy pytest output should still distill");
    assert!(distilled.text.contains("test_case_119"));
    assert!(distilled.text.contains("120 failed"));
}

#[test]
fn cargo_removes_build_noise_but_keeps_errors_and_test_results() {
    let mut out = String::new();
    for i in 0..140 {
        out.push_str(&format!(
            "   Compiling dep{i} v0.1.{i} (/very/long/registry/path/dep{i})\n"
        ));
    }
    out.push_str(
        "error[E0308]: mismatched types\n\
         --> src/main.rs:12:5\n\
          |\n\
         12 |     \"hello\"\n\
          |     ^^^^^^^ expected `i32`, found `&str`\n\
         \n\
         error: could not compile `myapp` (bin \"myapp\") due to 1 previous error\n",
    );
    let distilled =
        distill_command_output("cargo build", &out).expect("cargo output should distill");

    assert_eq!(distilled.filter, DistillFilter::Cargo);
    assert!(distilled.text.contains("error[E0308]: mismatched types"));
    assert!(distilled.text.contains("--> src/main.rs:12:5"));
    assert!(distilled.text.contains("could not compile"));
    assert!(!distilled.text.contains("Compiling dep40"));
    assert!(distilled.text.len() * 2 < out.len());
}

#[test]
fn cargo_test_removes_passed_test_lines_but_keeps_failures_and_summary() {
    let mut out = String::from(
        "   Compiling myapp v0.1.0 (/repo)\n    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.34s\n     Running unittests src/lib.rs (target/debug/deps/myapp-abc123)\n\nrunning 3 tests\n",
    );
    for i in 0..160 {
        out.push_str(&format!("test tests::case_{i} ... ok\n"));
    }
    out.push_str(
        "test tests::case_bad ... FAILED\n\
         \n\
         failures:\n\
         \n\
         ---- tests::case_bad stdout ----\n\
         thread 'tests::case_bad' panicked at src/lib.rs:42:9:\n\
         assertion `left == right` failed\n\
         \n\
         failures:\n\
             tests::case_bad\n\
         \n\
         test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n",
    );
    let distilled =
        distill_command_output("cargo test -p myapp", &out).expect("cargo test should distill");

    assert_eq!(distilled.filter, DistillFilter::Cargo);
    assert!(distilled.text.contains("test tests::case_bad ... FAILED"));
    assert!(distilled.text.contains("panicked at src/lib.rs:42:9"));
    assert!(
        distilled
            .text
            .contains("test result: FAILED. 2 passed; 1 failed")
    );
    assert!(!distilled.text.contains("test tests::case_30 ... ok"));
}

#[test]
fn git_log_collapses_each_commit_to_one_line() {
    let mut out = String::new();
    for i in 0..50 {
        out.push_str(&format!(
            "commit {:040x}\n\
             Author: Dev <dev@example.com>\n\
             Date:   Fri Aug 22 10:0{i}:00 2026 +0800\n\
             \n\
                 feat: add thing number {i}\n\
             \n\
                 A long body paragraph that nobody reads and that should be dropped\n\
                 by the distiller entirely.\n\
             \n",
            i
        ));
    }
    let distilled = distill_command_output("git log -50", &out).expect("git log should distill");

    assert_eq!(distilled.filter, DistillFilter::GitLog);
    assert_eq!(distilled.text.matches("commit ").count(), 0);
    assert_eq!(distilled.text.matches("feat: add thing number").count(), 50);
    assert!(!distilled.text.contains("nobody reads"));
    assert!(distilled.text.len() * 2 < out.len());
}

#[test]
fn command_detection_ignores_non_command_mentions() {
    // `grep` for the word pytest is not a pytest run.
    let out = pytest_output(60);
    assert_eq!(distill_command_output("grep -rn pytest docs/", &out), None);
}

#[test]
fn compound_command_still_detects_pytest() {
    let out = pytest_output(60);
    let distilled = distill_command_output("cd /repo && pytest tests/", &out)
        .expect("compound command should be detected");
    assert_eq!(distilled.filter, DistillFilter::Pytest);
}
