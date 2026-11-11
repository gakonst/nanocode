use std::time::{SystemTime, UNIX_EPOCH};

use opentelemetry::{
    Array, InstrumentationScope, KeyValue, Value,
    trace::{Event, Link, SpanContext, SpanId, SpanKind, Status, TraceId},
};
use opentelemetry_proto::tonic::trace::v1::{SpanFlags, span, status};
use opentelemetry_sdk::{Resource, trace::SpanData};
use serde::{
    Serialize, Serializer,
    ser::{Error, SerializeMap, SerializeStruct},
};

pub(crate) fn unsigned_integer(text: &str) -> Option<i64> {
    let digits = text.as_bytes();
    let canonical = match digits {
        [] => false,
        [b'0'] => true,
        [first, ..] => *first != b'0' && digits.iter().all(u8::is_ascii_digit),
    };
    canonical.then(|| text.parse().ok()).flatten()
}

struct Text<T>(T);

impl<T: std::fmt::Display> Serialize for Text<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

struct Hex<'a>(&'a [u8]);

impl Serialize for Hex<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut text = [0_u8; 2 * size_of::<TraceId>()];
        let text = text
            .get_mut(..2 * self.0.len())
            .ok_or_else(|| S::Error::custom("an identifier longer than a trace id"))?;
        for (pair, byte) in text.chunks_exact_mut(2).zip(self.0) {
            pair[0] = DIGITS[usize::from(byte >> 4)];
            pair[1] = DIGITS[usize::from(byte & 0xf)];
        }
        serializer.serialize_str(std::str::from_utf8(text).map_err(S::Error::custom)?)
    }
}

fn nanos(time: SystemTime) -> Text<u64> {
    Text(
        time.duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos() as u64),
    )
}

fn flags(context: &SpanContext, remote: bool) -> u32 {
    let flags = u32::from(context.trace_flags().to_u8()) | SpanFlags::ContextHasIsRemoteMask as u32;
    if remote {
        flags | SpanFlags::ContextIsRemoteMask as u32
    } else {
        flags
    }
}

enum Scalar<'a> {
    Bool(bool),
    Integer(i64),
    Double(f64),
    String(&'a str),
}

impl Serialize for Scalar<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        match self {
            Self::Bool(value) => map.serialize_entry("boolValue", value)?,
            Self::Integer(value) => map.serialize_entry("intValue", &Text(value))?,
            Self::Double(value) => map.serialize_entry("doubleValue", value)?,
            Self::String(value) => map.serialize_entry("stringValue", value)?,
        }
        map.end()
    }
}

struct Values<'a>(&'a Array);

impl Serialize for Values<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut array = serializer.serialize_struct("ArrayValue", 1)?;
        match self.0 {
            Array::Bool(values) => array.serialize_field(
                "values",
                &values.iter().copied().map(Scalar::Bool).collect::<Vec<_>>(),
            )?,
            Array::I64(values) => array.serialize_field(
                "values",
                &values
                    .iter()
                    .copied()
                    .map(Scalar::Integer)
                    .collect::<Vec<_>>(),
            )?,
            Array::F64(values) => array.serialize_field(
                "values",
                &values
                    .iter()
                    .copied()
                    .map(Scalar::Double)
                    .collect::<Vec<_>>(),
            )?,
            Array::String(values) => array.serialize_field(
                "values",
                &values
                    .iter()
                    .map(|value| Scalar::String(value.as_str()))
                    .collect::<Vec<_>>(),
            )?,
            _ => return Err(S::Error::custom("an attribute array OTLP does not define")),
        }
        array.end()
    }
}

struct Any<'a> {
    value: &'a Value,
    integers: bool,
}

impl Serialize for Any<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.value {
            Value::Bool(value) => Scalar::Bool(*value).serialize(serializer),
            Value::I64(value) => Scalar::Integer(*value).serialize(serializer),
            Value::F64(value) => Scalar::Double(*value).serialize(serializer),
            Value::String(value) => {
                if self.integers
                    && let Some(integer) = unsigned_integer(value.as_str())
                {
                    return Scalar::Integer(integer).serialize(serializer);
                }
                Scalar::String(value.as_str()).serialize(serializer)
            }
            Value::Array(values) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("arrayValue", &Values(values))?;
                map.end()
            }
            _ => Err(S::Error::custom("an attribute value OTLP does not define")),
        }
    }
}

