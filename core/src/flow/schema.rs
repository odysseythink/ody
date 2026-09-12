//! Minimal JSON-schema subset validator for flow agent `schema:` fields
//! (M2.4). The workspace has no `jsonschema` dependency; this hand-rolled
//! validator implements exactly the subset flow plans need and ignores
//! unknown keywords (forward-compatible with richer schemas).
//!
//! Supported keywords:
//! - `type`: `object` | `array` | `string` | `number` | `integer` |
//!   `boolean` | `null`
//! - `properties`: `{ name: <schema> }` (objects)
//! - `required`: `[name, ...]` (objects)
//! - `items`: `<schema>` (arrays)
//! - `enum`: `[value, ...]` (any type)
//!
//! Anything else is ignored. A malformed schema (e.g. `type` that is not a
//! string, `properties` that is not an object) is an error, surfaced as the
//! validation failure reason so retry feedback is actionable.

use serde_json::Map;
use serde_json::Value;

/// Validate `value` against the schema subset. Returns `Ok(())` or a
/// human-readable reason describing the first mismatch.
pub(crate) fn validate_against_schema(
    value: &Value,
    schema: &Map<String, Value>,
) -> Result<(), String> {
    validate_inner(value, schema, "$")
}

fn validate_inner(value: &Value, schema: &Map<String, Value>, path: &str) -> Result<(), String> {
    if let Some(expected) = schema.get("type") {
        let expected = expected
            .as_str()
            .ok_or_else(|| "schema keyword 'type' must be a string".to_string())?;
        check_type(value, expected).map_err(|kind| {
            format!("expected {expected} at {path}, found {kind}")
        })?;
    }

    if let Some(variants) = schema.get("enum") {
        let variants = variants
            .as_array()
            .ok_or_else(|| "schema keyword 'enum' must be an array".to_string())?;
        if !variants.iter().any(|variant| variant == value) {
            return Err(format!("value at {path} is not one of the allowed values"));
        }
    }

    if let Some(properties) = schema.get("properties") {
        let properties = properties
            .as_object()
            .ok_or_else(|| "schema keyword 'properties' must be an object".to_string())?;
        if let Some(object) = value.as_object() {
            for (name, property_schema) in properties {
                if let Some(property_value) = object.get(name) {
                    let property_schema = property_schema
                        .as_object()
                        .ok_or_else(|| {
                            format!("schema for property '{name}' must be an object")
                        })?;
                    validate_inner(
                        property_value,
                        property_schema,
                        &format!("{path}.{name}"),
                    )?;
                }
            }
        }
    }

    if let Some(required) = schema.get("required") {
        let required = required
            .as_array()
            .ok_or_else(|| "schema keyword 'required' must be an array".to_string())?;
        if let Some(object) = value.as_object() {
            for name in required {
                let name = name
                    .as_str()
                    .ok_or_else(|| "schema 'required' entries must be strings".to_string())?;
                if !object.contains_key(name) {
                    return Err(format!("missing required property '{name}' at {path}"));
                }
            }
        }
    }

    if let Some(items) = schema.get("items") {
        let items = items
            .as_object()
            .ok_or_else(|| "schema keyword 'items' must be an object".to_string())?;
        if let Some(array) = value.as_array() {
            for (index, element) in array.iter().enumerate() {
                validate_inner(element, items, &format!("{path}[{index}]"))?;
            }
        }
    }

    Ok(())
}

fn check_type(value: &Value, expected: &str) -> Result<(), &'static str> {
    let actual = match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) => {
            if number.is_f64() {
                "number"
            } else {
                "integer"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    match expected {
        "number" if actual == "integer" => Ok(()),
        _ if actual == expected => Ok(()),
        _ => Err(actual),
    }
}

#[cfg(test)]
mod schema_tests {
    use super::*;
    use serde_json::json;

    fn object(schema: Value) -> Map<String, Value> {
        schema.as_object().expect("schema should be an object").clone()
    }

    #[test]
    fn accepts_matching_object() {
        let schema = object(json!({
            "type": "object",
            "required": ["mechanics"],
            "properties": {
                "mechanics": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["name"],
                        "properties": { "name": { "type": "string" } },
                    },
                },
            },
        }));
        let value = json!({"mechanics": [{"name": "jump"}], "extra": 1});
        assert_eq!(validate_against_schema(&value, &schema), Ok(()));
    }

    #[test]
    fn rejects_wrong_type_with_path() {
        let schema = object(json!({"type": "object"}));
        let err = validate_against_schema(&json!([]), &schema).expect_err("array is not an object");
        assert!(err.contains("expected object at $"), "unexpected: {err}");
    }

    #[test]
    fn rejects_missing_required_property() {
        let schema = object(json!({"type": "object", "required": ["name"]}));
        let err =
            validate_against_schema(&json!({}), &schema).expect_err("missing required property");
        assert!(err.contains("missing required property 'name'"), "unexpected: {err}");
    }

    #[test]
    fn rejects_nested_property_mismatch() {
        let schema = object(json!({
            "type": "object",
            "properties": { "gdd": { "type": "object", "required": ["constraints"] } },
        }));
        let err = validate_against_schema(&json!({"gdd": {}}), &schema)
            .expect_err("nested property missing");
        assert!(err.contains("$.gdd"), "path should pinpoint the nested field: {err}");
    }

    #[test]
    fn integer_accepts_ints_and_rejects_floats() {
        let schema = object(json!({"type": "integer"}));
        assert_eq!(validate_against_schema(&json!(3), &schema), Ok(()));
        let err =
            validate_against_schema(&json!(3.5), &schema).expect_err("float is not an integer");
        assert!(err.contains("expected integer"), "unexpected: {err}");
    }

    #[test]
    fn number_accepts_ints_and_floats() {
        let schema = object(json!({"type": "number"}));
        assert_eq!(validate_against_schema(&json!(3), &schema), Ok(()));
        assert_eq!(validate_against_schema(&json!(3.5), &schema), Ok(()));
    }

    #[test]
    fn enum_rejects_outside_values() {
        let schema = object(json!({"enum": ["a", "b"]}));
        assert_eq!(validate_against_schema(&json!("a"), &schema), Ok(()));
        assert!(validate_against_schema(&json!("c"), &schema).is_err());
    }

    #[test]
    fn malformed_schema_is_an_error() {
        let schema = object(json!({"type": 3}));
        assert!(validate_against_schema(&json!(null), &schema).is_err());
        let schema = object(json!({"required": "name"}));
        assert!(validate_against_schema(&json!({}), &schema).is_err());
    }

    #[test]
    fn unknown_keywords_are_ignored() {
        let schema = object(json!({"description": "free-form", "minimum": 3}));
        assert_eq!(validate_against_schema(&json!(1), &schema), Ok(()));
    }
}
