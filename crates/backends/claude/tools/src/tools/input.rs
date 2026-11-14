use serde_json::{Number, Value};

pub fn validate(name: &str, schema: &Value, input: &Value) -> Result<Option<Value>, String> {
    let Some(fields) = input.as_object() else {
        return Ok(None);
    };
    let properties = schema.get("properties").and_then(Value::as_object);
    let mut coerced = fields.clone();
    let mut issues = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|key| !fields.contains_key(*key))
        .map(|key| format!("The required parameter `{key}` is missing"))
        .collect::<Vec<_>>();
    if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
        issues.extend(
            fields
                .keys()
                .filter(|key| properties.is_none_or(|properties| !properties.contains_key(*key)))
                .map(|key| format!("An unexpected parameter `{key}` was provided")),
        );
    }
    for (key, property) in properties.into_iter().flatten() {
        let (Some(value), Some(kind)) = (
            coerced.get_mut(key),
            property.get("type").and_then(Value::as_str),
        ) else {
            continue;
        };
        if matches!(kind, "number" | "integer")
            && let Some(number) = value.as_str().and_then(numeric)
        {
            *value = number;
        }
        let expected = match (kind, &*value) {
            ("string", Value::String(_))
            | ("boolean", Value::Bool(_))
            | ("array", Value::Array(_))
            | ("object", Value::Object(_))
            | ("number", Value::Number(_)) => continue,
            ("integer", Value::Number(number))
                if number.as_f64().is_some_and(|n| n.fract() == 0.0) =>
            {
                continue;
            }
            ("integer", Value::Number(_)) => "int",
            ("integer", _) => "number",
            ("string" | "boolean" | "array" | "object" | "number", _) => kind,
            _ => continue,
        };
        issues.push(format!(
            "The parameter `{key}` type is expected as `{expected}` but provided as `{}`",
            provided(value)
        ));
    }
    if issues.is_empty() {
        return Ok((coerced != *fields).then_some(Value::Object(coerced)));
    }
    Err(format!(
        "{name} failed due to the following {}:\n{}",
        if issues.len() > 1 { "issues" } else { "issue" },
        issues.join("\n")
    ))
}

fn numeric(text: &str) -> Option<Value> {
    let number = text.trim().parse::<f64>().ok().filter(|n| n.is_finite())?;
    if number.fract() == 0.0
        && let Ok(whole) = text.trim().parse::<i64>()
    {
        return Some(Value::from(whole));
    }
    Number::from_f64(number).map(Value::Number)
}

const fn provided(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