struct Attribute<'a> {
    pair: &'a KeyValue,
    integers: bool,
}

impl Serialize for Attribute<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut pair = serializer.serialize_struct("KeyValue", 2)?;
        pair.serialize_field("key", self.pair.key.as_str())?;
        pair.serialize_field(
            "value",
            &Any {
                value: &self.pair.value,
                integers: self.integers,
            },
        )?;
        pair.end()
    }
}

struct Attributes<I> {
    pairs: I,
    integers: bool,
}

impl<'a, I: Iterator<Item = &'a KeyValue> + Clone> Serialize for Attributes<I> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.pairs.clone().map(|pair| Attribute {
            pair,
            integers: self.integers,
        }))
    }
}

fn attributes(pairs: &[KeyValue]) -> Attributes<std::slice::Iter<'_, KeyValue>> {
    Attributes {
        pairs: pairs.iter(),
        integers: false,
    }
}

struct Occurrence<'a>(&'a Event);

impl Serialize for Occurrence<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut event = serializer.serialize_struct("Event", 4)?;
        event.serialize_field("timeUnixNano", &nanos(self.0.timestamp))?;
        event.serialize_field("name", &*self.0.name)?;
        event.serialize_field("attributes", &attributes(&self.0.attributes))?;
        event.serialize_field("droppedAttributesCount", &self.0.dropped_attributes_count)?;
        event.end()
    }
}

struct Linked<'a>(&'a Link);

impl Serialize for Linked<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let context = &self.0.span_context;
        let mut link = serializer.serialize_struct("Link", 6)?;
        link.serialize_field("traceId", &Hex(&context.trace_id().to_bytes()))?;
        link.serialize_field("spanId", &Hex(&context.span_id().to_bytes()))?;
        link.serialize_field("traceState", &context.trace_state().header())?;
        link.serialize_field("attributes", &attributes(&self.0.attributes))?;
        link.serialize_field("droppedAttributesCount", &self.0.dropped_attributes_count)?;
        link.serialize_field("flags", &flags(context, context.is_remote()))?;
        link.end()
    }
}

struct Outcome<'a>(&'a Status);

impl Serialize for Outcome<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (message, code) = match self.0 {
            Status::Unset => ("", status::StatusCode::Unset),
            Status::Error { description } => (&**description, status::StatusCode::Error),
            Status::Ok => ("", status::StatusCode::Ok),
        };
        let mut status = serializer.serialize_struct("Status", 2)?;
        status.serialize_field("message", message)?;
        status.serialize_field("code", &(code as i32))?;
        status.end()
    }
}

struct Span<'a>(&'a SpanData);

impl Serialize for Span<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let data = self.0;
        let context = &data.span_context;
        let parent = data.parent_span_id.to_bytes();
        let kind = match data.span_kind {
            SpanKind::Client => span::SpanKind::Client,
            SpanKind::Consumer => span::SpanKind::Consumer,
            SpanKind::Internal => span::SpanKind::Internal,
            SpanKind::Producer => span::SpanKind::Producer,
            SpanKind::Server => span::SpanKind::Server,
        };
        let mut span = serializer.serialize_struct("Span", 16)?;
        span.serialize_field("traceId", &Hex(&context.trace_id().to_bytes()))?;
        span.serialize_field("spanId", &Hex(&context.span_id().to_bytes()))?;
        span.serialize_field("traceState", &context.trace_state().header())?;
        span.serialize_field(
            "parentSpanId",
            &Hex(if data.parent_span_id == SpanId::INVALID {
                &[]
            } else {
                &parent
            }),
        )?;
        span.serialize_field("flags", &flags(context, data.parent_span_is_remote))?;
        span.serialize_field("name", &*data.name)?;
        span.serialize_field("kind", &(kind as i32))?;
        span.serialize_field("startTimeUnixNano", &nanos(data.start_time))?;
        span.serialize_field("endTimeUnixNano", &nanos(data.end_time))?;
        span.serialize_field(
            "attributes",
            &Attributes {
                pairs: data.attributes.iter(),
                integers: true,
            },
        )?;
        span.serialize_field("droppedAttributesCount", &data.dropped_attributes_count)?;
        span.serialize_field(
            "events",
            &data.events.iter().map(Occurrence).collect::<Vec<_>>(),
        )?;
        span.serialize_field("droppedEventsCount", &data.events.dropped_count)?;
        span.serialize_field("links", &data.links.iter().map(Linked).collect::<Vec<_>>())?;
        span.serialize_field("droppedLinksCount", &data.links.dropped_count)?;
        span.serialize_field("status", &Outcome(&data.status))?;
        span.end()
    }
}

