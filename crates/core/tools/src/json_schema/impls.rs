use super::{
    ArrayValidation, InstanceType, Items, JsonSchema, Schema, SchemaGenerator, SchemaObject,
    StringValidation, Subschemas, Types,
};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

const UNSIGNED_MINIMUM: f64 = 0.0;
const CHARACTER_LENGTH: u32 = 1;

fn typed(kind: InstanceType) -> SchemaObject {
    SchemaObject {
        instance_type: Some(Types::One(kind)),
        ..SchemaObject::default()
    }
}

macro_rules! simple_impl {
    ($type:ty, $name:literal, $kind:ident, $format:expr) => {
        impl JsonSchema for $type {
            fn is_referenceable() -> bool {
                false
            }

            fn schema_name() -> String {
                $name.to_owned()
            }

            fn schema_id() -> Cow<'static, str> {
                Cow::Borrowed($name)
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                Schema::Object(SchemaObject {
                    format: $format,
                    ..typed(InstanceType::$kind)
                })
            }
        }
    };
}

simple_impl!(str, "String", String, None);
simple_impl!(String, "String", String, None);
simple_impl!(bool, "Boolean", Boolean, None);
simple_impl!(f32, "float", Number, Some("float".to_owned()));
simple_impl!(f64, "double", Number, Some("double".to_owned()));
simple_impl!(i8, "int8", Integer, Some("int8".to_owned()));
simple_impl!(i16, "int16", Integer, Some("int16".to_owned()));
simple_impl!(i32, "int32", Integer, Some("int32".to_owned()));
simple_impl!(i64, "int64", Integer, Some("int64".to_owned()));
simple_impl!(i128, "int128", Integer, Some("int128".to_owned()));
simple_impl!(isize, "int", Integer, Some("int".to_owned()));
simple_impl!((), "Null", Null, None);
simple_impl!(Path, "String", String, None);
simple_impl!(PathBuf, "String", String, None);

macro_rules! unsigned_impl {
    ($type:ty, $format:literal) => {
        impl JsonSchema for $type {
            fn is_referenceable() -> bool {
                false
            }

            fn schema_name() -> String {
                $format.to_owned()
            }

            fn schema_id() -> Cow<'static, str> {
                Cow::Borrowed($format)
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                let mut schema = SchemaObject {
                    format: Some($format.to_owned()),
                    ..typed(InstanceType::Integer)
                };
                schema.number().minimum = Some(UNSIGNED_MINIMUM);
                Schema::Object(schema)
            }
        }
    };
}

unsigned_impl!(u8, "uint8");
unsigned_impl!(u16, "uint16");
unsigned_impl!(u32, "uint32");
unsigned_impl!(u64, "uint64");
unsigned_impl!(u128, "uint128");
unsigned_impl!(usize, "uint");

impl JsonSchema for char {
    fn is_referenceable() -> bool {
        false
    }

    fn schema_name() -> String {
        "Character".to_owned()
    }

    fn schema_id() -> Cow<'static, str> {
        Cow::Borrowed("char")
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        Schema::Object(SchemaObject {
            string: Some(StringValidation {
                max_length: Some(CHARACTER_LENGTH),
                min_length: Some(CHARACTER_LENGTH),
            }),
            ..typed(InstanceType::String)
        })
    }
}

impl<T: JsonSchema> JsonSchema for Option<T> {
    fn is_referenceable() -> bool {
        false
    }

    fn schema_name() -> String {
        format!("Nullable_{}", T::schema_name())
    }

    fn schema_id() -> Cow<'static, str> {
        Cow::Owned(format!("Option<{}>", T::schema_id()))
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let schema = generator.subschema_for::<T>();
        if !generator.settings().option_add_null_type {
            return schema;
        }
        match schema {
            Schema::Bool(true) => Schema::Bool(true),
            Schema::Bool(false) => <()>::json_schema(generator),
            Schema::Object(mut object) => {
                if let Some(types) = &mut object.instance_type {
                    types.add_null();
                    return Schema::Object(object);
                }
                Schema::Object(SchemaObject {
                    subschemas: Some(Subschemas {
                        any_of: Some(vec![Schema::Object(object), <()>::json_schema(generator)]),
                        ..Subschemas::default()
                    }),
                    ..SchemaObject::default()
                })
            }
        }
    }

    fn non_optional_json_schema(generator: &mut SchemaGenerator) -> Schema {
        T::non_optional_json_schema(generator)
    }

    fn is_option() -> bool {
        true
    }
}

