//! Normalize JSON Schema tool parameters for Kimi (Moonshot).
//!
//! Kimi's tool validator rejects schemas that omit an explicit `type` on a
//! property, and does not resolve `$ref`. This module dereferences local
//! `$ref`s and infers missing `type` fields. Ported from ody-code's
//! `kosong-rs` `kimi_schema.rs`; the original `panic!` fallbacks are replaced
//! with graceful defaults so request-time conversion never aborts.

use serde_json::Map;
use serde_json::Value;
use std::collections::HashSet;

const TYPE_COMPLETION_SKIP_KEYS: &[&str] =
    &["$ref", "allOf", "anyOf", "else", "if", "not", "oneOf", "then"];

#[derive(Debug, Clone, Copy)]
enum SchemaSlotKind {
    Single,
    Array,
    Map,
    SchemaOrArray,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StructuralType {
    String,
    Object,
    Array,
}

#[derive(Debug, Clone, Copy)]
struct ChildSchemaSlot {
    key: &'static str,
    kind: SchemaSlotKind,
    parent_type: Option<StructuralType>,
}

const CHILD_SCHEMA_SLOTS: &[ChildSchemaSlot] = &[
    ChildSchemaSlot { key: "$defs", kind: SchemaSlotKind::Map, parent_type: None },
    ChildSchemaSlot { key: "definitions", kind: SchemaSlotKind::Map, parent_type: None },
    ChildSchemaSlot {
        key: "dependencies",
        kind: SchemaSlotKind::Map,
        parent_type: Some(StructuralType::Object),
    },
    ChildSchemaSlot {
        key: "dependentSchemas",
        kind: SchemaSlotKind::Map,
        parent_type: Some(StructuralType::Object),
    },
    ChildSchemaSlot {
        key: "patternProperties",
        kind: SchemaSlotKind::Map,
        parent_type: Some(StructuralType::Object),
    },
    ChildSchemaSlot {
        key: "properties",
        kind: SchemaSlotKind::Map,
        parent_type: Some(StructuralType::Object),
    },
    ChildSchemaSlot {
        key: "additionalItems",
        kind: SchemaSlotKind::Single,
        parent_type: Some(StructuralType::Array),
    },
    ChildSchemaSlot {
        key: "additionalProperties",
        kind: SchemaSlotKind::Single,
        parent_type: Some(StructuralType::Object),
    },
    ChildSchemaSlot {
        key: "contains",
        kind: SchemaSlotKind::Single,
        parent_type: Some(StructuralType::Array),
    },
    ChildSchemaSlot {
        key: "contentSchema",
        kind: SchemaSlotKind::Single,
        parent_type: Some(StructuralType::String),
    },
    ChildSchemaSlot { key: "else", kind: SchemaSlotKind::Single, parent_type: None },
    ChildSchemaSlot { key: "if", kind: SchemaSlotKind::Single, parent_type: None },
    ChildSchemaSlot { key: "not", kind: SchemaSlotKind::Single, parent_type: None },
    ChildSchemaSlot {
        key: "propertyNames",
        kind: SchemaSlotKind::Single,
        parent_type: Some(StructuralType::Object),
    },
    ChildSchemaSlot { key: "then", kind: SchemaSlotKind::Single, parent_type: None },
    ChildSchemaSlot {
        key: "unevaluatedItems",
        kind: SchemaSlotKind::Single,
        parent_type: Some(StructuralType::Array),
    },
    ChildSchemaSlot {
        key: "unevaluatedProperties",
        kind: SchemaSlotKind::Single,
        parent_type: Some(StructuralType::Object),
    },
    ChildSchemaSlot { key: "allOf", kind: SchemaSlotKind::Array, parent_type: None },
    ChildSchemaSlot { key: "anyOf", kind: SchemaSlotKind::Array, parent_type: None },
    ChildSchemaSlot { key: "oneOf", kind: SchemaSlotKind::Array, parent_type: None },
    ChildSchemaSlot {
        key: "prefixItems",
        kind: SchemaSlotKind::Array,
        parent_type: Some(StructuralType::Array),
    },
    ChildSchemaSlot {
        key: "items",
        kind: SchemaSlotKind::SchemaOrArray,
        parent_type: Some(StructuralType::Array),
    },
];

fn child_schema_keys_for_parent_type(parent_type: StructuralType) -> Vec<&'static str> {
    CHILD_SCHEMA_SLOTS
        .iter()
        .filter_map(|slot| (slot.parent_type == Some(parent_type)).then_some(slot.key))
        .collect()
}

