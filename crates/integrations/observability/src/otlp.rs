use std::time::{SystemTime, UNIX_EPOCH};

use opentelemetry::{
    Array, InstrumentationScope, KeyValue, Value,
    trace::{Event, Link, SpanContext, SpanId, SpanKind, Status},
};
use opentelemetry_proto::tonic::trace::v1::{SpanFlags, span, status};
use opentelemetry_sdk::{Resource, trace::SpanData};

pub(crate) fn unsigned_integer(text: &str) -> Option<i64> {
    let digits = text.as_bytes();
    let canonical = match digits {
        [] => false,
        [b'0'] => true,
        [first, ..] => *first != b'0' && digits.iter().all(u8::is_ascii_digit),
    };
    canonical.then(|| text.parse().ok()).flatten()
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn plain(bytes: &[u8]) -> usize {
    const STEP: usize = size_of::<u64>();
    const ONES: u64 = u64::MAX / 0xff;
    let mut index = 0;
    for chunk in bytes.as_chunks::<STEP>().0 {
        let word = u64::from_le_bytes(*chunk);
        let control = word.wrapping_sub(ONES * 0x20) & !word;
        let quote = word ^ (ONES * u64::from(b'"'));
        let quote = quote.wrapping_sub(ONES) & !quote;
        let backslash = word ^ (ONES * u64::from(b'\\'));
        let backslash = backslash.wrapping_sub(ONES) & !backslash;
        let found = (control | quote | backslash) & (ONES << 7);
        if found != 0 {
            return index + (found.trailing_zeros() / u8::BITS) as usize;
        }
        index += STEP;
    }
    index
        + bytes[index..]
            .iter()
            .take_while(|&&byte| !matches!(byte, 0..0x20 | b'"' | b'\\'))
            .count()
}

fn string(json: &mut Vec<u8>, text: &str) {
    let mut bytes = text.as_bytes();
    json.push(b'"');
    loop {
        let (run, rest) = bytes.split_at(plain(bytes));
        json.extend_from_slice(run);
        let Some((&byte, rest)) = rest.split_first() else {
            break;
        };
        match byte {
            b'"' => json.extend_from_slice(b"\\\""),
            b'\\' => json.extend_from_slice(b"\\\\"),
            b'\x08' => json.extend_from_slice(b"\\b"),
            b'\x0c' => json.extend_from_slice(b"\\f"),
            b'\n' => json.extend_from_slice(b"\\n"),
            b'\r' => json.extend_from_slice(b"\\r"),
            b'\t' => json.extend_from_slice(b"\\t"),
            _ => json.extend_from_slice(&[
                b'\\',
                b'u',
                b'0',
                b'0',
                HEX[usize::from(byte >> 4)],
                HEX[usize::from(byte & 0xf)],
            ]),
        }
        bytes = rest;
    }
    json.push(b'"');
}

fn hex(json: &mut Vec<u8>, bytes: &[u8]) {
    json.push(b'"');
    for byte in bytes {
        json.extend_from_slice(&[HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0xf)]]);
    }
    json.push(b'"');
}

