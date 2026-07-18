use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use serde::Deserialize;
use tokio::process::Command;

use ody_protocol::protocol::ReviewOutputEvent;
use ody_protocol::user_input::UserInput;
use ody_tools::ToolName;
use ody_tools::ToolSpec;
use regex::Regex;

use crate::function_tool::FunctionCallError;
use crate::tasks::SessionTaskContext;
use crate::tasks::run_one_shot_review;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::review_tests_spec::create_review_tests_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

const TEST_FILE_RE: &str = r"\.(test|spec)\.([cm]?[jt]sx?)$";
const SOURCE_FILE_RE: &str = r"\.[cm]?[jt]sx?$";
const REVIEW_CONTENT_BUDGET_CHARS: usize = 300_000;
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewTestsArgs {
    #[serde(default)]
    project_root: Option<String>,
}

pub struct ReviewTestsHandler;

impl ToolExecutor<ToolInvocation> for ReviewTestsHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("review_tests")
    }

    fn spec(&self) -> ToolSpec {
        create_review_tests_tool()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        false
    }

    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(invocation))
    }
}

impl CoreToolRuntime for ReviewTestsHandler {}

impl ReviewTestsHandler {
    #[allow(deprecated)]
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            cancellation_token,
            payload,
            ..
        } = invocation;

        if !turn.config.test_review_enabled {
            return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                "Test review is disabled via test_review_enabled.".to_string(),
                Some(false),
            )));
        }

        let arguments = match &payload {
            ToolPayload::Function { arguments } => arguments.clone(),
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "review_tests handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: ReviewTestsArgs = parse_arguments(&arguments)?;
        let workspace_root = turn
            .environments
            .primary()
            .and_then(|env| env.cwd().to_abs_path().ok())
            .unwrap_or_else(|| turn.cwd.clone())
            .as_path()
            .to_path_buf();
        let project_root: PathBuf = args
            .project_root
            .as_deref()
            .map(|p| workspace_root.join(p))
            .unwrap_or(workspace_root);

        let reviewer_model = turn
            .config
            .test_review_model
            .clone()
            .unwrap_or_else(|| turn.model_info.slug.clone());

        let changed_files = git_changed_files(&project_root).await.map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to read git status for test review: {err}"
            ))
        })?;

        let test_files = changed_files
            .iter()
            .filter(|path| is_test_file(path))
            .cloned()
            .collect::<Vec<_>>();

        if test_files.is_empty() {
            return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                "No changed test files detected; nothing to review.".to_string(),
                Some(true),
            )));
        }

        let entries = build_review_entries(&test_files, &changed_files);
        let review_content =
            build_review_content(&project_root, &entries, REVIEW_CONTENT_BUDGET_CHARS)
                .await
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!(
                        "failed to build review content for test review: {err}"
                    ))
                })?;

        if review_content.trim().is_empty() {
            return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                "Changed test files could not be read; nothing to review.".to_string(),
                Some(true),
            )));
        }

        let session_ctx = Arc::new(SessionTaskContext::new(
            Arc::clone(&session),
            Arc::clone(&turn.extension_data),
        ));
        let prompt = build_test_review_prompt(&review_content);
        let input = vec![UserInput::Text {
            text: "Review the changed tests and implementation context above.".to_string(),
            text_elements: Vec::new(),
        }];
        let timeout = tokio::time::Duration::from_millis(DEFAULT_TIMEOUT_MS);

        let result = tokio::time::timeout(
            timeout,
            run_one_shot_review(
                session_ctx,
                turn,
                input,
                cancellation_token,
                prompt,
                reviewer_model,
            ),
        )
        .await
        .map_err(|_| FunctionCallError::RespondToModel("test review timed out".to_string()))?;

        let output_text = match result {
            Some(event) => format_review_output(event),
            None => "Test review could not be completed.".to_string(),
        };

        Ok(boxed_tool_output(FunctionToolOutput::from_text(
            output_text,
            Some(true),
        )))
    }
}

fn is_test_file(path: &str) -> bool {
    Regex::new(TEST_FILE_RE)
        .ok()
        .map(|re| re.is_match(path))
        .unwrap_or(false)
}

fn is_source_file(path: &str) -> bool {
    Regex::new(SOURCE_FILE_RE)
        .ok()
        .map(|re| re.is_match(path))
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
struct ReviewEntry {
    label: &'static str,
    path: String,
}

fn build_review_entries(test_files: &[String], changed_files: &[String]) -> Vec<ReviewEntry> {
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |label: &'static str, path: String| {
        if seen.insert(path.clone()) {
            entries.push(ReviewEntry { label, path });
        }
    };
    let re = Regex::new(TEST_FILE_RE).expect("TEST_FILE_RE is a valid regex");
    for t in test_files {
        push("TEST FILE", t.clone());
        let sibling = re.replace(t, ".$2").to_string();
        if sibling != *t {
            push("IMPLEMENTATION FILE", sibling);
        }
    }
    for f in changed_files {
        if !is_test_file(f) && is_source_file(f) {
            push("IMPLEMENTATION FILE", f.clone());
        }
    }
    entries
}

