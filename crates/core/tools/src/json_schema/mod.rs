mod derive;
mod generator;
mod impls;
mod schema;

pub use generator::{Draft, SchemaGenerator, SchemaSettings};
pub use schema::{
    ArrayValidation, InstanceType, Items, Metadata, NumberValidation, ObjectValidation, Schema,
    SchemaObject, StringValidation, Subschemas, Types,
};
use std::borrow::Cow;

#[doc(hidden)]
pub mod private {
    pub use super::derive::*;
    pub use super::{JsonSchema, Schema, SchemaGenerator};
}

pub trait JsonSchema {
    #[must_use]
    fn is_referenceable() -> bool {
        true
    }

    #[must_use]
    fn schema_name() -> String;

    #[must_use]
    fn schema_id() -> Cow<'static, str> {
        Cow::Owned(Self::schema_name())
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema;

    #[doc(hidden)]
    fn non_optional_json_schema(generator: &mut SchemaGenerator) -> Schema {
        Self::json_schema(generator)
    }

    #[doc(hidden)]
    #[must_use]
    fn is_option() -> bool {
        false
    }
}
