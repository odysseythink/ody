use std::collections::HashMap;

use ody_protocol::models::{
    FunctionCallOutputBody, FunctionCallOutputContentItem, FunctionCallOutputPayload,
    ResponseInputItem,
};
use ody_tools::{ToolName, ToolPayload};

use crate::tools::flat_tool_name;

const REMINDER_TEXT_1: &str = "\n\n<system-reminder>\n\
You are repeating the exact same tool call with identical parameters. \
Please carefully analyze the previous result. If the task is not yet complete, \
try a different method or parameters instead of repeating the same call.\
\n</system-reminder>";

fn make_reminder_text2(tool_name: &str, repeat_count: usize, args: &str) -> String {
    format!(
        "\n\n<system-reminder>\n\
        You have repeatedly called the same tool with identical parameters many times.\n\
        Repeated tool call detected:\n\
        - tool: {tool_name}\n\
        - repeated_times: {repeat_count}\n\
        - arguments: {args}\n\
        The previous repeated calls did not make progress. \
        Do not call this exact same tool with the exact same arguments again.\n\
        Carefully inspect the latest tool result and choose a different next action, \
        different parameters, or finish the task if enough evidence has been gathered.\
        \n</system-reminder>"
    )
}

/// Detects repetitive tool calls within a single turn and nudges the model by
/// appending system reminders to the tool result when the same (tool, args)
/// streak crosses thresholds.
#[derive(Default)]
pub(crate) struct ToolCallDeduplicator {
    /// Keys for calls registered in the current step, in provider order.
    step_calls: Vec<String>,
    /// Map from call id to the dedup key pinned at registration time.
    call_key_by_call_id: HashMap<String, String>,
    /// Last key seen at the end of the previous step.
    consecutive_key: Option<String>,
    /// Streak length for `consecutive_key` carried over from previous steps.
    consecutive_count: usize,
}

impl ToolCallDeduplicator {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Must be called at the start of each model step.
    pub(crate) fn begin_step(&mut self) {
        self.step_calls.clear();
        self.call_key_by_call_id.clear();
    }

    /// Registers a tool call for the current step and returns the cross-step
    /// streak length ending at this call.
    pub(crate) fn register(
        &mut self,
        call_id: &str,
        tool_name: &ToolName,
        payload: &ToolPayload,
    ) -> usize {
        let key = make_key(tool_name, payload);
        self.call_key_by_call_id
            .insert(call_id.to_string(), key.clone());
        self.step_calls.push(key.clone());

        let mut last_key = self.consecutive_key.clone();
        let mut streak = self.consecutive_count;
        for k in &self.step_calls {
            if Some(k) == last_key.as_ref() {
                streak += 1;
            } else {
                last_key = Some(k.clone());
                streak = 1;
            }
        }
        streak
    }

    /// Must be called after all tool calls for the current step have resolved.
    pub(crate) fn end_step(&mut self) {
        for key in &self.step_calls {
            if Some(key) == self.consecutive_key.as_ref() {
                self.consecutive_count += 1;
            } else {
                self.consecutive_key = Some(key.clone());
                self.consecutive_count = 1;
            }
        }
    }

    /// Appends a repetition reminder to the tool result when the streak length
    /// matches the configured thresholds (3, 5, 8).
    pub(crate) fn append_reminder(
        &self,
        response: ResponseInputItem,
        tool_name: &ToolName,
        payload: &ToolPayload,
        streak: usize,
    ) -> ResponseInputItem {
        if streak == 3 {
            append_reminder_to_response(response, REMINDER_TEXT_1)
        } else if streak == 5 || streak == 8 {
            let args = payload_arguments_string(payload);
            let reminder = make_reminder_text2(&flat_tool_name(tool_name), streak, &args);
            append_reminder_to_response(response, &reminder)
        } else {
            response
        }
    }
}

fn make_key(tool_name: &ToolName, payload: &ToolPayload) -> String {
    let args = payload_arguments_string(payload);
    format!("{} {}", flat_tool_name(tool_name), args)
}

fn payload_arguments_string(payload: &ToolPayload) -> String {
    match payload {
        ToolPayload::Function { arguments } => arguments.clone(),
        ToolPayload::ToolSearch { arguments } => {
            serde_json::to_string(arguments).unwrap_or_default()
        }
        ToolPayload::Custom { input } => input.clone(),
    }
}

fn append_reminder_to_response(response: ResponseInputItem, reminder: &str) -> ResponseInputItem {
    match response {
        ResponseInputItem::FunctionCallOutput { call_id, output } => {
            ResponseInputItem::FunctionCallOutput {
                call_id,
                output: append_reminder_to_payload(output, reminder),
            }
        }
        ResponseInputItem::CustomToolCallOutput {
            call_id,
            name,
            output,
        } => ResponseInputItem::CustomToolCallOutput {
            call_id,
            name,
            output: append_reminder_to_payload(output, reminder),
        },
        _ => response,
    }
}

