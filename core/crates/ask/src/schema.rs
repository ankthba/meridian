//! Minimal JSON Schema validation for tool inputs.
//!
//! With `eager_input_streaming` the API forwards tool input without
//! validating it, so the client checks each parsed input against the tool's
//! schema before running anything. This covers the subset strict tool
//! schemas use: `type` (single or list), `properties`, `required`,
//! `additionalProperties`, `items`, `enum`, `const`, `anyOf`/`oneOf`,
//! `allOf`, and local `$ref` into `$defs`/`definitions`. Other keywords
//! (`description`, `format`, ...) are ignored.

use serde_json::Value;

/// `Ok(())` if `value` satisfies `schema`, else a message naming the first
/// failing location, e.g. `$.symbol: expected string`.
pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    check(schema, schema, value, "$", 0)
}

const MAX_DEPTH: usize = 64;

fn check(root: &Value, schema: &Value, value: &Value, path: &str, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("{path}: schema nesting too deep"));
    }
    let Value::Object(s) = schema else {
        // `true` accepts anything; `false` rejects everything.
        return match schema {
            Value::Bool(false) => Err(format!("{path}: not allowed")),
            _ => Ok(()),
        };
    };

    if let Some(r) = s.get("$ref").and_then(Value::as_str) {
        let target = r
            .strip_prefix('#')
            .and_then(|p| if p.is_empty() { Some(root) } else { root.pointer(p) })
            .ok_or_else(|| format!("{path}: unresolvable $ref {r}"))?;
        check(root, target, value, path, depth + 1)?;
    }

    if let Some(t) = s.get("type") {
        let types: Vec<&str> = match t {
            Value::String(x) => vec![x.as_str()],
            Value::Array(xs) => xs.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !types.is_empty() && !types.iter().any(|ty| type_matches(ty, value)) {
            return Err(format!("{path}: expected {}", types.join(" or ")));
        }
    }

    if let Some(Value::Array(options)) = s.get("enum")
        && !options.contains(value)
    {
        return Err(format!("{path}: not one of the allowed values"));
    }
    if let Some(c) = s.get("const")
        && c != value
    {
        return Err(format!("{path}: must equal {c}"));
    }

    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(subs)) = s.get(key)
            && !subs.iter().any(|sub| check(root, sub, value, path, depth + 1).is_ok())
        {
            return Err(format!("{path}: matches none of the {key} alternatives"));
        }
    }
    if let Some(Value::Array(subs)) = s.get("allOf") {
        for sub in subs {
            check(root, sub, value, path, depth + 1)?;
        }
    }

    if let Value::Object(obj) = value {
        let props = s.get("properties").and_then(Value::as_object);
        if let Some(Value::Array(req)) = s.get("required") {
            for k in req.iter().filter_map(Value::as_str) {
                if !obj.contains_key(k) {
                    return Err(format!("{path}: missing required property `{k}`"));
                }
            }
        }
        for (k, v) in obj {
            let child = format!("{path}.{k}");
            match props.and_then(|p| p.get(k)) {
                Some(sub) => check(root, sub, v, &child, depth + 1)?,
                None => match s.get("additionalProperties") {
                    Some(Value::Bool(false)) => return Err(format!("{path}: unexpected property `{k}`")),
                    Some(extra @ Value::Object(_)) => check(root, extra, v, &child, depth + 1)?,
                    _ => {}
                },
            }
        }
    }

    if let (Value::Array(items), Some(item_schema)) = (value, s.get("items")) {
        for (i, v) in items.iter().enumerate() {
            check(root, item_schema, v, &format!("{path}[{i}]"), depth + 1)?;
        }
    }
    Ok(())
}

fn type_matches(ty: &str, v: &Value) -> bool {
    match ty {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => v.is_i64() || v.is_u64() || v.as_f64().is_some_and(|f| f.is_finite() && f.fract() == 0.0),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "symbol": {"type": "string"},
                "limit": {"type": "integer"},
                "fields": {"type": "array", "items": {"type": "string", "enum": ["open", "close"]}},
                "range": {"$ref": "#/$defs/range"},
                "note": {"type": ["string", "null"]}
            },
            "required": ["symbol"],
            "additionalProperties": false,
            "$defs": {"range": {"type": "object", "properties": {"days": {"type": "integer"}},
                                "required": ["days"], "additionalProperties": false}}
        })
    }

    #[test]
    fn accepts_valid_input() {
        let v = json!({"symbol": "AAPL", "limit": 5, "fields": ["close"], "range": {"days": 30}, "note": null});
        assert_eq!(validate(&schema(), &v), Ok(()));
    }

    #[test]
    fn rejects_invalid_input() {
        let s = schema();
        assert!(validate(&s, &json!({})).unwrap_err().contains("missing required property `symbol`"));
        assert!(validate(&s, &json!({"symbol": 1})).unwrap_err().contains("$.symbol: expected string"));
        assert!(validate(&s, &json!({"symbol": "A", "x": 1})).unwrap_err().contains("unexpected property `x`"));
        assert!(validate(&s, &json!({"symbol": "A", "limit": 1.5})).is_err());
        assert!(validate(&s, &json!({"symbol": "A", "fields": ["high"]})).unwrap_err().contains("$.fields[0]"));
        assert!(validate(&s, &json!({"symbol": "A", "range": {}})).unwrap_err().contains("$.range"));
        assert!(validate(&s, &json!([1])).is_err());
    }

    #[test]
    fn any_of_and_const() {
        let s = json!({"anyOf": [{"type": "string"}, {"const": 3}]});
        assert!(validate(&s, &json!("x")).is_ok());
        assert!(validate(&s, &json!(3)).is_ok());
        assert!(validate(&s, &json!(4)).is_err());
    }
}