fn object_structure_keys() -> HashSet<&'static str> {
    let mut set: HashSet<&'static str> = child_schema_keys_for_parent_type(StructuralType::Object)
        .into_iter()
        .collect();
    set.extend(["dependentRequired", "maxProperties", "minProperties", "required"]);
    set
}

fn array_structure_keys() -> HashSet<&'static str> {
    let mut set: HashSet<&'static str> = child_schema_keys_for_parent_type(StructuralType::Array)
        .into_iter()
        .collect();
    set.extend(["maxContains", "maxItems", "minContains", "minItems", "uniqueItems"]);
    set
}

fn string_structure_keys() -> HashSet<&'static str> {
    let mut set: HashSet<&'static str> = child_schema_keys_for_parent_type(StructuralType::String)
        .into_iter()
        .collect();
    set.extend(["contentEncoding", "contentMediaType", "format", "maxLength", "minLength", "pattern"]);
    set
}

fn numeric_structure_keys() -> HashSet<&'static str> {
    ["exclusiveMaximum", "exclusiveMinimum", "maximum", "minimum", "multipleOf"]
        .into_iter()
        .collect()
}

/// Normalize a JSON Schema object for Kimi tool validation.
pub fn normalize_kimi_tool_schema(schema: Map<String, Value>) -> Map<String, Value> {
    ensure_kimi_property_types(deref_json_schema(schema))
}

fn deref_json_schema(schema: Map<String, Value>) -> Map<String, Value> {
    let mut visited = HashSet::new();
    let result = resolve_node(Value::Object(schema.clone()), &schema, &mut visited);
    let mut result = match result {
        Value::Object(m) => m,
        _ => schema,
    };
    if !has_unresolved_definition_ref_in_obj(&result, "$defs") {
        result.remove("$defs");
    }
    if !has_unresolved_definition_ref_in_obj(&result, "definitions") {
        result.remove("definitions");
    }
    result
}

fn has_unresolved_definition_ref(node: &Value, bucket_key: &str) -> bool {
    match node {
        Value::Array(arr) => arr.iter().any(|c| has_unresolved_definition_ref(c, bucket_key)),
        Value::Object(obj) => has_unresolved_definition_ref_in_obj(obj, bucket_key),
        _ => false,
    }
}

fn has_unresolved_definition_ref_in_obj(obj: &Map<String, Value>, bucket_key: &str) -> bool {
    if let Some(Value::String(r)) = obj.get("$ref")
        && r.starts_with(&format!("#/{bucket_key}/"))
    {
        return true;
    }
    obj.iter().any(|(k, v)| {
        if k == bucket_key {
            return false;
        }
        has_unresolved_definition_ref(v, bucket_key)
    })
}

fn resolve_node(node: Value, root: &Map<String, Value>, visited: &mut HashSet<String>) -> Value {
    match node {
        Value::Array(arr) => {
            Value::Array(arr.into_iter().map(|item| resolve_node(item, root, visited)).collect())
        }
        Value::Object(obj) => {
            if let Some(Value::String(ref_key)) = obj.get("$ref").cloned() {
                if is_local_json_pointer_ref(&ref_key) {
                    if visited.contains(&ref_key) {
                        return Value::Object(obj);
                    }
                    if let Some(resolved) = resolve_local_json_pointer(root, &ref_key) {
                        visited.insert(ref_key.clone());
                        let resolved = resolve_node(resolved, root, visited);
                        visited.remove(&ref_key);
                        // If the resolved result still contains unresolved $ref
                        // pointers, keep the original $ref intact (circular ref).
                        if has_unresolved_definition_ref(&resolved, "$defs")
                            || has_unresolved_definition_ref(&resolved, "definitions")
                        {
                            return Value::Object(obj);
                        }
                        if let Value::Object(mut resolved_obj) = resolved {
                            for (k, v) in obj {
                                if k == "$ref" {
                                    continue;
                                }
                                resolved_obj.insert(k, resolve_node(v, root, visited));
                            }
                            return Value::Object(resolved_obj);
                        }
                        return resolved;
                    }
                }
                return Value::Object(obj);
            }
            let resolved: Map<String, Value> =
                obj.into_iter().map(|(k, v)| (k, resolve_node(v, root, visited))).collect();
            Value::Object(resolved)
        }
        other => other,
    }
}

fn is_local_json_pointer_ref(r: &str) -> bool {
    r == "#" || r.starts_with("#/")
}

