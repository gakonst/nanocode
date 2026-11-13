use std::{
    borrow::Cow,
    hash::{Hash, Hasher},
    time::SystemTime,
};

pub type Text = Cow<'static, str>;

#[derive(Clone, Debug)]
pub enum Value {
    Bool(bool),
    I64(i64),
    F64(f64),
    String(Text),
    Strings(Vec<Text>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::I64(left), Self::I64(right)) => left == right,
            (Self::F64(left), Self::F64(right)) => left.to_bits() == right.to_bits(),
            (Self::String(left), Self::String(right)) => left == right,
            (Self::Strings(left), Self::Strings(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Bool(value) => value.hash(state),
            Self::I64(value) => value.hash(state),
            Self::F64(value) => value.to_bits().hash(state),
            Self::String(value) => value.hash(state),
            Self::Strings(value) => value.hash(state),
        }
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::I64(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::F64(value)
    }
}

impl From<&'static str> for Value {
    fn from(value: &'static str) -> Self {
        Self::String(Cow::Borrowed(value))
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(Cow::Owned(value))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KeyValue {
    pub key: Text,
    pub value: Value,
}

impl KeyValue {
    pub fn new(key: impl Into<Text>, value: impl Into<Value>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpanContext {
    pub trace_id: u128,
    pub span_id: u64,
    pub sampled: bool,
}

impl SpanContext {
    pub const INVALID: Self = Self {
        trace_id: 0,
        span_id: 0,
        sampled: false,
    };

    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.trace_id != 0 && self.span_id != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Client,
    Server,
    Producer,
    Consumer,
    Internal,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    #[default]
    Unset,
    Error(Text),
    Ok,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub name: Text,
    pub timestamp: SystemTime,
    pub attributes: Vec<KeyValue>,
    pub dropped_attributes_count: u32,
}

#[derive(Clone, Debug)]
pub struct SpanData {
    pub context: SpanContext,
    pub parent_span_id: u64,
    pub kind: SpanKind,
    pub name: Text,
    pub start_time: SystemTime,
    pub end_time: SystemTime,
    pub attributes: Vec<KeyValue>,
    pub dropped_attributes_count: u32,
    pub events: Vec<Event>,
    pub dropped_events_count: u32,
    pub links: Vec<SpanContext>,
    pub dropped_links_count: u32,
    pub status: Status,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    pub name: Text,
    pub version: Option<Text>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resource {
    pub attributes: Vec<KeyValue>,
    pub schema_url: Option<Text>,
}

impl Resource {
    pub fn insert(&mut self, pair: KeyValue) {
        match self.attributes.iter_mut().find(|held| held.key == pair.key) {
            Some(held) => held.value = pair.value,
            None => self.attributes.push(pair),
        }
    }

    pub fn merge(&self, other: &Self) -> Self {
        if self.attributes.is_empty() && self.schema_url.is_none() {
            return other.clone();
        }
        if other.attributes.is_empty() && other.schema_url.is_none() {
            return self.clone();
        }
        let mut merged = Self {
            attributes: self.attributes.clone(),
            schema_url: match (&self.schema_url, &other.schema_url) {
                (Some(left), Some(right)) if left == right => Some(left.clone()),
                (Some(_), Some(_)) | (None, None) => None,
                (None, Some(url)) | (Some(url), None) => Some(url.clone()),
            },
        };
        for pair in &other.attributes {
            merged.insert(pair.clone());
        }
        merged
    }
}

#[derive(Debug)]
pub enum ExportError {
    AlreadyShutdown,

    Timeout(std::time::Duration),

    InternalFailure(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyShutdown => formatter.write_str("Shutdown already invoked"),
            Self::Timeout(timeout) => write!(formatter, "Operation timed out after {timeout:?}"),
            Self::InternalFailure(reason) => write!(formatter, "Operation failed: {reason}"),
        }
    }
}

impl std::error::Error for ExportError {}

pub(crate) type Exported = Result<(), ExportError>;
