//! Template interpolation for Flow prompts.
//!
//! Two forms exist:
//! - `${{ name }}` / `${{ name.path }}` — resolve context bindings. `args`
//!   is pre-bound by the kernel so `${{ args.key }}` works. Inside a
//!   pipeline `each` template, the first segment `item` resolves to the
//!   current pipeline item and shadows any binding of the same name.
//! - `${item}` / `${item.path}` — single-brace form, valid only inside a
//!   pipeline `each` template (elsewhere it is an unknown binding).
//!
//! Non-string values render as compact JSON. M1 has no escape syntax: a
//! literal `${{` cannot be expressed (documented limitation; `$${{ x }}`
//! renders `$` + value).

use serde_json::Value;

use super::FlowError;

/// Rendering context: context bindings plus the current pipeline item (only
/// set when rendering a `pipeline`'s `each` template).
pub(crate) struct TemplateContext<'a> {
    pub(crate) bindings: &'a serde_json::Map<String, Value>,
    /// Current pipeline item; shadows a context binding named `item`.
    pub(crate) item: Option<&'a Value>,
}

pub(crate) fn render_template(
    template: &str,
    ctx: &TemplateContext<'_>,
    step: &str,
) -> Result<String, FlowError> {
    let mut out = String::with_capacity(template.len());
    let mut cursor = 0usize;
    while cursor < template.len() {
        let rest = &template[cursor..];
        if rest.starts_with("${{") {
            // `${{` is three bytes; skip past both opening braces.
            let after_open = &rest[3..];
            let Some(rel_end) = after_open.find("}}") else {
                return Err(FlowError::UnterminatedTemplate { step: step.to_string() });
            };
            let expr = after_open[..rel_end].trim();
            let segments = parse_expr(expr).ok_or_else(|| FlowError::InvalidExpression {
                expr: expr.to_string(),
                step: step.to_string(),
            })?;
            let root = resolve_root(&segments[0], ctx, step)?;
            let value = resolve_path(root, &segments[1..], &segments[0], step)?;
            out.push_str(&value_to_text(&value));
            // Consumed: `${{` (3) + expr (rel_end) + `}}` (2).
            cursor += 3 + rel_end + 2;
            continue;
        }
        if rest.starts_with("${") {
            let after_open = &rest[2..];
            let Some(rel_end) = after_open.find('}') else {
                return Err(FlowError::UnterminatedTemplate { step: step.to_string() });
            };
            let expr = after_open[..rel_end].trim();
            // Single-brace form is reserved for the pipeline item: `${item}`
            // or `${item.path}` (digits allowed in every path segment, so an
            // array-valued item supports `${item.0}`).
            let path: &str = match expr {
                "item" => "",
                _ => {
                    let Some(path) = expr.strip_prefix("item.") else {
                        return Err(FlowError::InvalidExpression {
                            expr: expr.to_string(),
                            step: step.to_string(),
                        });
                    };
                    if path.is_empty() {
                        return Err(FlowError::InvalidExpression {
                            expr: expr.to_string(),
                            step: step.to_string(),
                        });
                    }
                    path
                }
            };
            let Some(item) = ctx.item else {
                return Err(FlowError::UnknownBinding { name: "item".to_string(), step: step.to_string() });
            };
            let segments = parse_path(path).ok_or_else(|| FlowError::InvalidExpression {
                expr: expr.to_string(),
                step: step.to_string(),
            })?;
            let value = resolve_path(item, &segments, "item", step)?;
            out.push_str(&value_to_text(&value));
            cursor += 2 + rel_end + 1;
            continue;
        }
        let ch = rest.chars().next().expect("cursor < len");
        out.push(ch);
        cursor += ch.len_utf8();
    }
    Ok(out)
}

/// `${{ }}` expressions: dot-separated `[A-Za-z0-9_]+` segments; the first
/// segment must not start with a digit. Empty expressions are rejected.
fn parse_expr(expr: &str) -> Option<Vec<String>> {
    if expr.chars().next()?.is_ascii_digit() {
        return None;
    }
    parse_path(expr)
}

/// Validates a dotted path. Every segment must be `[A-Za-z0-9_]+` and non-empty;
/// digits are allowed in any segment (array indices). An empty path yields an
/// empty segment list (bare `${item}` / `${{ name }}` with no path).
fn parse_path(path: &str) -> Option<Vec<String>> {
    if path.is_empty() {
        return Some(Vec::new());
    }
    let segments: Vec<&str> = path.split('.').collect();
    if segments.iter().any(|seg| seg.is_empty()) {
        return None;
    }
    if !segments
        .iter()
        .all(|seg| seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    {
        return None;
    }
    Some(segments.into_iter().map(str::to_string).collect())
}

fn resolve_root<'v>(name: &str, ctx: &'v TemplateContext<'_>, step: &str) -> Result<&'v Value, FlowError> {
    // Inside an `each` template the current item shadows any binding of the
    // same name; outside item scope a binding named `item` (if the user bound
    // one) is visible, matching plain binding lookup.
    if name == "item" {
        if let Some(item) = ctx.item {
            return Ok(item);
        }
    }
    ctx.bindings.get(name).ok_or_else(|| FlowError::UnknownBinding {
        name: name.to_string(),
        step: step.to_string(),
    })
}

fn resolve_path(root: &Value, path: &[String], name: &str, step: &str) -> Result<Value, FlowError> {
    let mut current = root;
    for seg in path {
        let missing = || FlowError::MissingPath {
            name: name.to_string(),
            path: seg.clone(),
            step: step.to_string(),
        };
        current = match current {
            Value::Object(map) => map.get(seg).ok_or_else(missing)?,
            Value::Array(items) if seg.bytes().all(|b| b.is_ascii_digit()) => {
                let Ok(idx) = seg.parse::<usize>() else {
                    return Err(missing());
                };
                items.get(idx).ok_or_else(missing)?
            }
            _ => return Err(missing()),
        };
    }
    Ok(current.clone())
}

fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).expect("serializing a serde_json::Value cannot fail"),
    }
}