fn append_reminder_to_payload(
    payload: FunctionCallOutputPayload,
    reminder: &str,
) -> FunctionCallOutputPayload {
    let body = match payload.body {
        FunctionCallOutputBody::Text(mut text) => {
            text.push_str(reminder);
            FunctionCallOutputBody::Text(text)
        }
        FunctionCallOutputBody::ContentItems(mut items) => {
            if let Some(FunctionCallOutputContentItem::InputText { text }) = items.last_mut() {
                text.push_str(reminder);
            } else {
                items.push(FunctionCallOutputContentItem::InputText {
                    text: reminder.to_string(),
                });
            }
            FunctionCallOutputBody::ContentItems(items)
        }
    };
    FunctionCallOutputPayload {
        body,
        success: payload.success,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn function_response(text: &str) -> ResponseInputItem {
        ResponseInputItem::FunctionCallOutput {
            call_id: "call-1".to_string(),
            output: FunctionCallOutputPayload::from_text(text.to_string()),
        }
    }

    fn content_items_response(text: &str) -> ResponseInputItem {
        ResponseInputItem::FunctionCallOutput {
            call_id: "call-1".to_string(),
            output: FunctionCallOutputPayload {
                body: FunctionCallOutputBody::ContentItems(vec![
                    FunctionCallOutputContentItem::InputText {
                        text: text.to_string(),
                    },
                ]),
                success: Some(true),
            },
        }
    }

    fn payload(args: &str) -> ToolPayload {
        ToolPayload::Function {
            arguments: args.to_string(),
        }
    }

    fn tool(name: &str) -> ToolName {
        ToolName::plain(name)
    }

    #[test]
    fn no_reminder_for_first_two_repeats() {
        let mut dedup = ToolCallDeduplicator::new();
        let name = tool("bash");
        let args = payload("echo hi");

        assert_eq!(dedup.register("c1", &name, &args), 1);
        assert_eq!(dedup.register("c2", &name, &args), 2);

        let r1 = dedup.append_reminder(function_response("out1"), &name, &args, 1);
        let r2 = dedup.append_reminder(function_response("out2"), &name, &args, 2);

        assert!(!matches_text(&r1).contains("system-reminder"));
        assert!(!matches_text(&r2).contains("system-reminder"));
    }

    #[test]
    fn reminder_at_third_repeat() {
        let mut dedup = ToolCallDeduplicator::new();
        let name = tool("bash");
        let args = payload("echo hi");

        dedup.register("c1", &name, &args);
        dedup.register("c2", &name, &args);
        let streak = dedup.register("c3", &name, &args);
        let response = dedup.append_reminder(function_response("out"), &name, &args, streak);

        let text = matches_text(&response);
        assert!(text.contains("<system-reminder>"));
        assert!(text.contains("repeating the exact same tool call"));
    }

    #[test]
    fn detailed_reminder_at_fifth_repeat() {
        let mut dedup = ToolCallDeduplicator::new();
        let name = tool("bash");
        let args = payload("echo hi");

        for i in 1..=4 {
            dedup.register(&format!("c{i}"), &name, &args);
        }
        let streak = dedup.register("c5", &name, &args);
        let response = dedup.append_reminder(function_response("out"), &name, &args, streak);

        let text = matches_text(&response);
        assert!(text.contains("<system-reminder>"));
        assert!(text.contains("Repeated tool call detected:"));
        assert!(text.contains("tool: bash"));
        assert!(text.contains("repeated_times: 5"));
        assert!(text.contains("echo hi"));
    }

    #[test]
    fn different_args_reset_streak() {
        let mut dedup = ToolCallDeduplicator::new();
        let name = tool("bash");

        assert_eq!(dedup.register("c1", &name, &payload("echo a")), 1);
        assert_eq!(dedup.register("c2", &name, &payload("echo a")), 2);
        assert_eq!(dedup.register("c3", &name, &payload("echo b")), 1);
        assert_eq!(dedup.register("c4", &name, &payload("echo b")), 2);
    }

    #[test]
    fn cross_step_streak_continues() {
        let mut dedup = ToolCallDeduplicator::new();
        let name = tool("bash");
        let args = payload("echo hi");

        // Step 1: two identical calls.
        assert_eq!(dedup.register("c1", &name, &args), 1);
        assert_eq!(dedup.register("c2", &name, &args), 2);
        dedup.end_step();

        // Step 2: one more identical call should hit the third repeat.
        dedup.begin_step();
        assert_eq!(dedup.register("c3", &name, &args), 3);
    }

    #[test]
    fn cross_step_different_args_resets() {
        let mut dedup = ToolCallDeduplicator::new();
        let name = tool("bash");

        assert_eq!(dedup.register("c1", &name, &payload("echo a")), 1);
        assert_eq!(dedup.register("c2", &name, &payload("echo a")), 2);
        dedup.end_step();

        dedup.begin_step();
        assert_eq!(dedup.register("c3", &name, &payload("echo b")), 1);
    }

    #[test]
    fn reminder_appended_to_content_items() {
        let mut dedup = ToolCallDeduplicator::new();
        let name = tool("bash");
        let args = payload("echo hi");

        dedup.register("c1", &name, &args);
        dedup.register("c2", &name, &args);
        let streak = dedup.register("c3", &name, &args);
        let response = dedup.append_reminder(content_items_response("out"), &name, &args, streak);

        let text = matches_text(&response);
        assert!(text.contains("<system-reminder>"));
    }

    fn matches_text(item: &ResponseInputItem) -> String {
        match item {
            ResponseInputItem::FunctionCallOutput { output, .. }
            | ResponseInputItem::CustomToolCallOutput { output, .. } => {
                output.body.to_text().unwrap_or_default()
            }
            _ => String::new(),
        }
    }
}
