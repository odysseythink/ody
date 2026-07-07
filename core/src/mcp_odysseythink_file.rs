//! Bridges Apps SDK-style `odysseythink/fileParams` metadata into Ody's MCP flow.
//!
//! Strategy:
//! - Inspect `_meta["odysseythink/fileParams"]` to discover which tool arguments are
//!   file inputs.
//! - At tool execution time, file uploads for Ody Apps tools are not supported,
//!   so the rewrite returns a clear error for any declared file parameter.
//!
//! Model-visible schema masking is owned by `ody-mcp` alongside MCP tool
//! inventory, so this module only handles the execution-time argument rewrite.

use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use serde_json::Value as JsonValue;

pub(crate) async fn rewrite_mcp_tool_arguments_for_odysseythink_files(
    sess: &Session,
    turn_context: &TurnContext,
    arguments_value: Option<JsonValue>,
    odysseythink_file_input_params: Option<&[String]>,
) -> Result<Option<JsonValue>, String> {
    let Some(odysseythink_file_input_params) = odysseythink_file_input_params else {
        return Ok(arguments_value);
    };

    let Some(arguments_value) = arguments_value else {
        return Ok(None);
    };
    let Some(arguments) = arguments_value.as_object() else {
        return Ok(Some(arguments_value));
    };
    let mut rewritten_arguments = arguments.clone();

    for field_name in odysseythink_file_input_params {
        let Some(value) = arguments.get(field_name) else {
            continue;
        };
        let Some(uploaded_value) =
            rewrite_argument_value_for_odysseythink_files(turn_context, field_name, value)
                .await?
        else {
            continue;
        };
        rewritten_arguments.insert(field_name.clone(), uploaded_value);
    }

    if rewritten_arguments == *arguments {
        return Ok(Some(arguments_value));
    }

    Ok(Some(JsonValue::Object(rewritten_arguments)))
}

async fn rewrite_argument_value_for_odysseythink_files(
    turn_context: &TurnContext,
    field_name: &str,
    value: &JsonValue,
) -> Result<Option<JsonValue>, String> {
    match value {
        JsonValue::String(file_path) => {
            let rewritten = build_uploaded_argument_value(
                turn_context,
                field_name,
                /*index*/ None,
                file_path,
            )
            .await?;
            Ok(Some(rewritten))
        }
        JsonValue::Array(values) => {
            let mut rewritten_values = Vec::with_capacity(values.len());
            for (index, item) in values.iter().enumerate() {
                let Some(file_path) = item.as_str() else {
                    return Ok(None);
                };
                let rewritten = build_uploaded_argument_value(
                    turn_context,
                    field_name,
                    Some(index),
                    file_path,
                )
                .await?;
                rewritten_values.push(rewritten);
            }
            Ok(Some(JsonValue::Array(rewritten_values)))
        }
        _ => Ok(None),
    }
}

async fn build_uploaded_argument_value(
    _turn_context: &TurnContext,
    field_name: &str,
    index: Option<usize>,
    file_path: &str,
) -> Result<JsonValue, String> {
    let location = match index {
        Some(index) => format!("`{field_name}[{index}]`"),
        None => format!("`{field_name}`"),
    };
    Err(format!(
        "Ody Apps file uploads are not supported in this release ({location}: {file_path})"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::make_session_and_context;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;

    #[tokio::test]
    async fn odysseythink_file_argument_rewrite_requires_declared_file_params() {
        let (session, turn_context) = make_session_and_context().await;
        let arguments = Some(serde_json::json!({
            "file": "/tmp/ody-smoke-file.txt"
        }));

        let rewritten = rewrite_mcp_tool_arguments_for_odysseythink_files(
            &session,
            &Arc::new(turn_context),
            arguments.clone(),
            /*odysseythink_file_input_params*/ None,
        )
        .await
        .expect("rewrite should succeed");

        assert_eq!(rewritten, arguments);
    }

    #[tokio::test]
    async fn odysseythink_file_upload_returns_unsupported() {
        let (session, turn_context) = make_session_and_context().await;
        let arguments = Some(serde_json::json!({ "file": "/tmp/ody-smoke-file.txt" }));
        let result = rewrite_mcp_tool_arguments_for_odysseythink_files(
            &session,
            &turn_context,
            arguments,
            Some(&["file".to_string()]),
        )
        .await;
        let err = result.expect_err("should return unsupported");
        assert!(err.contains("not supported"));
    }
}
