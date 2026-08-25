use super::maybe_distill_exec_output;
use ody_protocol::exec_output::ExecToolCallOutput;
use ody_protocol::exec_output::StreamOutput;
use ody_protocol::protocol::TruncationPolicy;
use pretty_assertions::assert_eq;
use std::time::Duration;

fn exec_output_with_text(text: String) -> ExecToolCallOutput {
    ExecToolCallOutput {
        exit_code: 0,
        stdout: StreamOutput::new(text.clone()),
        stderr: StreamOutput::new(String::new()),
        aggregated_output: StreamOutput::new(text),
        duration: Duration::from_millis(12),
        timed_out: false,
    }
}

fn big_pytest_output(passed: usize) -> String {
    let mut out = String::from("collected 101 items\n");
    for i in 0..passed {
        out.push_str(&format!(
            "tests/test_suite.py::test_passing_case_{i} PASSED                    [{:3}%]\n",
            i % 100
        ));
    }
    out.push_str(
        "tests/test_suite.py::test_broken FAILED                                [100%]\n\
         =================================== FAILURES ===================================\n\
         ______________________________ test_broken ________________________________\n\
         E       RuntimeError: boom\n\
         =========================== short test summary info ============================\n\
         FAILED tests/test_suite.py::test_broken - RuntimeError: boom\n\
         ===================== 1 failed, 100 passed in 3.21s ======================\n",
    );
    out
}

#[test]
fn under_budget_output_is_not_distilled() {
    let out = exec_output_with_text(big_pytest_output(60));
    // Generous budget: content fits, distiller must stay out of the way.
    let policy = TruncationPolicy::Bytes(1_000_000);
    assert_eq!(maybe_distill_exec_output("pytest", &out, policy), None);
}

#[test]
fn unlimited_budget_is_not_distilled() {
    let out = exec_output_with_text(big_pytest_output(200));
    // Zero budget means "no truncation configured": fail open, keep raw content.
    let policy = TruncationPolicy::Bytes(0);
    assert_eq!(maybe_distill_exec_output("pytest", &out, policy), None);
}

#[test]
fn over_budget_pytest_output_is_distilled_and_raw_is_preserved() {
    let raw = big_pytest_output(200);
    let out = exec_output_with_text(raw.clone());
    let policy = TruncationPolicy::Bytes(4_000);
    let distilled =
        maybe_distill_exec_output("pytest tests/", &out, policy).expect("should distill");

    assert!(distilled.text.contains("[ody:distill filter=pytest"));
    assert!(distilled.text.contains("RuntimeError: boom"));
    assert!(distilled.text.contains("1 failed, 100 passed"));
    assert!(!distilled.text.contains("test_passing_case_100 PASSED"));
    // Raw content is handed back verbatim for spilling.
    assert_eq!(distilled.raw_content, raw);
    assert!(distilled.text.len() < raw.len());
}

#[test]
fn over_budget_unknown_command_falls_back_to_plain_truncation() {
    let out = exec_output_with_text("line\n".repeat(5_000));
    let policy = TruncationPolicy::Bytes(4_000);
    assert_eq!(
        maybe_distill_exec_output("grep -rn foo src/", &out, policy),
        None
    );
}

#[test]
fn distilled_output_still_over_budget_is_backstopped_by_truncation() {
    // Failure-heavy output: distiller keeps everything, so it stays large.
    let mut raw = String::new();
    for i in 0..400 {
        out_line(&mut raw, i);
    }
    let out = exec_output_with_text(raw);
    let policy = TruncationPolicy::Bytes(4_000);
    let distilled = maybe_distill_exec_output("pytest", &out, policy).expect("should distill");

    assert!(
        distilled.text.len() <= 4_000 + 512,
        "backstop truncation should keep the distilled text near the budget, got {} bytes",
        distilled.text.len()
    );
}

fn out_line(buf: &mut String, i: usize) {
    buf.push_str(&format!(
        "FAILED tests/test_big.py::test_failure_number_{i} - assertion exploded loudly\n"
    ));
}

#[test]
fn timed_out_output_carries_timeout_prefix_into_raw_content() {
    let mut out = exec_output_with_text(big_pytest_output(200));
    out.timed_out = true;
    let policy = TruncationPolicy::Bytes(4_000);
    let distilled = maybe_distill_exec_output("pytest", &out, policy).expect("should distill");

    assert!(distilled.raw_content.starts_with("command timed out after"));
}