fn resolve_local_json_pointer(root: &Map<String, Value>, r: &str) -> Option<Value> {
    if r == "#" {
        return Some(Value::Object(root.clone()));
    }
    let mut current: Value = Value::Object(root.clone());
    for raw_part in r[2..].split('/') {
        let part = unescape_json_pointer_part(raw_part);
        current = match current {
            Value::Object(mut obj) => obj.remove(&part)?,
            Value::Array(arr) => {
                let idx = part.parse::<usize>().ok()?;
                arr.into_iter().nth(idx)?
            }
            _ => return None,
        };
    }
    Some(current)
}

fn unescape_json_pointer_part(part: &str) -> String {
    part.replace("~1", "/").replace("~0", "~")
}

fn ensure_kimi_property_types(schema: Map<String, Value>) -> Map<String, Value> {
    let mut normalized = Value::Object(schema);
    recurse_schema(&mut normalized);
    match normalized {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn recurse_schema(node: &mut Value) {
    if let Value::Object(obj) = node {
        visit_child_schemas(obj, normalize_property);
    }
}

fn visit_child_schemas(node: &mut Map<String, Value>, visit: fn(&mut Value)) {
    for slot in CHILD_SCHEMA_SLOTS {
        let Some(value) = node.get_mut(slot.key) else {
            continue;
        };
        match slot.kind {
            SchemaSlotKind::Single => {
                if value.is_object() {
                    visit(value);
                }
            }
            SchemaSlotKind::Array => {
                if let Value::Array(arr) = value {
                    for item in arr {
                        visit(item);
                    }
                }
            }
            SchemaSlotKind::Map => {
                if let Value::Object(obj) = value {
                    for (_, item) in obj {
                        visit(item);
                    }
                }
            }
            SchemaSlotKind::SchemaOrArray => {
                if value.is_object() {
                    visit(value);
                } else if let Value::Array(arr) = value {
                    for item in arr {
                        visit(item);
                    }
                }
            }
        }
    }
}

fn normalize_property(node: &mut Value) {
    let Some(obj) = node.as_object_mut() else {
        return;
    };
    if !obj.contains_key("type") && !has_any_key(obj, TYPE_COMPLETION_SKIP_KEYS) {
        let inferred = if let Some(Value::Array(values)) = obj.get("enum") {
            if !values.is_empty() {
                infer_type_from_values(values)
            } else {
                infer_type_from_structure(obj)
            }
        } else if let Some(v) = obj.get("const") {
            infer_type_from_values(std::slice::from_ref(v))
        } else {
            infer_type_from_structure(obj)
        };
        obj.insert("type".into(), Value::String(inferred));
    }
    recurse_schema(node);
}

fn has_any_key(obj: &Map<String, Value>, keys: &[&str]) -> bool {
    keys.iter().any(|k| obj.contains_key(*k))
}

fn infer_type_from_structure(obj: &Map<String, Value>) -> String {
    if has_any_key(obj, &object_structure_keys().into_iter().collect::<Vec<_>>()) {
        return "object".into();
    }
    if has_any_key(obj, &array_structure_keys().into_iter().collect::<Vec<_>>()) {
        return "array".into();
    }
    if has_any_key(obj, &string_structure_keys().into_iter().collect::<Vec<_>>()) {
        return "string".into();
    }
    if has_any_key(obj, &numeric_structure_keys().into_iter().collect::<Vec<_>>()) {
        return "number".into();
    }
    "string".into()
}

fn infer_type_from_values(values: &[Value]) -> String {
    let mut inferred: HashSet<String> = HashSet::new();
    for v in values {
        if let Some(t) = infer_value_type(v) {
            inferred.insert(t);
        } else {
            // Non-JSON value: fall back to a permissive string type.
            return "string".into();
        }
    }
    let normalized = normalize_inferred_types(inferred);
    // Mixed/empty enum types fall back to string rather than aborting.
    normalized.into_iter().next().unwrap_or_else(|| "string".into())
}

fn infer_value_type(value: &Value) -> Option<String> {
    match value {
        Value::Null => Some("null".into()),
        Value::Array(_) => Some("array".into()),
        Value::String(_) => Some("string".into()),
        Value::Number(n) => Some(if n.is_i64() || n.is_u64() {
            "integer".into()
        } else {
            "number".into()
        }),
        Value::Bool(_) => Some("boolean".into()),
        Value::Object(_) => Some("object".into()),
    }
}

fn normalize_inferred_types(types: HashSet<String>) -> Vec<String> {
    let mut normalized = types;
    if normalized.contains("number") {
        normalized.remove("integer");
    }
    let order = ["string", "number", "integer", "boolean", "object", "array", "null"];
    order
        .into_iter()
        .filter(|t| normalized.contains(*t))
        .map(str::to_string)
        .collect()
}