fn sequence<T: ?Sized + JsonSchema>(generator: &mut SchemaGenerator, unique: bool) -> Schema {
    Schema::Object(SchemaObject {
        array: Some(ArrayValidation {
            items: Some(Items::One(Box::new(generator.subschema_for::<T>()))),
            unique_items: unique.then_some(true),
            ..ArrayValidation::default()
        }),
        ..typed(InstanceType::Array)
    })
}

macro_rules! sequence_impl {
    ($($generics:tt)+) => {
        impl $($generics)+ {
            fn is_referenceable() -> bool {
                false
            }

            fn schema_name() -> String {
                format!("Array_of_{}", T::schema_name())
            }

            fn schema_id() -> Cow<'static, str> {
                Cow::Owned(format!("[{}]", T::schema_id()))
            }

            fn json_schema(generator: &mut SchemaGenerator) -> Schema {
                sequence::<T>(generator, false)
            }
        }
    };
}

sequence_impl!(<T: JsonSchema> JsonSchema for [T]);
sequence_impl!(<T: JsonSchema> JsonSchema for Vec<T>);
sequence_impl!(<T: JsonSchema> JsonSchema for VecDeque<T>);

macro_rules! set_impl {
    ($($generics:tt)+) => {
        impl $($generics)+ {
            fn is_referenceable() -> bool {
                false
            }

            fn schema_name() -> String {
                format!("Set_of_{}", T::schema_name())
            }

            fn schema_id() -> Cow<'static, str> {
                Cow::Owned(format!("Set<{}>", T::schema_id()))
            }

            fn json_schema(generator: &mut SchemaGenerator) -> Schema {
                sequence::<T>(generator, true)
            }
        }
    };
}

set_impl!(<T: JsonSchema> JsonSchema for BTreeSet<T>);
set_impl!(<T: JsonSchema, H> JsonSchema for HashSet<T, H>);

macro_rules! map_impl {
    ($($generics:tt)+) => {
        impl $($generics)+ {
            fn is_referenceable() -> bool {
                false
            }

            fn schema_name() -> String {
                format!("Map_of_{}", V::schema_name())
            }

            fn schema_id() -> Cow<'static, str> {
                Cow::Owned(format!("Map<{}>", V::schema_id()))
            }

            fn json_schema(generator: &mut SchemaGenerator) -> Schema {
                let mut schema = typed(InstanceType::Object);
                schema.object().additional_properties =
                    Some(Box::new(generator.subschema_for::<V>()));
                Schema::Object(schema)
            }
        }
    };
}

map_impl!(<K, V: JsonSchema> JsonSchema for BTreeMap<K, V>);
map_impl!(<K, V: JsonSchema, H> JsonSchema for HashMap<K, V, H>);

macro_rules! forward_impl {
    (($($generics:tt)+) => $target:ty) => {
        impl $($generics)+ {
            fn is_referenceable() -> bool {
                <$target>::is_referenceable()
            }

            fn schema_name() -> String {
                <$target>::schema_name()
            }

            fn schema_id() -> Cow<'static, str> {
                <$target>::schema_id()
            }

            fn json_schema(generator: &mut SchemaGenerator) -> Schema {
                <$target>::json_schema(generator)
            }

            fn non_optional_json_schema(generator: &mut SchemaGenerator) -> Schema {
                <$target>::non_optional_json_schema(generator)
            }

            fn is_option() -> bool {
                <$target>::is_option()
            }
        }
    };
}

