mod contract;
pub use contract::{ValidationIssue, Validator, validator_for};

mod compiler;
mod content;
mod draft;
mod ecma;
mod format_compat;
mod formats;
mod loader;
mod numbers;
mod output;
mod regex;
mod root;
mod roots;
mod unicode;
mod util;
mod validator;

pub use {
    compiler::{CompileError, Compiler, Draft},
    content::{Decoder, MediaType},
    formats::Format,
    loader::{SchemeUrlLoader, UrlLoader},
    output::{
        AbsoluteKeywordLocation, FlagOutput, KeywordPath, OutputError, OutputUnit, SchemaToken,
    },
    validator::{InstanceLocation, InstanceToken},
};

use std::{borrow::Cow, collections::HashMap, error::Error, fmt::Display};

use crate::ecma::Regex;
use serde_json::{Number, Value};
use util::*;

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SchemaIndex(usize);

#[derive(Default)]
pub struct Schemas {
    list: Vec<Schema>,
    map: HashMap<UrlPtr, usize>,
}

impl Schemas {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&mut self, locs: Vec<UrlPtr>, compiled: Vec<Schema>) {
        for (up, sch) in locs.into_iter().zip(compiled) {
            let i = self.list.len();
            self.list.push(sch);
            self.map.insert(up, i);
        }
    }

    fn get(&self, idx: SchemaIndex) -> &Schema {
        &self.list[idx.0]
    }

    fn get_by_loc(&self, up: &UrlPtr) -> Option<&Schema> {
        self.map.get(up).and_then(|&i| self.list.get(i))
    }

    pub fn contains(&self, sch_index: SchemaIndex) -> bool {
        self.list.get(sch_index.0).is_some()
    }

    pub const fn size(&self) -> usize {
        self.list.len()
    }

    pub fn is_valid(&self, value: &Value, index: SchemaIndex) -> bool {
        validator::validate(value, self.get(index), self, true).is_ok()
    }

    pub fn validate<'s, 'v>(
        &'s self,
        v: &'v Value,
        sch_index: SchemaIndex,
    ) -> Result<(), ValidationError<'s, 'v>> {
        let Some(sch) = self.list.get(sch_index.0) else {
            panic!("Schemas::validate: schema index out of bounds");
        };
        validator::validate(v, sch, self, false)
    }
}

#[derive(Default)]
struct Schema {
    draft_version: usize,
    idx: SchemaIndex,
    loc: String,
    resource: SchemaIndex,
    dynamic_anchors: HashMap<String, SchemaIndex>,
    all_props_evaluated: bool,
    all_items_evaluated: bool,
    num_items_evaluated: usize,

    boolean: Option<bool>,
    ref_: Option<SchemaIndex>,
    recursive_ref: Option<SchemaIndex>,
    recursive_anchor: bool,
    dynamic_ref: Option<DynamicRef>,
    dynamic_anchor: Option<String>,
    types: Types,
    enum_: Option<Enum>,
    constant: Option<Value>,
    not: Option<SchemaIndex>,
    all_of: Vec<SchemaIndex>,
    any_of: Vec<SchemaIndex>,
    one_of: Vec<SchemaIndex>,
    if_: Option<SchemaIndex>,
    then: Option<SchemaIndex>,
    else_: Option<SchemaIndex>,
    format: Option<Format>,

    min_properties: Option<Number>,
    max_properties: Option<Number>,
    required: Vec<String>,
    properties: HashMap<String, SchemaIndex>,
    pattern_properties: Vec<(Regex, SchemaIndex)>,
    property_names: Option<SchemaIndex>,
    additional_properties: Option<Additional>,
    dependent_required: Vec<(String, Vec<String>)>,
    dependent_schemas: Vec<(String, SchemaIndex)>,
    dependencies: Vec<(String, Dependency)>,
    unevaluated_properties: Option<SchemaIndex>,

