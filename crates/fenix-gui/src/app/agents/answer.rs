//! A structured answer to an extension's ask (`answer_as`): the JSON
//! block found in what the agent said, and held to the schema the
//! extension gave. A small JSON Schema: `type`, `properties`,
//! `required`, `items`, `enum`, `additionalProperties: false`,
//! `minLength`, `maxLength` -- enough for a draft's `{title, body}`.

use serde_json::Value;

/// The JSON value in an answer: a fenced block (```json, or ```) that
/// parses, else the first object or array in the text that does.
pub(crate) fn find_json(text: &str) -> Option<Value> {
    let mut rest = text;
    while let Some(open) = rest.find("```") {
        let after = &rest[open + 3..];
        let body_start = after.find('\n').map_or(after.len(), |i| i + 1);
        let lang = after[..body_start].trim();
        let body = &after[body_start..];
        let Some(close) = body.find("```") else { break };
        if lang.is_empty() || lang.eq_ignore_ascii_case("json") {
            if let Ok(v) = serde_json::from_str::<Value>(body[..close].trim()) {
                if v.is_object() || v.is_array() {
                    return Some(v);
                }
            }
        }
        rest = &body[close + 3..];
    }
    // Outside code blocks of other languages: `fn x() {}` is no answer.
    let mut outside = String::new();
    let mut inside = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            inside = !inside;
            continue;
        }
        if !inside {
            outside.push_str(line);
            outside.push('\n');
        }
    }
    let text = outside.as_str();
    for (at, c) in text.char_indices() {
        if c == '{' || c == '[' {
            let mut values = serde_json::Deserializer::from_str(&text[at..]).into_iter::<Value>();
            if let Some(Ok(v)) = values.next() {
                return Some(v);
            }
        }
    }
    None
}

/// What's wrong with `value` by `schema`, a line each; none when it fits.
pub(crate) fn schema_errors(value: &Value, schema: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    check(value, schema, "$", &mut errors);
    errors
}

fn type_fits(value: &Value, ty: &str) -> bool {
    match ty {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => {
            value.as_i64().is_some()
                || value.as_u64().is_some()
                || value.as_f64().is_some_and(|f| f.fract() == 0.0)
        }
        _ => true,
    }
}

fn check(value: &Value, schema: &Value, at: &str, errors: &mut Vec<String>) {
    let Some(schema) = schema.as_object() else {
        return;
    };
    match schema.get("type") {
        Some(Value::String(ty)) if !type_fits(value, ty) => {
            errors.push(format!("{at} isn't of type {ty}"));
            return;
        }
        Some(Value::Array(types))
            if !types
                .iter()
                .filter_map(Value::as_str)
                .any(|ty| type_fits(value, ty)) =>
        {
            let names: Vec<&str> = types.iter().filter_map(Value::as_str).collect();
            errors.push(format!("{at} isn't one of {}", names.join(", ")));
            return;
        }
        _ => {}
    }
    if let Some(Value::Array(allowed)) = schema.get("enum") {
        if !allowed.contains(value) {
            errors.push(format!("{at} isn't one of the values it may be"));
        }
    }
    if let Some(text) = value.as_str() {
        let n = text.chars().count() as u64;
        if schema
            .get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|min| n < min)
        {
            errors.push(format!("{at} is too short"));
        }
        if schema
            .get("maxLength")
            .and_then(Value::as_u64)
            .is_some_and(|max| n > max)
        {
            errors.push(format!("{at} is too long"));
        }
    }
    if let Some(object) = value.as_object() {
        for name in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !object.contains_key(name) {
                errors.push(format!("{at}.{name} is missing"));
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        for (name, v) in object {
            match properties.and_then(|p| p.get(name)) {
                Some(sub) => check(v, sub, &format!("{at}.{name}"), errors),
                None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    errors.push(format!("{at}.{name} isn't expected"))
                }
                None => {}
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (i, v) in array.iter().enumerate() {
            check(v, items, &format!("{at}[{i}]"), errors);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_json_in_an_answer_is_found() {
        assert_eq!(
            find_json("Here:\n```json\n{\"title\": \"T\"}\n```\nDone."),
            Some(json!({"title": "T"}))
        );
        assert_eq!(find_json("```\n[1, 2]\n```"), Some(json!([1, 2])));
        assert_eq!(
            find_json("```rust\nfn x() {}\n```\nand {\"a\": 1} after"),
            Some(json!({"a": 1})),
            "not code that isn't JSON"
        );
        assert_eq!(
            find_json("Sure: {\"a\": {\"b\": 2}} -- that's it"),
            Some(json!({"a": {"b": 2}}))
        );
        assert_eq!(find_json("no braces {here"), None);
    }

    #[test]
    fn a_value_is_held_to_its_schema() {
        let schema = json!({"type": "object", "required": ["title", "body"], "properties": {"title": {"type": "string", "maxLength": 10}, "body": {"type": "string"}, "labels": {"type": "array", "items": {"enum": ["bug", "docs"]}}}, "additionalProperties": false});
        assert!(schema_errors(
            &json!({"title": "T", "body": "B", "labels": ["bug"]}),
            &schema
        )
        .is_empty());
        assert_eq!(
            schema_errors(&json!({"title": "T"}), &schema),
            ["$.body is missing"]
        );
        assert_eq!(
            schema_errors(&json!({"title": 3, "body": "B"}), &schema),
            ["$.title isn't of type string"]
        );
        assert_eq!(
            schema_errors(&json!({"title": "a very long title", "body": "B"}), &schema),
            ["$.title is too long"]
        );
        assert_eq!(
            schema_errors(
                &json!({"title": "T", "body": "B", "labels": ["x"]}),
                &schema
            ),
            ["$.labels[0] isn't one of the values it may be"]
        );
        assert_eq!(
            schema_errors(&json!({"title": "T", "body": "B", "extra": 1}), &schema),
            ["$.extra isn't expected"]
        );
        assert_eq!(
            schema_errors(&json!([]), &schema),
            ["$ isn't of type object"]
        );
        assert!(schema_errors(&json!(3), &json!({"type": "integer"})).is_empty());
        assert!(schema_errors(&json!(null), &json!({"type": ["string", "null"]})).is_empty());
    }
}