forward_impl!((<T: ?Sized + JsonSchema> JsonSchema for &T) => T);
forward_impl!((<T: ?Sized + JsonSchema> JsonSchema for &mut T) => T);
forward_impl!((<T: ?Sized + JsonSchema> JsonSchema for Box<T>) => T);
forward_impl!((<T: ?Sized + JsonSchema> JsonSchema for Rc<T>) => T);
forward_impl!((<T: ?Sized + JsonSchema> JsonSchema for Arc<T>) => T);
forward_impl!((<T: ?Sized + JsonSchema + ToOwned> JsonSchema for Cow<'_, T>) => T);
forward_impl!((JsonSchema for serde_json::Map<String, serde_json::Value>) => BTreeMap<String, serde_json::Value>);

impl JsonSchema for serde_json::Value {
    fn is_referenceable() -> bool {
        false
    }

    fn schema_name() -> String {
        "AnyValue".to_owned()
    }

    fn schema_id() -> Cow<'static, str> {
        Cow::Borrowed("AnyValue")
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        Schema::Bool(true)
    }
}

impl JsonSchema for serde_json::Number {
    fn is_referenceable() -> bool {
        false
    }

    fn schema_name() -> String {
        "Number".to_owned()
    }

    fn schema_id() -> Cow<'static, str> {
        Cow::Borrowed("Number")
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        Schema::Object(typed(InstanceType::Number))
    }
}

macro_rules! tuple_impl {
    ($($name:ident)+) => {
        impl<$($name: JsonSchema),+> JsonSchema for ($($name,)+) {
            fn is_referenceable() -> bool {
                false
            }

            fn schema_name() -> String {
                format!("Tuple_of_{}", [$($name::schema_name()),+].join("_and_"))
            }

            fn schema_id() -> Cow<'static, str> {
                Cow::Owned(format!("({})", [$($name::schema_id()),+].join(",")))
            }

            fn json_schema(generator: &mut SchemaGenerator) -> Schema {
                super::derive::tuple(vec![$(generator.subschema_for::<$name>()),+])
            }
        }
    };
}

tuple_impl!(T0);
tuple_impl!(T0 T1);
tuple_impl!(T0 T1 T2);
tuple_impl!(T0 T1 T2 T3);
tuple_impl!(T0 T1 T2 T3 T4);
tuple_impl!(T0 T1 T2 T3 T4 T5);
tuple_impl!(T0 T1 T2 T3 T4 T5 T6);
tuple_impl!(T0 T1 T2 T3 T4 T5 T6 T7);
tuple_impl!(T0 T1 T2 T3 T4 T5 T6 T7 T8);
tuple_impl!(T0 T1 T2 T3 T4 T5 T6 T7 T8 T9);
tuple_impl!(T0 T1 T2 T3 T4 T5 T6 T7 T8 T9 T10);
tuple_impl!(T0 T1 T2 T3 T4 T5 T6 T7 T8 T9 T10 T11);
tuple_impl!(T0 T1 T2 T3 T4 T5 T6 T7 T8 T9 T10 T11 T12);
tuple_impl!(T0 T1 T2 T3 T4 T5 T6 T7 T8 T9 T10 T11 T12 T13);
tuple_impl!(T0 T1 T2 T3 T4 T5 T6 T7 T8 T9 T10 T11 T12 T13 T14);
tuple_impl!(T0 T1 T2 T3 T4 T5 T6 T7 T8 T9 T10 T11 T12 T13 T14 T15);

impl<T, const LENGTH: usize> JsonSchema for [T; LENGTH]
where
    T: JsonSchema,
{
    fn is_referenceable() -> bool {
        false
    }

    fn schema_name() -> String {
        if LENGTH == 0 {
            return "EmptyArray".to_owned();
        }
        format!("Array_size_{LENGTH}_of_{}", T::schema_name())
    }

    fn schema_id() -> Cow<'static, str> {
        if LENGTH == 0 {
            return Cow::Borrowed("[]");
        }
        Cow::Owned(format!("[{LENGTH}; {}]", T::schema_id()))
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let count = u32::try_from(LENGTH).expect("an array schema's length fits in u32");
        let items = (LENGTH != 0).then(|| Items::One(Box::new(generator.subschema_for::<T>())));
        Schema::Object(SchemaObject {
            array: Some(ArrayValidation {
                items,
                max_items: Some(count),
                min_items: (LENGTH != 0).then_some(count),
                ..ArrayValidation::default()
            }),
            ..typed(InstanceType::Array)
        })
    }
}
