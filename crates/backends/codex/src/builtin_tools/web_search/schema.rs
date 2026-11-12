use std::sync::LazyLock;

use nanocode_tools::SchemaSettings;
use serde_json::{Map, Value};

use super::wire::SearchCommands;

static COMMANDS_SCHEMA: LazyLock<Value> = LazyLock::new(generate_commands_schema);

pub(crate) fn commands_schema() -> Value {
    COMMANDS_SCHEMA.clone()
}

fn generate_commands_schema() -> Value {
    let mut schema = SchemaSettings {
        inline_subschemas: true,
        option_add_null_type: false,
        ..SchemaSettings::draft2019_09()
    }
    .root_schema_for::<SearchCommands>();
    let mut tool_schema = Map::new();
    for key in [
        "properties",
        "required",
        "type",
        "additionalProperties",
        "$defs",
        "definitions",
    ] {
        if let Some(value) = schema.remove(key) {
            tool_schema.insert(key.to_owned(), value);
        }
    }
    Value::Object(tool_schema)
}
