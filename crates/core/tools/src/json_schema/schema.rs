use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq)]
pub enum Schema {
    Bool(bool),
    Object(SchemaObject),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchemaObject {
    pub metadata: Option<Metadata>,
    pub instance_type: Option<Types>,
    pub format: Option<String>,
    pub enum_values: Option<Vec<Value>>,
    pub subschemas: Option<Subschemas>,
    pub number: Option<NumberValidation>,
    pub string: Option<StringValidation>,
    pub array: Option<ArrayValidation>,
    pub object: Option<ObjectValidation>,
    pub reference: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub title: Option<String>,
    pub description: Option<String>,
    pub default: Option<Value>,
    pub deprecated: bool,
    pub read_only: bool,
    pub write_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum InstanceType {
    Null,
    Boolean,
    Object,
    Array,
    Number,
    String,
    Integer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Types {
    One(InstanceType),
    Many(Vec<InstanceType>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Subschemas {
    pub all_of: Option<Vec<Schema>>,
    pub any_of: Option<Vec<Schema>>,
    pub one_of: Option<Vec<Schema>>,
    pub not: Option<Box<Schema>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NumberValidation {
    pub minimum: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StringValidation {
    pub max_length: Option<u32>,
    pub min_length: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArrayValidation {
    pub items: Option<Items>,
    pub max_items: Option<u32>,
    pub min_items: Option<u32>,
    pub unique_items: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Items {
    One(Box<Schema>),
    Many(Vec<Schema>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObjectValidation {
    pub required: BTreeSet<String>,
    pub properties: BTreeMap<String, Schema>,
    pub additional_properties: Option<Box<Schema>>,
}

impl Schema {
    #[must_use]
    pub fn new_ref(reference: String) -> Self {
        Self::Object(SchemaObject {
            reference: Some(reference),
            ..SchemaObject::default()
        })
    }

    #[must_use]
    pub fn into_object(self) -> SchemaObject {
        match self {
            Self::Object(object) => object,
            Self::Bool(true) => SchemaObject::default(),
            Self::Bool(false) => SchemaObject {
                subschemas: Some(Subschemas {
                    not: Some(Box::new(Self::Object(SchemaObject::default()))),
                    ..Subschemas::default()
                }),
                ..SchemaObject::default()
            },
        }
    }

    #[must_use]
    pub fn flatten(self, other: Self) -> Self {
        if self.is_null_type() {
            return other;
        }
        if other.is_null_type() {
            return self;
        }
        Self::Object(self.into_object().merge(other.into_object()))
    }

    const fn is_null_type(&self) -> bool {
        matches!(
            self,
            Self::Object(SchemaObject {
                instance_type: Some(Types::One(InstanceType::Null)),
                ..
            })
        )
    }

    #[must_use]
    pub fn to_value(&self) -> Value {
        match self {
            Self::Bool(value) => Value::Bool(*value),
            Self::Object(object) => {
                let mut fields = Map::new();
                object.write(&mut fields);
                Value::Object(fields)
            }
        }
    }
}

impl SchemaObject {
    pub fn metadata(&mut self) -> &mut Metadata {
        self.metadata.get_or_insert_with(Metadata::default)
    }

    pub fn subschemas(&mut self) -> &mut Subschemas {
        self.subschemas.get_or_insert_with(Subschemas::default)
    }

    pub fn number(&mut self) -> &mut NumberValidation {
        self.number.get_or_insert_with(NumberValidation::default)
    }

    pub fn string(&mut self) -> &mut StringValidation {
        self.string.get_or_insert_with(StringValidation::default)
    }

    pub fn array(&mut self) -> &mut ArrayValidation {
        self.array.get_or_insert_with(ArrayValidation::default)
    }

    pub fn object(&mut self) -> &mut ObjectValidation {
        self.object.get_or_insert_with(ObjectValidation::default)
    }

    pub fn write(&self, fields: &mut Map<String, Value>) {
        if let Some(metadata) = &self.metadata {
            metadata.write(fields);
        }
        if let Some(types) = &self.instance_type {
            fields.insert("type".to_owned(), types.to_value());
        }
        if let Some(format) = &self.format {
            fields.insert("format".to_owned(), Value::String(format.clone()));
        }
        if let Some(values) = &self.enum_values {
            fields.insert("enum".to_owned(), Value::Array(values.clone()));
        }
        if let Some(subschemas) = &self.subschemas {
            subschemas.write(fields);
        }
        if let Some(minimum) = self.number.as_ref().and_then(|number| number.minimum) {
            fields.insert("minimum".to_owned(), Value::from(minimum));
        }
        if let Some(string) = &self.string {
            string.write(fields);
        }
        if let Some(array) = &self.array {
            array.write(fields);
        }
        if let Some(object) = &self.object {
            object.write(fields);
        }
        if let Some(reference) = &self.reference {
            fields.insert("$ref".to_owned(), Value::String(reference.clone()));
        }
    }

    pub(crate) fn remove_ref_siblings(&mut self) {
        if let Some(subschemas) = &mut self.subschemas {
            let lists = [
                &mut subschemas.all_of,
                &mut subschemas.any_of,
                &mut subschemas.one_of,
            ];
            for schema in lists.into_iter().flatten().flatten() {
                schema.remove_ref_siblings();
            }
            if let Some(schema) = &mut subschemas.not {
                schema.remove_ref_siblings();
            }
        }
        if let Some(items) = self.array.as_mut().and_then(|array| array.items.as_mut()) {
            match items {
                Items::One(schema) => schema.remove_ref_siblings(),
                Items::Many(schemas) => schemas.iter_mut().for_each(Schema::remove_ref_siblings),
            }
        }
        if let Some(object) = &mut self.object {
            object
                .properties
                .values_mut()
                .for_each(Schema::remove_ref_siblings);
            if let Some(schema) = &mut object.additional_properties {
                schema.remove_ref_siblings();
            }
        }
        let Some(reference) = self.reference.take() else {
            return;
        };
        if *self == Self::default() {
            self.reference = Some(reference);
            return;
        }
        self.subschemas()
            .all_of
            .get_or_insert_with(Vec::new)
            .push(Schema::new_ref(reference));
    }

    fn merge(self, other: Self) -> Self {
        Self {
            metadata: merge_option(self.metadata, other.metadata, Metadata::merge),
            instance_type: merge_option(self.instance_type, other.instance_type, Types::merge),
            format: self.format.or(other.format),
            enum_values: merge_option(self.enum_values, other.enum_values, |mut first, second| {
                first.extend(second);
                first
            }),
            subschemas: merge_option(self.subschemas, other.subschemas, Subschemas::merge),
            number: merge_option(self.number, other.number, |first, second| {
                NumberValidation {
                    minimum: first.minimum.or(second.minimum),
                }
            }),
            string: merge_option(self.string, other.string, |first, second| {
                StringValidation {
                    max_length: first.max_length.or(second.max_length),
                    min_length: first.min_length.or(second.min_length),
                }
            }),
            array: merge_option(self.array, other.array, ArrayValidation::merge),
            object: merge_option(self.object, other.object, ObjectValidation::merge),
            reference: self.reference.or(other.reference),
        }
    }
}

impl Schema {
    fn remove_ref_siblings(&mut self) {
        if let Self::Object(object) = self {
            object.remove_ref_siblings();
        }
    }
}

impl Metadata {
    fn write(&self, fields: &mut Map<String, Value>) {
        if let Some(title) = &self.title {
            fields.insert("title".to_owned(), Value::String(title.clone()));
        }
        if let Some(description) = &self.description {
            fields.insert("description".to_owned(), Value::String(description.clone()));
        }
        if let Some(default) = &self.default {
            fields.insert("default".to_owned(), default.clone());
        }
        if self.deprecated {
            fields.insert("deprecated".to_owned(), Value::Bool(true));
        }
        if self.read_only {
            fields.insert("readOnly".to_owned(), Value::Bool(true));
        }
        if self.write_only {
            fields.insert("writeOnly".to_owned(), Value::Bool(true));
        }
    }

    fn merge(self, other: Self) -> Self {
        Self {
            title: self.title.or(other.title),
            description: self.description.or(other.description),
            default: self.default.or(other.default),
            deprecated: self.deprecated || other.deprecated,
            read_only: self.read_only || other.read_only,
            write_only: self.write_only || other.write_only,
        }
    }
}

impl InstanceType {
    const fn name(self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Boolean => "boolean",
            Self::Object => "object",
            Self::Array => "array",
            Self::Number => "number",
            Self::String => "string",
            Self::Integer => "integer",
        }
    }
}

impl Types {
    fn to_value(&self) -> Value {
        match self {
            Self::One(kind) => Value::String(kind.name().to_owned()),
            Self::Many(kinds) => Value::Array(
                kinds
                    .iter()
                    .map(|kind| Value::String(kind.name().to_owned()))
                    .collect(),
            ),
        }
    }

    pub(crate) fn add_null(&mut self) {
        match self {
            Self::One(kind) if *kind != InstanceType::Null => {
                *self = Self::Many(vec![*kind, InstanceType::Null]);
            }
            Self::Many(kinds) if !kinds.contains(&InstanceType::Null) => {
                kinds.push(InstanceType::Null);
            }
            Self::One(_) | Self::Many(_) => {}
        }
    }

    fn merge(self, other: Self) -> Self {
        if self == other {
            return self;
        }
        let mut kinds = match (self, other) {
            (Self::Many(mut first), Self::Many(second)) => {
                first.extend(second);
                first
            }
            (Self::Many(mut many), Self::One(one)) | (Self::One(one), Self::Many(mut many)) => {
                many.push(one);
                many
            }
            (Self::One(first), Self::One(second)) => vec![first, second],
        };
        kinds.sort();
        kinds.dedup();
        Self::Many(kinds)
    }
}

impl Subschemas {
    fn write(&self, fields: &mut Map<String, Value>) {
        for (name, schemas) in [
            ("allOf", &self.all_of),
            ("anyOf", &self.any_of),
            ("oneOf", &self.one_of),
        ] {
            if let Some(schemas) = schemas {
                fields.insert(
                    name.to_owned(),
                    Value::Array(schemas.iter().map(Schema::to_value).collect()),
                );
            }
        }
        if let Some(schema) = &self.not {
            fields.insert("not".to_owned(), schema.to_value());
        }
    }

    fn merge(self, other: Self) -> Self {
        Self {
            all_of: self.all_of.or(other.all_of),
            any_of: self.any_of.or(other.any_of),
            one_of: self.one_of.or(other.one_of),
            not: self.not.or(other.not),
        }
    }
}

impl StringValidation {
    fn write(&self, fields: &mut Map<String, Value>) {
        if let Some(length) = self.max_length {
            fields.insert("maxLength".to_owned(), Value::from(length));
        }
        if let Some(length) = self.min_length {
            fields.insert("minLength".to_owned(), Value::from(length));
        }
    }
}

impl ArrayValidation {
    fn write(&self, fields: &mut Map<String, Value>) {
        match &self.items {
            Some(Items::One(schema)) => {
                fields.insert("items".to_owned(), schema.to_value());
            }
            Some(Items::Many(schemas)) => {
                fields.insert(
                    "items".to_owned(),
                    Value::Array(schemas.iter().map(Schema::to_value).collect()),
                );
            }
            None => {}
        }
        if let Some(count) = self.max_items {
            fields.insert("maxItems".to_owned(), Value::from(count));
        }
        if let Some(count) = self.min_items {
            fields.insert("minItems".to_owned(), Value::from(count));
        }
        if let Some(unique) = self.unique_items {
            fields.insert("uniqueItems".to_owned(), Value::Bool(unique));
        }
    }

    fn merge(self, other: Self) -> Self {
        Self {
            items: self.items.or(other.items),
            max_items: self.max_items.or(other.max_items),
            min_items: self.min_items.or(other.min_items),
            unique_items: self.unique_items.or(other.unique_items),
        }
    }
}

impl ObjectValidation {
    fn write(&self, fields: &mut Map<String, Value>) {
        if !self.required.is_empty() {
            fields.insert(
                "required".to_owned(),
                Value::Array(self.required.iter().cloned().map(Value::String).collect()),
            );
        }
        if !self.properties.is_empty() {
            fields.insert(
                "properties".to_owned(),
                Value::Object(
                    self.properties
                        .iter()
                        .map(|(name, schema)| (name.clone(), schema.to_value()))
                        .collect(),
                ),
            );
        }
        if let Some(schema) = &self.additional_properties {
            fields.insert("additionalProperties".to_owned(), schema.to_value());
        }
    }

    fn merge(mut self, other: Self) -> Self {
        self.required.extend(other.required);
        self.properties.extend(other.properties);
        Self {
            additional_properties: merge_additional_properties(
                self.additional_properties,
                other.additional_properties,
            ),
            ..self
        }
    }
}

fn merge_additional_properties(
    first: Option<Box<Schema>>,
    second: Option<Box<Schema>>,
) -> Option<Box<Schema>> {
    match (first.map(|schema| *schema), second.map(|schema| *schema)) {
        (Some(Schema::Bool(true)), _) | (_, Some(Schema::Bool(true))) => {
            Some(Box::new(Schema::Bool(true)))
        }
        (None, _) | (_, None) => None,
        (Some(Schema::Object(first)), Some(Schema::Object(second))) => {
            Some(Box::new(Schema::Object(first.merge(second))))
        }
        (Some(object @ Schema::Object(_)), Some(Schema::Bool(false)))
        | (Some(Schema::Bool(false)), Some(object @ Schema::Object(_))) => Some(Box::new(object)),
        (Some(Schema::Bool(false)), Some(Schema::Bool(false))) => {
            Some(Box::new(Schema::Bool(false)))
        }
    }
}

fn merge_option<T>(
    first: Option<T>,
    second: Option<T>,
    merge: impl FnOnce(T, T) -> T,
) -> Option<T> {
    match (first, second) {
        (Some(first), Some(second)) => Some(merge(first, second)),
        (first, None) => first,
        (None, second) => second,
    }
}
