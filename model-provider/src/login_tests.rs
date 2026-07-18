//! Tests for the OpenAI-compatible `/models` response parser used during login.

use crate::login::{LoginModelError, LoginModelInfo, parse_models_response};

#[test]
fn parse_models_response_returns_sorted_models() {
    let body = r#"{"data": [{"id": "z-model"}, {"id": "a-model"}]}"#;
    let models = parse_models_response(body).expect("parse should succeed");

    assert_eq!(
        models,
        vec![
            LoginModelInfo {
                id: "a-model".to_string(),
                display_name: "a-model".to_string(),
            },
            LoginModelInfo {
                id: "z-model".to_string(),
                display_name: "z-model".to_string(),
            },
        ]
    );
}

#[test]
fn parse_models_response_fails_on_invalid_json() {
    let err = parse_models_response("not json").expect_err("parse should fail");
    assert!(
        matches!(err, LoginModelError::InvalidResponse(_)),
        "expected InvalidResponse error, got {err:?}"
    );
}

#[test]
fn parse_models_response_fails_when_empty() {
    let err = parse_models_response(r#"{"data": []}"#).expect_err("parse should fail");
    assert!(
        matches!(err, LoginModelError::NoModels),
        "expected NoModels, got {err:?}"
    );
}

#[test]
fn parse_models_response_ignores_unknown_fields() {
    let body = r#"{"data": [{"id": "a-model", "object": "model"}], "object": "list"}"#;
    let models = parse_models_response(body).expect("parse should succeed");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "a-model");
}
