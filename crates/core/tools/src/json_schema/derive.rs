use super::{
    InstanceType, Items, JsonSchema, Schema, SchemaGenerator, SchemaObject, Subschemas, Types,
};
use serde::Serialize;
use serde_json::Value;

#[must_use]
pub fn struct_object(deny_unknown_fields: bool) -> SchemaObject {
    let mut schema = SchemaObject {
        instance_type: Some(Types::One(InstanceType::Object)),
        ..SchemaObject::default()
    };
    let object = schema.object();
    if deny_unknown_fields {
        object.additional_properties = Some(Box::new(Schema::Bool(false)));
    }
    schema
}

pub fn insert_property<T: ?Sized + JsonSchema>(
    schema: &mut SchemaObject,
    name: &str,
    has_default: bool,
    property: Schema,
) {
    let object = schema.object();
    object.properties.insert(name.to_owned(), property);
    if !has_default && !T::is_option() {
        object.required.insert(name.to_owned());
    }
}

pub fn flattened<T: ?Sized + JsonSchema>(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = T::non_optional_json_schema(generator);
    if T::is_option()
        && let Schema::Object(SchemaObject {
            object: Some(object),
            ..
        }) = &mut schema
    {
        object.required.clear();
    }
    schema
}

#[must_use]
pub fn string_enum(names: &[&str]) -> Schema {
    Schema::Object(SchemaObject {
        instance_type: Some(Types::One(InstanceType::String)),
        enum_values: Some(names.iter().map(|name| Value::from(*name)).collect()),
        ..SchemaObject::default()
    })
}

#[must_use]
pub fn externally_tagged_variant(name: &str, content: Schema) -> Schema {
    let mut schema = SchemaObject {
        instance_type: Some(Types::One(InstanceType::Object)),
        ..SchemaObject::default()
    };
    let object = schema.object();
    object.properties.insert(name.to_owned(), content);
    object.required.insert(name.to_owned());
    object.additional_properties = Some(Box::new(Schema::Bool(false)));
    Schema::Object(schema)
}

#[must_use]
pub fn internally_tagged_variant(tag: &str, name: &str, deny_unknown_fields: bool) -> Schema {
    let mut schema = SchemaObject {
        instance_type: Some(Types::One(InstanceType::Object)),
        ..SchemaObject::default()
    };
    let object = schema.object();
    object
        .properties
        .insert(tag.to_owned(), string_enum(&[name]));
    object.required.insert(tag.to_owned());
    if deny_unknown_fields {
        object.additional_properties = Some(Box::new(Schema::Bool(false)));
    }
    Schema::Object(schema)
}

#[must_use]
pub fn adjacently_tagged_variant(
    tag: &str,
    content_name: &str,
    name: &str,
    content: Option<Schema>,
    deny_unknown_fields: bool,
) -> Schema {
    let mut schema = internally_tagged_variant(tag, name, deny_unknown_fields).into_object();
    if let Some(content) = content {
        let object = schema.object();
        object.properties.insert(content_name.to_owned(), content);
        object.required.insert(content_name.to_owned());
    }
    Schema::Object(schema)
}

#[must_use]
pub fn tuple(items: Vec<Schema>) -> Schema {
    let count = u32::try_from(items.len()).expect("a tuple's arity fits in u32");
    let mut schema = SchemaObject {
        instance_type: Some(Types::One(InstanceType::Array)),
        ..SchemaObject::default()
    };
    let array = schema.array();
    array.items = Some(Items::Many(items));
    array.max_items = Some(count);
    array.min_items = Some(count);
    Schema::Object(schema)
}

#[must_use]
pub fn variants(unique_names: bool, schemas: Vec<Schema>) -> Schema {
    let subschemas = if unique_names {
        Subschemas {
            one_of: Some(schemas),
            ..Subschemas::default()
        }
    } else {
        Subschemas {
            any_of: Some(schemas),
            ..Subschemas::default()
        }
    };
    Schema::Object(SchemaObject {
        subschemas: Some(subschemas),
        ..SchemaObject::default()
    })
}

#[must_use]
pub fn with_title(schema: Schema, title: &str) -> Schema {
    let mut schema = schema.into_object();
    schema.metadata().title = Some(title.to_owned());
    Schema::Object(schema)
}

#[must_use]
pub fn with_description(schema: Schema, description: &str) -> Schema {
    let mut schema = schema.into_object();
    schema.metadata().description = Some(description.to_owned());
    Schema::Object(schema)
}

#[must_use]
pub fn deprecated(schema: Schema) -> Schema {
    let mut schema = schema.into_object();
    schema.metadata().deprecated = true;
    Schema::Object(schema)
}

#[must_use]
pub fn read_only(schema: Schema) -> Schema {
    let mut schema = schema.into_object();
    schema.metadata().read_only = true;
    Schema::Object(schema)
}

#[must_use]
pub fn write_only(schema: Schema) -> Schema {
    let mut schema = schema.into_object();
    schema.metadata().write_only = true;
    Schema::Object(schema)
}

#[must_use]
pub fn with_default(schema: Schema, default: Option<Value>) -> Schema {
    let Some(default) = default else {
        return schema;
    };
    let mut schema = schema.into_object();
    schema.metadata().default = Some(default);
    Schema::Object(schema)
}

pub fn default_value<T: ?Sized + Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("a JSON schema default should serialize to JSON")
}