    min_items: Option<Number>,
    max_items: Option<Number>,
    unique_items: bool,
    min_contains: Option<Number>,
    max_contains: Option<Number>,
    contains: Option<SchemaIndex>,
    items: Option<Items>,
    additional_items: Option<Additional>,
    prefix_items: Vec<SchemaIndex>,
    items2020: Option<SchemaIndex>,
    unevaluated_items: Option<SchemaIndex>,

    min_length: Option<Number>,
    max_length: Option<Number>,
    pattern: Option<Regex>,
    content_encoding: Option<Decoder>,
    content_media_type: Option<MediaType>,
    content_schema: Option<SchemaIndex>,

    minimum: Option<Number>,
    maximum: Option<Number>,
    exclusive_minimum: Option<Number>,
    exclusive_maximum: Option<Number>,
    multiple_of: Option<Number>,
}

#[derive(Debug)]
struct Enum {
    types: Types,

    values: Vec<Value>,
}

#[derive(Debug)]
enum Items {
    SchemaRef(SchemaIndex),
    SchemaRefs(Vec<SchemaIndex>),
}

#[derive(Debug)]
enum Additional {
    Bool(bool),
    SchemaRef(SchemaIndex),
}

#[derive(Debug)]
enum Dependency {
    Props(Vec<String>),
    SchemaRef(SchemaIndex),
}

struct DynamicRef {
    sch: SchemaIndex,
    anchor: Option<String>,
}

