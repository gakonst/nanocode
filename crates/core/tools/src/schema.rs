use schemars::{JsonSchema, r#gen::SchemaSettings};
use serde_json::{Map, Value, json};

const ROOT: [&str; 5] = ["$schema", "title", "description", "$defs", "definitions"];
const MAPS: [&str; 3] = ["properties", "$defs", "definitions"];
const NESTED: [&str; 3] = ["items", "additionalProperties", "not"];
const BRANCHES: [&str; 3] = ["anyOf", "oneOf", "allOf"];
const NUMERIC: [&str; 2] = ["integer", "number"];

#[doc(hidden)]
#[must_use]
pub fn schema_for<T: JsonSchema>() -> Value {
    let schema = SchemaSettings::draft2019_09()
        .with(|settings| {
            settings.inline_subschemas = true;
            settings.option_add_null_type = false;
        })
        .into_generator()
        .into_root_schema_for::<T>();
    let Value::Object(mut schema) =
        serde_json::to_value(schema).expect("a schemars root schema should serialize to an object")
    else {
        unreachable!("a schemars root schema should be an object");
    };
    let mut tool_schema = Map::new();
    for key in [
        "properties",
        "required",
        "type",
        "additionalProperties",
        "$defs",
        "definitions",
        "enum",
        "const",
        "anyOf",
        "oneOf",
        "allOf",
    ] {
        if let Some(value) = schema.remove(key) {
            tool_schema.insert(key.to_owned(), value);
        }
    }
    Value::Object(tool_schema)
}

#[must_use]
pub fn plain_schema(mut schema: Value) -> Value {
    plain(&mut schema);
    if let Some(fields) = schema.as_object_mut() {
        for key in ROOT {
            fields.remove(key);
        }
    }
    schema
}

fn plain(schema: &mut Value) {
    let Some(fields) = schema.as_object_mut() else {
        return;
    };
    for key in MAPS {
        if let Some(Value::Object(children)) = fields.get_mut(key) {
            children.values_mut().for_each(plain);
        }
    }
    for key in NESTED {
        if let Some(child) = fields.get_mut(key) {
            plain(child);
        }
    }
    for key in BRANCHES {
        if let Some(Value::Array(members)) = fields.get_mut(key) {
            members.iter_mut().for_each(plain);
            members.retain(|member| *member != json!({"type": "null"}));
        }
    }
    if let Some(Value::Array(kinds)) = fields.get_mut("type") {
        kinds.retain(|kind| kind.as_str() != Some("null"));
        if let [kind] = kinds.as_slice() {
            let kind = kind.clone();
            fields.insert("type".into(), kind);
        }
    }
    if let Some(Value::Array(values)) = fields.get_mut("enum") {
        values.retain(|value| !value.is_null());
    }
    if fields
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| NUMERIC.contains(&kind))
    {
        fields.remove("format");
    }
    if fields.get("additionalProperties") == Some(&Value::Bool(true)) {
        fields.remove("additionalProperties");
    }
    if fields.get("default").is_some_and(trivial) {
        fields.remove("default");
    }
    for key in BRANCHES {
        let single = match fields.get(key) {
            Some(Value::Array(members)) if members.len() == 1 => members.first().cloned(),
            _ => None,
        };
        if let Some(Value::Object(member)) = single {
            fields.remove(key);
            for (name, value) in member {
                fields.entry(name).or_insert(value);
            }
        }
    }
    if let Some(flat) = fields.get("oneOf").and_then(tagged) {
        fields.remove("oneOf");
        fields.extend(flat);
    }
}

fn trivial(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(set) => !set,
        Value::Number(number) => number.as_u64() == Some(0) || number.as_i64() == Some(0),
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(fields) => fields.is_empty(),
    }
}

fn tagged(variants: &Value) -> Option<Map<String, Value>> {
    let variants = variants.as_array()?;
    let first = variants.first()?;
    let tag = first
        .get("properties")?
        .as_object()?
        .iter()
        .find(|(_, schema)| schema.get("const").is_some())?
        .0
        .clone();
    let mut properties = Map::new();
    let mut tags = Vec::new();
    let mut required: Option<Vec<Value>> = None;
    for variant in variants {
        let fields = variant.get("properties")?.as_object()?;
        tags.push(fields.get(&tag)?.get("const")?.clone());
        for (name, schema) in fields {
            if *name != tag {
                properties
                    .entry(name.clone())
                    .or_insert_with(|| schema.clone());
            }
        }
        let wanted = variant
            .get("required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        required = Some(match required {
            None => wanted,
            Some(mut kept) => {
                kept.retain(|name| wanted.contains(name));
                kept
            }
        });
    }
    properties.insert(tag, json!({"type": "string", "enum": tags}));
    let mut flat = Map::new();
    flat.insert("type".into(), "object".into());
    flat.insert("properties".into(), Value::Object(properties));
    flat.insert(
        "required".into(),
        Value::Array(required.unwrap_or_default()),
    );
    if let Some(closed) = first.get("additionalProperties") {
        flat.insert("additionalProperties".into(), closed.clone());
    }
    Some(flat)
}