struct Scope<'a>(&'a InstrumentationScope);

impl Serialize for Scope<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut scope = serializer.serialize_struct("InstrumentationScope", 4)?;
        scope.serialize_field("name", self.0.name())?;
        scope.serialize_field("version", self.0.version().unwrap_or_default())?;
        scope.serialize_field(
            "attributes",
            &self
                .0
                .attributes()
                .map(|pair| Attribute {
                    pair,
                    integers: false,
                })
                .collect::<Vec<_>>(),
        )?;
        scope.serialize_field("droppedAttributesCount", &0_u32)?;
        scope.end()
    }
}

struct Described<'a>(&'a Resource);

impl Serialize for Described<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let pairs = self
            .0
            .iter()
            .map(|(key, value)| KeyValue::new(key.clone(), value.clone()))
            .collect::<Vec<_>>();
        let mut resource = serializer.serialize_struct("Resource", 3)?;
        resource.serialize_field("attributes", &attributes(&pairs))?;
        resource.serialize_field("droppedAttributesCount", &0_u32)?;
        resource.serialize_field("entityRefs", &[(); 0])?;
        resource.end()
    }
}

pub(crate) fn span(data: &SpanData, json: &mut Vec<u8>) -> serde_json::Result<()> {
    serde_json::to_writer(json, &Span(data))
}

#[derive(Debug, Default)]
pub(crate) struct Origin {
    resource: Vec<u8>,
    schema_url: Vec<u8>,
}

impl Origin {
    pub(crate) fn of(resource: &Resource) -> serde_json::Result<Self> {
        Ok(Self {
            resource: serde_json::to_vec(&Described(resource))?,
            schema_url: serde_json::to_vec(resource.schema_url().unwrap_or_default())?,
        })
    }
}

#[derive(Default)]
pub(crate) struct Batch {
    scopes: Vec<(InstrumentationScope, Vec<u8>)>,
}

impl Batch {
    pub(crate) const fn is_empty(&self) -> bool {
        self.scopes.is_empty()
    }

    pub(crate) fn push(&mut self, scope: InstrumentationScope, span: &[u8]) {
        match self.scopes.iter_mut().find(|(held, _)| *held == scope) {
            Some((_, spans)) => {
                spans.push(b',');
                spans.extend_from_slice(span);
            }
            None => self.scopes.push((scope, span.to_vec())),
        }
    }

    pub(crate) fn line(&mut self, origin: &Origin, line: &mut Vec<u8>) -> serde_json::Result<()> {
        line.extend_from_slice(br#"{"resourceSpans":[{"resource":"#);
        line.extend_from_slice(&origin.resource);
        line.extend_from_slice(br#","scopeSpans":["#);
        for (index, (scope, spans)) in self.scopes.drain(..).enumerate() {
            if index > 0 {
                line.push(b',');
            }
            line.extend_from_slice(br#"{"scope":"#);
            serde_json::to_writer(&mut *line, &Scope(&scope))?;
            line.extend_from_slice(br#","spans":["#);
            line.extend_from_slice(&spans);
            line.extend_from_slice(br#"],"schemaUrl":"#);
            serde_json::to_writer(&mut *line, scope.schema_url().unwrap_or_default())?;
            line.push(b'}');
        }
        line.extend_from_slice(br#"],"schemaUrl":"#);
        line.extend_from_slice(&origin.schema_url);
        line.extend_from_slice(b"}]}\n");
        Ok(())
    }
}
