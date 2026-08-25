//! Spill + assembly helpers for command-aware output distillation.
//!
//! When the distiller compresses an exec output, the pre-distillation content
//! is written to a temp file and the model-visible text points at it, so no
//! information is ever lost: the model can read the file back with its normal
//! file tools.

use ody_protocol::ThreadId;
use ody_protocol::exec_output::ExecToolCallOutput;
use std::path::Path;
use std::path::PathBuf;

pub(crate) const DISTILL_OUTPUTS_DIRNAME: &str = "ody_distill_outputs";

/// Writes the pre-distillation content under
/// `<base_dir>/<DISTILL_OUTPUTS_DIRNAME>/<thread_id>/<call_id>.log` and returns
/// the path. Best-effort: returns `None` on any IO failure so callers fall
/// back to the plain truncation path.
pub(crate) async fn spill_distill_source(
    base_dir: &Path,
    thread_id: ThreadId,
    call_id: &str,
    content: &str,
) -> Option<PathBuf> {
    let dir = base_dir
        .join(DISTILL_OUTPUTS_DIRNAME)
        .join(thread_id.to_string());
    tokio::fs::create_dir_all(&dir).await.ok()?;
    let path = dir.join(format!("{call_id}.log"));
    tokio::fs::write(&path, content).await.ok()?;
    Some(path)
}

/// Assembles the model-visible response for a distilled exec output, matching
/// the `Exit code / Wall time / Output` envelope used by plain formatting.
pub(crate) fn assemble_distilled_response(
    distilled_text: &str,
    spill_path: Option<&Path>,
    original_token_count: usize,
    exec_output: &ExecToolCallOutput,
) -> String {
    let duration_seconds = ((exec_output.duration.as_secs_f32()) * 10.0).round() / 10.0;
    let mut sections = vec![
        format!("Exit code: {}", exec_output.exit_code),
        format!("Wall time: {duration_seconds} seconds"),
        "Output:".to_string(),
        distilled_text.to_string(),
    ];
    if let Some(path) = spill_path {
        sections.push(format!(
            "[ody:distill] full output (~{original_token_count} tokens) saved to {}; \
             read it with your file tools if you need the removed details.",
            path.display()
        ));
    }
    sections.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_protocol::exec_output::StreamOutput;
    use pretty_assertions::assert_eq;
    use std::time::Duration;

    fn sample_output() -> ExecToolCallOutput {
        ExecToolCallOutput {
            exit_code: 1,
            stdout: StreamOutput::new("raw".to_string()),
            stderr: StreamOutput::new(String::new()),
            aggregated_output: StreamOutput::new("raw".to_string()),
            duration: Duration::from_millis(1234),
            timed_out: false,
        }
    }

    #[tokio::test]
    async fn spill_writes_content_and_returns_path() {
        let base = tempfile::tempdir().expect("tempdir");
        let thread_id = ThreadId::new();
        let path = spill_distill_source(base.path(), thread_id, "call-1", "full raw output")
            .await
            .expect("spill should succeed");

        assert_eq!(
            path,
            base.path()
                .join(DISTILL_OUTPUTS_DIRNAME)
                .join(thread_id.to_string())
                .join("call-1.log")
        );
        let on_disk = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(on_disk, "full raw output");
    }

    #[tokio::test]
    async fn spill_fails_open_on_unwritable_base() {
        let thread_id = ThreadId::new();
        let bogus = Path::new("/definitely/not/a/writable/dir/ody-test");
        assert_eq!(
            spill_distill_source(bogus, thread_id, "call-1", "data").await,
            None
        );
    }

    #[test]
    fn assemble_includes_envelope_and_spill_pointer() {
        let text = assemble_distilled_response(
            "[ody:distill filter=pytest ...]\nkept lines",
            Some(Path::new("/tmp/ody_distill_outputs/t/c.log")),
            1234,
            &sample_output(),
        );
        assert_eq!(
            text,
            "Exit code: 1\n\
             Wall time: 1.2 seconds\n\
             Output:\n\
             [ody:distill filter=pytest ...]\n\
             kept lines\n\
             [ody:distill] full output (~1234 tokens) saved to \
             /tmp/ody_distill_outputs/t/c.log; \
             read it with your file tools if you need the removed details."
        );
    }

    #[test]
    fn assemble_without_spill_omits_pointer() {
        let text = assemble_distilled_response("distilled", None, 10, &sample_output());
        assert_eq!(
            text,
            "Exit code: 1\nWall time: 1.2 seconds\nOutput:\ndistilled"
        );
    }
}