fn decimal(json: &mut Vec<u8>, mut value: u64) {
    let mut digits = [0_u8; u64::MAX.ilog10() as usize + 1];
    let mut start = digits.len();
    loop {
        start -= 1;
        digits[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    json.extend_from_slice(&digits[start..]);
}

fn integer(json: &mut Vec<u8>, value: i64) {
    if value < 0 {
        json.push(b'-');
    }
    decimal(json, value.unsigned_abs());
}

fn nanos(json: &mut Vec<u8>, time: SystemTime) {
    json.push(b'"');
    decimal(
        json,
        time.duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos() as u64),
    );
    json.push(b'"');
}

fn flags(context: &SpanContext, remote: bool) -> u64 {
    let flags = u32::from(context.trace_flags().to_u8()) | SpanFlags::ContextHasIsRemoteMask as u32;
    u64::from(if remote {
        flags | SpanFlags::ContextIsRemoteMask as u32
    } else {
        flags
    })
}

fn boolean(json: &mut Vec<u8>, value: bool) {
    json.extend_from_slice(if value {
        br#"{"boolValue":true}"#
    } else {
        br#"{"boolValue":false}"#
    });
}

fn whole(json: &mut Vec<u8>, value: i64) {
    json.extend_from_slice(br#"{"intValue":""#);
    integer(json, value);
    json.extend_from_slice(br#""}"#);
}

fn double(json: &mut Vec<u8>, value: f64) -> serde_json::Result<()> {
    json.extend_from_slice(br#"{"doubleValue":"#);
    serde_json::to_writer(&mut *json, &value)?;
    json.push(b'}');
    Ok(())
}

fn text(json: &mut Vec<u8>, value: &str) {
    json.extend_from_slice(br#"{"stringValue":"#);
    string(json, value);
    json.push(b'}');
}

fn unknown(what: &str) -> serde_json::Error {
    <serde_json::Error as serde::ser::Error>::custom(format_args!("{what} OTLP does not define"))
}

fn listed<T>(
    json: &mut Vec<u8>,
    items: impl IntoIterator<Item = T>,
    mut item: impl FnMut(&mut Vec<u8>, T) -> serde_json::Result<()>,
) -> serde_json::Result<()> {
    json.push(b'[');
    for (index, value) in items.into_iter().enumerate() {
        if index > 0 {
            json.push(b',');
        }
        item(json, value)?;
    }
    json.push(b']');
    Ok(())
}

fn any(json: &mut Vec<u8>, value: &Value, integers: bool) -> serde_json::Result<()> {
    match value {
        Value::Bool(value) => boolean(json, *value),
        Value::I64(value) => whole(json, *value),
        Value::F64(value) => double(json, *value)?,
        Value::String(value) => {
            if integers && let Some(value) = unsigned_integer(value.as_str()) {
                whole(json, value);
            } else {
                text(json, value.as_str());
            }
        }
        Value::Array(values) => {
            json.extend_from_slice(br#"{"arrayValue":{"values":"#);
            match values {
                Array::Bool(values) => listed(json, values, |json, value| {
                    boolean(json, *value);
                    Ok(())
                })?,
                Array::I64(values) => listed(json, values, |json, value| {
                    whole(json, *value);
                    Ok(())
                })?,
                Array::F64(values) => listed(json, values, |json, value| double(json, *value))?,
                Array::String(values) => listed(json, values, |json, value| {
                    text(json, value.as_str());
                    Ok(())
                })?,
                _ => return Err(unknown("an attribute array")),
            }
            json.extend_from_slice(b"}}");
        }
        _ => return Err(unknown("an attribute value")),
    }
    Ok(())
}

fn attributes<'a>(
    json: &mut Vec<u8>,
    pairs: impl IntoIterator<Item = &'a KeyValue>,
    integers: bool,
) -> serde_json::Result<()> {
    listed(json, pairs, |json, pair| {
        json.extend_from_slice(br#"{"key":"#);
        string(json, pair.key.as_str());
        json.extend_from_slice(br#","value":"#);
        any(json, &pair.value, integers)?;
        json.push(b'}');
        Ok(())
    })
}

fn count(json: &mut Vec<u8>, name: &[u8], value: u32) {
    json.extend_from_slice(name);
    decimal(json, u64::from(value));
}

fn event(json: &mut Vec<u8>, event: &Event) -> serde_json::Result<()> {
    json.extend_from_slice(br#"{"timeUnixNano":"#);
    nanos(json, event.timestamp);
    json.extend_from_slice(br#","name":"#);
    string(json, &event.name);
    json.extend_from_slice(br#","attributes":"#);
    attributes(json, &event.attributes, false)?;
    count(
        json,
        br#","droppedAttributesCount":"#,
        event.dropped_attributes_count,
    );
    json.push(b'}');
    Ok(())
}

fn link(json: &mut Vec<u8>, link: &Link) -> serde_json::Result<()> {
    let context = &link.span_context;
    json.extend_from_slice(br#"{"traceId":"#);
    hex(json, &context.trace_id().to_bytes());
    json.extend_from_slice(br#","spanId":"#);
    hex(json, &context.span_id().to_bytes());
    json.extend_from_slice(br#","traceState":"#);
    string(json, &context.trace_state().header());
    json.extend_from_slice(br#","attributes":"#);
    attributes(json, &link.attributes, false)?;
    count(
        json,
        br#","droppedAttributesCount":"#,
        link.dropped_attributes_count,
    );
    json.extend_from_slice(br#","flags":"#);
    decimal(json, flags(context, context.is_remote()));
    json.push(b'}');
    Ok(())
}

pub(crate) fn span(data: &SpanData, json: &mut Vec<u8>) -> serde_json::Result<()> {
    let context = &data.span_context;
    let kind = match data.span_kind {
        SpanKind::Client => span::SpanKind::Client,
        SpanKind::Consumer => span::SpanKind::Consumer,
        SpanKind::Internal => span::SpanKind::Internal,
        SpanKind::Producer => span::SpanKind::Producer,
        SpanKind::Server => span::SpanKind::Server,
    };
    let (message, code) = match &data.status {
        Status::Unset => ("", status::StatusCode::Unset),
        Status::Error { description } => (&**description, status::StatusCode::Error),
        Status::Ok => ("", status::StatusCode::Ok),
    };
    json.extend_from_slice(br#"{"traceId":"#);
    hex(json, &context.trace_id().to_bytes());
    json.extend_from_slice(br#","spanId":"#);
    hex(json, &context.span_id().to_bytes());
    json.extend_from_slice(br#","traceState":"#);
    string(json, &context.trace_state().header());
    json.extend_from_slice(br#","parentSpanId":"#);
    if data.parent_span_id == SpanId::INVALID {
        hex(json, &[]);
    } else {
        hex(json, &data.parent_span_id.to_bytes());
    }
    json.extend_from_slice(br#","flags":"#);
    decimal(json, flags(context, data.parent_span_is_remote));
    json.extend_from_slice(br#","name":"#);
    string(json, &data.name);
    json.extend_from_slice(br#","kind":"#);
    integer(json, i64::from(kind as i32));
    json.extend_from_slice(br#","startTimeUnixNano":"#);
    nanos(json, data.start_time);
    json.extend_from_slice(br#","endTimeUnixNano":"#);
    nanos(json, data.end_time);
    json.extend_from_slice(br#","attributes":"#);
    attributes(json, &data.attributes, true)?;
    count(
        json,
        br#","droppedAttributesCount":"#,
        data.dropped_attributes_count,
    );
    json.extend_from_slice(br#","events":"#);
    listed(json, data.events.iter(), event)?;
    count(
        json,
        br#","droppedEventsCount":"#,
        data.events.dropped_count,
    );
    json.extend_from_slice(br#","links":"#);
    listed(json, data.links.iter(), link)?;
    count(json, br#","droppedLinksCount":"#, data.links.dropped_count);
    json.extend_from_slice(br#","status":{"message":"#);
    string(json, message);
    json.extend_from_slice(br#","code":"#);
    integer(json, i64::from(code as i32));
    json.extend_from_slice(b"}}");
    Ok(())
}

#[derive(Debug)]
pub(crate) struct Origin {
    resource: Vec<u8>,
    schema_url: Vec<u8>,
}

impl Origin {
    pub(crate) fn of(resource: &Resource) -> serde_json::Result<Self> {
        let pairs = resource
            .iter()
            .map(|(key, value)| KeyValue::new(key.clone(), value.clone()))
            .collect::<Vec<_>>();
        let mut json = br#"{"attributes":"#.to_vec();
        attributes(&mut json, &pairs, false)?;
        json.extend_from_slice(br#","droppedAttributesCount":0,"entityRefs":[]}"#);
        let mut schema_url = Vec::new();
        string(&mut schema_url, resource.schema_url().unwrap_or_default());
        Ok(Self {
            resource: json,
            schema_url,
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
        line.extend_from_slice(br#","scopeSpans":"#);
        listed(line, self.scopes.drain(..), |line, (scope, spans)| {
            line.extend_from_slice(br#"{"scope":{"name":"#);
            string(line, scope.name());
            line.extend_from_slice(br#","version":"#);
            string(line, scope.version().unwrap_or_default());
            line.extend_from_slice(br#","attributes":"#);
            attributes(line, scope.attributes(), false)?;
            line.extend_from_slice(br#","droppedAttributesCount":0},"spans":["#);
            line.extend_from_slice(&spans);
            line.extend_from_slice(br#"],"schemaUrl":"#);
            string(line, scope.schema_url().unwrap_or_default());
            line.push(b'}');
            Ok(())
        })?;
        line.extend_from_slice(br#","schemaUrl":"#);
        line.extend_from_slice(&origin.schema_url);
        line.extend_from_slice(b"}]}\n");
        Ok(())
    }
}