async fn build_review_content(
    project_root: &Path,
    entries: &[ReviewEntry],
    budget: usize,
) -> Result<String, std::io::Error> {
    let mut sections = Vec::new();
    let mut total = 0usize;
    let mut omitted = 0usize;
    for entry in entries {
        if total >= budget {
            omitted += 1;
            continue;
        }
        let path = project_root.join(&entry.path);
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(text) => text,
            Err(_) => continue,
        };
        let section = format!(
            "===== {}: {} =====\n\n{}\n",
            entry.label, entry.path, content
        );
        total += section.len();
        sections.push(section);
    }
    if omitted > 0 {
        sections.push(format!(
            "===== [truncated: {} file(s) omitted to fit the review budget] =====\n",
            omitted
        ));
    }
    Ok(sections.join("\n"))
}

async fn git_changed_files(project_root: &Path) -> Result<Vec<String>, std::io::Error> {
    let output = Command::new("git")
        .args(["status", "--short", "--no-renames"])
        .current_dir(project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut files = Vec::new();
    for line in text.lines() {
        if line.len() < 4 {
            continue;
        }
        let path_part = &line[3..];
        if let Some(arrow) = path_part.find(" -> ") {
            files.push(path_part[arrow + 4..].to_string());
        } else {
            files.push(path_part.to_string());
        }
    }
    Ok(files)
}

fn build_test_review_prompt(review_content: &str) -> String {
    format!(
        "You are an independent test-code reviewer. Review the changed tests and their implementation context below. Your job is to find weaknesses in the tests: missing coverage, vacuous assertions, mismatches between test and implementation, brittle tests, and missing edge cases.\n\nAlso suggest concrete mutation probes: one-line changes to the implementation that should cause a test to fail. If a test would stay green under the probe, the test is vacuous.\n\nReturn your response as a JSON object matching the ReviewOutputEvent schema: findings[] (each with title, body, confidence_score, priority, and optional code_location), overall_correctness, overall_explanation, overall_confidence_score.\n\n{}\n",
        review_content
    )
}

fn format_review_output(event: ReviewOutputEvent) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "# Independent test review\n\nOverall correctness: {}\nOverall confidence score: {}\nOverall explanation: {}",
        event.overall_correctness, event.overall_confidence_score, event.overall_explanation
    ));
    if event.findings.is_empty() {
        lines.push("\n**Findings:** none.".to_string());
    } else {
        lines.push(format!("\n## Findings ({})", event.findings.len()));
        for finding in &event.findings {
            lines.push(format!(
                "- **[priority {}] {}**",
                finding.priority, finding.title
            ));
            lines.push(format!("  confidence score: {}", finding.confidence_score));
            lines.push(format!("  {}", finding.body));
            if finding.code_location.absolute_file_path.as_os_str().len() > 0 {
                lines.push(format!(
                    "  _at {}:{}-{}_",
                    finding.code_location.absolute_file_path.display(),
                    finding.code_location.line_range.start,
                    finding.code_location.line_range.end
                ));
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_test_and_spec_files() {
        assert!(is_test_file("src/foo.test.ts"));
        assert!(is_test_file("src/foo.spec.tsx"));
        assert!(is_test_file("src/foo.test.mjs"));
        assert!(is_test_file("lib/bar.spec.cjs"));
        assert!(!is_test_file("src/foo.ts"));
        assert!(!is_test_file("src/foo.test.ts.snap"));
        assert!(!is_test_file("test_setup.js"));
    }

    #[test]
    fn recognizes_source_files() {
        assert!(is_source_file("src/foo.ts"));
        assert!(is_source_file("src/foo.tsx"));
        assert!(is_source_file("lib/bar.mjs"));
        assert!(is_source_file("lib/baz.cjs"));
        assert!(is_source_file("src/foo.test.ts"));
        assert!(!is_source_file("README.md"));
    }

    #[test]
    fn builds_review_entries_for_changed_tests_and_sources() {
        let changed = vec![
            "src/foo.test.ts".to_string(),
            "src/foo.ts".to_string(),
            "src/bar.ts".to_string(),
            "README.md".to_string(),
        ];
        let entries = build_review_entries(&["src/foo.test.ts".to_string()], &changed);
        let labels: Vec<_> = entries.iter().map(|e| e.label).collect();
        assert_eq!(
            labels,
            vec!["TEST FILE", "IMPLEMENTATION FILE", "IMPLEMENTATION FILE"]
        );
        assert_eq!(entries[0].path, "src/foo.test.ts");
        assert_eq!(entries[1].path, "src/foo.ts");
        assert_eq!(entries[2].path, "src/bar.ts");
    }

    #[test]
    fn build_review_entries_deduplicates_sibling_and_source_files() {
        let changed = vec!["src/foo.test.ts".to_string(), "src/foo.ts".to_string()];
        let entries = build_review_entries(&["src/foo.test.ts".to_string()], &changed);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "src/foo.test.ts");
        assert_eq!(entries[1].path, "src/foo.ts");
    }
}
