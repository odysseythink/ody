pub(crate) mod code_mode;
pub(crate) mod context;
pub(crate) mod events;
pub(crate) mod exec_output_distill;
pub(crate) mod handlers;
pub(crate) mod hook_names;
pub(crate) mod hosted_spec;
pub(crate) mod lifecycle;
pub(crate) mod network_approval;
pub(crate) mod orchestrator;
pub(crate) mod parallel;
pub(crate) mod registry;
pub(crate) mod router;
pub(crate) mod runtimes;
pub(crate) mod sandboxing;
pub(crate) mod spec_plan;
pub(crate) mod tool_dispatch_trace;

use std::borrow::Cow;

use crate::session::turn_context::TurnContext;
use ody_features::Feature;
use ody_protocol::exec_output::ExecToolCallOutput;
use ody_protocol::model_metadata::ToolMode;
use ody_tools::ToolName;
use ody_utils_output_truncation::TruncationPolicy;
use ody_utils_output_truncation::distill::distill_command_output;
use ody_utils_output_truncation::formatted_truncate_text;
use ody_utils_output_truncation::truncate_text;
pub use router::ToolRouter;

// Telemetry preview limits: keep log events smaller than model budgets.
pub(crate) const TELEMETRY_PREVIEW_MAX_BYTES: usize = 2 * 1024; // 2 KiB
pub(crate) const TELEMETRY_PREVIEW_MAX_LINES: usize = 64; // lines
pub(crate) const TELEMETRY_PREVIEW_TRUNCATION_NOTICE: &str =
    "[... telemetry preview truncated ...]";

/// Legacy boundaries such as hook payloads, telemetry tags, and Responses tool
/// names still require a single flattened string. Keep comparisons and sorting
/// on `ToolName` itself; use this only when crossing those boundaries.
pub(crate) fn flat_tool_name(tool_name: &ToolName) -> Cow<'_, str> {
    match tool_name.namespace.as_deref() {
        Some(namespace) => {
            let mut name = String::with_capacity(namespace.len() + tool_name.name.len());
            name.push_str(namespace);
            name.push_str(&tool_name.name);
            Cow::Owned(name)
        }
        None => Cow::Borrowed(tool_name.name.as_str()),
    }
}

pub(crate) fn tool_user_shell_type(
    user_shell: &crate::shell::Shell,
) -> ody_tools::ToolUserShellType {
    match user_shell.shell_type {
        crate::shell::ShellType::Zsh => ody_tools::ToolUserShellType::Zsh,
        crate::shell::ShellType::Bash => ody_tools::ToolUserShellType::Bash,
        crate::shell::ShellType::PowerShell => ody_tools::ToolUserShellType::PowerShell,
        crate::shell::ShellType::Sh => ody_tools::ToolUserShellType::Sh,
        crate::shell::ShellType::Cmd => ody_tools::ToolUserShellType::Cmd,
    }
}

fn effective_tool_mode(turn_context: &TurnContext) -> ToolMode {
    turn_context.model_info.tool_mode.unwrap_or_else(|| {
        if turn_context.config.features.enabled(Feature::CodeModeOnly) {
            ToolMode::CodeModeOnly
        } else if turn_context.config.features.enabled(Feature::CodeMode) {
            ToolMode::CodeMode
        } else {
            ToolMode::Direct
        }
    })
}

/// Format the combined exec output for sending back to the model.
/// Includes exit code and duration metadata; truncates large bodies safely.
pub fn format_exec_output_for_model(
    exec_output: &ExecToolCallOutput,
    truncation_policy: TruncationPolicy,
) -> String {
    // round to 1 decimal place
    let duration_seconds = ((exec_output.duration.as_secs_f32()) * 10.0).round() / 10.0;

    let content = build_content_with_timeout(exec_output);

    let total_lines = content.lines().count();

    let formatted_output = truncate_text(&content, truncation_policy);

    let mut sections = Vec::new();

    sections.push(format!("Exit code: {}", exec_output.exit_code));
    sections.push(format!("Wall time: {duration_seconds} seconds"));
    if total_lines != formatted_output.lines().count() {
        sections.push(format!("Total output lines: {total_lines}"));
    }

    sections.push("Output:".to_string());
    sections.push(formatted_output);

    sections.join("\n")
}

pub fn format_exec_output_str(
    exec_output: &ExecToolCallOutput,
    truncation_policy: TruncationPolicy,
) -> String {
    let content = build_content_with_timeout(exec_output);

    // Truncate for model consumption before serialization.
    formatted_truncate_text(&content, truncation_policy)
}

/// Extracts exec output content and prepends a timeout message if the command timed out.
fn build_content_with_timeout(exec_output: &ExecToolCallOutput) -> String {
    if exec_output.timed_out {
        format!(
            "command timed out after {} milliseconds\n{}",
            exec_output.duration.as_millis(),
            exec_output.aggregated_output.text
        )
    } else {
        exec_output.aggregated_output.text.clone()
    }
}

/// Result of command-aware output distillation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DistilledExecOutput {
    /// Distilled body for the model, backstopped by the truncation policy.
    pub text: String,
    /// The untouched pre-distillation content, for spilling to disk so the
    /// model can read it back on demand.
    pub raw_content: String,
}

/// Attempts command-aware distillation before plain truncation.
///
/// Only engages when the content would otherwise be truncated (non-zero
/// policy budget exceeded), and only for commands the distiller recognizes.
/// Returns `None` to fall back to plain truncation.
// ody: distill 仅在"输出原本就要被截断"时介入;无 budget 限制(0)时不主动压缩。
// 升级触发条件:若要进一步省 token(未超限也压缩),在此放开 budget==0 分支并补评测。
// ody: distill 只接入 agent 发起的 shell/unified-exec 工具路径(events.rs finish);
// `!` 用户命令(tasks/user_shell.rs、user_shell_command.rs)仍走纯截断。
// 升级触发条件:用户观察到 `!` 长输出浪费上下文时,把这两条路径也接入
// maybe_distill_exec_output(spill 目录可复用同一 thread 目录)。
pub(crate) fn maybe_distill_exec_output(
    command_line: &str,
    exec_output: &ExecToolCallOutput,
    truncation_policy: TruncationPolicy,
) -> Option<DistilledExecOutput> {
    let budget = truncation_policy.byte_budget();
    if budget == 0 {
        return None;
    }
    let content = build_content_with_timeout(exec_output);
    if content.len() <= budget {
        return None;
    }
    let outcome = distill_command_output(command_line, &content)?;
    let text = if outcome.text.len() > budget {
        // The distiller deliberately keeps failures verbatim; when they alone
        // exceed the budget, fall back to plain truncation of the distilled
        // text so the policy limit still holds.
        truncate_text(&outcome.text, truncation_policy)
    } else {
        outcome.text
    };
    Some(DistilledExecOutput {
        text,
        raw_content: content,
    })
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod mod_tests;