impl Schema {
    fn new(loc: String) -> Self {
        Self {
            loc,
            ..Default::default()
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Type {
    Null = 1,
    Boolean = 2,
    Number = 4,
    Integer = 8,
    String = 16,
    Array = 32,
    Object = 64,
}

impl Type {
    const fn of(v: &Value) -> Self {
        match v {
            Value::Null => Self::Null,
            Value::Bool(_) => Self::Boolean,
            Value::Number(_) => Self::Number,
            Value::String(_) => Self::String,
            Value::Array(_) => Self::Array,
            Value::Object(_) => Self::Object,
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "null" => Some(Self::Null),
            "boolean" => Some(Self::Boolean),
            "number" => Some(Self::Number),
            "integer" => Some(Self::Integer),
            "string" => Some(Self::String),
            "array" => Some(Self::Array),
            "object" => Some(Self::Object),
            _ => None,
        }
    }

    const fn primitive(v: &Value) -> bool {
        !matches!(Self::of(v), Self::Array | Self::Object)
    }
}

impl Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Null => write!(f, "null"),
            Self::Boolean => write!(f, "boolean"),
            Self::Number => write!(f, "number"),
            Self::Integer => write!(f, "integer"),
            Self::String => write!(f, "string"),
            Self::Array => write!(f, "array"),
            Self::Object => write!(f, "object"),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Types(u8);

impl Types {
    const fn is_empty(self) -> bool {
        self.0 == 0
    }

    const fn add(&mut self, t: Type) {
        self.0 |= t as u8;
    }

    pub const fn contains(&self, t: Type) -> bool {
        self.0 & t as u8 != 0
    }

    pub fn iter(&self) -> impl Iterator<Item = Type> + '_ {
        static TYPES: [Type; 7] = [
            Type::Null,
            Type::Boolean,
            Type::Number,
            Type::Integer,
            Type::String,
            Type::Array,
            Type::Object,
        ];
        TYPES.iter().cloned().filter(|t| self.contains(*t))
    }
}

impl FromIterator<Type> for Types {
    fn from_iter<T: IntoIterator<Item = Type>>(iter: T) -> Self {
        let mut types = Self::default();
        for t in iter {
            types.add(t);
        }
        types
    }
}

#[derive(Debug)]
pub struct ValidationError<'s, 'v> {
    pub schema_url: &'s str,

    pub instance_location: InstanceLocation<'v>,

    pub kind: ErrorKind<'s, 'v>,

    pub causes: Vec<Self>,
}

impl Error for ValidationError<'_, '_> {}

#[derive(Debug)]
pub enum ErrorKind<'s, 'v> {
    Group,
    Schema {
        url: &'s str,
    },
    ContentSchema,
    PropertyName {
        prop: String,
    },
    Reference {
        kw: &'static str,
        url: &'s str,
    },
    RefCycle {
        url: &'s str,
        kw_loc1: String,
        kw_loc2: String,
    },
    FalseSchema,
    Type {
        got: Type,
        want: Types,
    },
    Enum {
        want: &'s Vec<Value>,
    },
    Const {
        want: &'s Value,
    },
    Format {
        got: Cow<'v, Value>,
        want: &'static str,
        err: Box<dyn Error>,
    },
    MinProperties {
        got: usize,
        want: &'s Number,
    },
    MaxProperties {
        got: usize,
        want: &'s Number,
    },
    AdditionalProperties {
        got: Vec<Cow<'v, str>>,
    },
    Required {
        want: Vec<&'s str>,
    },
    Dependency {
        prop: &'s str,

        missing: Vec<&'s str>,
    },
    DependentRequired {
        prop: &'s str,

        missing: Vec<&'s str>,
    },
    MinItems {
        got: usize,
        want: &'s Number,
    },
    MaxItems {
        got: usize,
        want: &'s Number,
    },
    Contains,
    MinContains {
        got: Vec<usize>,
        want: &'s Number,
    },
    MaxContains {
        got: Vec<usize>,
        want: &'s Number,
    },
    UniqueItems {
        got: [usize; 2],
    },
    AdditionalItems {
        got: usize,
    },
    MinLength {
        got: usize,
        want: &'s Number,
    },
    MaxLength {
        got: usize,
        want: &'s Number,
    },
    Pattern {
        got: Cow<'v, str>,
        want: &'s str,
    },
    ContentEncoding {
        want: &'static str,
        err: Box<dyn Error>,
    },
    ContentMediaType {
        got: Vec<u8>,
        want: &'static str,
        err: Box<dyn Error>,
    },
    Minimum {
        got: Cow<'v, Number>,
        want: &'s Number,
    },
    Maximum {
        got: Cow<'v, Number>,
        want: &'s Number,
    },
    ExclusiveMinimum {
        got: Cow<'v, Number>,
        want: &'s Number,
    },
    ExclusiveMaximum {
        got: Cow<'v, Number>,
        want: &'s Number,
    },
    MultipleOf {
        got: Cow<'v, Number>,
        want: &'s Number,
    },
    Not,

    AllOf,

    AnyOf,

    OneOf(Option<(usize, usize)>),
}

impl Display for ErrorKind<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Group => write!(f, "validation failed"),
            Self::Schema { url } => write!(f, "validation failed with {url}"),
            Self::ContentSchema => write!(f, "contentSchema failed"),
            Self::PropertyName { prop } => write!(f, "invalid property {}", quote(prop)),
            Self::Reference { .. } => {
                write!(f, "validation failed")
            }
            Self::RefCycle {
                url,
                kw_loc1,
                kw_loc2,
            } => write!(
                f,
                "both {} and {} resolve to {url} causing reference cycle",
                quote(&kw_loc1.to_string()),
                quote(&kw_loc2.to_string())
            ),
            Self::FalseSchema => write!(f, "false schema"),
            Self::Type { got, want } => {
                let want = join_iter(want.iter(), " or ");
                write!(f, "want {want}, but got {got}",)
            }
            Self::Enum { want } => {
                if want.iter().all(Type::primitive) {
                    if want.len() == 1 {
                        write!(f, "value must be ")?;
                        display(f, &want[0])
                    } else {
                        let want = join_iter(want.iter().map(string), ", ");
                        write!(f, "value must be one of {want}")
                    }
                } else {
                    write!(f, "enum failed")
                }
            }
            Self::Const { want } => {
                if Type::primitive(want) {
                    write!(f, "value must be ")?;
                    display(f, want)
                } else {
                    write!(f, "const failed")
                }
            }
            Self::Format { got, want, err } => {
                display(f, got)?;
                write!(f, " is not valid {want}: {err}")
            }
            Self::MinProperties { got, want } => write!(
                f,
                "minimum {want} properties required, but got {got} properties"
            ),
            Self::MaxProperties { got, want } => write!(
                f,
                "maximum {want} properties required, but got {got} properties"
            ),
            Self::AdditionalProperties { got } => {
                write!(
                    f,
                    "additionalProperties {} not allowed",
                    join_iter(got.iter().map(quote), ", ")
                )
            }
            Self::Required { want } => write!(
                f,
                "missing properties {}",
                join_iter(want.iter().map(quote), ", ")
            ),
            Self::Dependency { prop, missing } => {
                write!(
                    f,
                    "properties {} required, if {} property exists",
                    join_iter(missing.iter().map(quote), ", "),
                    quote(prop)
                )
            }
            Self::DependentRequired { prop, missing } => write!(
                f,
                "properties {} required, if {} property exists",
                join_iter(missing.iter().map(quote), ", "),
                quote(prop)
            ),
            Self::MinItems { got, want } => {
                write!(f, "minimum {want} items required, but got {got} items")
            }
            Self::MaxItems { got, want } => {
                write!(f, "maximum {want} items required, but got {got} items")
            }
            Self::MinContains { got, want } => {
                if got.is_empty() {
                    write!(
                        f,
                        "minimum {want} items required to match contains schema, but found none",
                    )
                } else {
                    write!(
                        f,
                        "minimum {want} items required to match contains schema, but found {} items at {}",
                        got.len(),
                        join_iter(got, ", ")
                    )
                }
            }
            Self::Contains => write!(f, "no items match contains schema"),
            Self::MaxContains { got, want } => {
                write!(
                    f,
                    "maximum {want} items required to match contains schema, but found {} items at {}",
                    got.len(),
                    join_iter(got, ", ")
                )
            }
            Self::UniqueItems { got: [i, j] } => write!(f, "items at {i} and {j} are equal"),
            Self::AdditionalItems { got } => write!(f, "last {got} additionalItems not allowed"),
            Self::MinLength { got, want } => write!(f, "length must be >={want}, but got {got}"),
            Self::MaxLength { got, want } => write!(f, "length must be <={want}, but got {got}"),
            Self::Pattern { got, want } => {
                write!(f, "{} does not match pattern {}", quote(got), quote(want))
            }
            Self::ContentEncoding { want, err } => {
                write!(f, "value is not {} encoded: {err}", quote(want))
            }
            Self::ContentMediaType { want, err, .. } => {
                write!(f, "value is not of mediatype {}: {err}", quote(want))
            }
            Self::Minimum { got, want } => write!(f, "must be >={want}, but got {got}"),
            Self::Maximum { got, want } => write!(f, "must be <={want}, but got {got}"),
            Self::ExclusiveMinimum { got, want } => write!(f, "must be > {want} but got {got}"),
            Self::ExclusiveMaximum { got, want } => write!(f, "must be < {want} but got {got}"),
            Self::MultipleOf { got, want } => write!(f, "{got} is not multipleOf {want}"),
            Self::Not => write!(f, "not failed"),
            Self::AllOf => write!(f, "allOf failed",),
            Self::AnyOf => write!(f, "anyOf failed"),
            Self::OneOf(None) => write!(f, "oneOf failed, none matched"),
            Self::OneOf(Some((i, j))) => write!(f, "oneOf failed, subschemas {i}, {j} matched"),
        }
    }
}

fn display(f: &mut std::fmt::Formatter, v: &Value) -> std::fmt::Result {
    match v {
        Value::String(s) => write!(f, "{}", quote(s)),
        Value::Array(_) | Value::Object(_) => write!(f, "value"),
        _ => write!(f, "{v}"),
    }
}

fn string(primitive: &Value) -> String {
    if let Value::String(s) = primitive {
        quote(s)
    } else {
        format!("{primitive}")
    }
}
