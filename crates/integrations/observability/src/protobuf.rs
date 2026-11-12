use crate::{
    metrics::{
        HISTOGRAM_BOUNDS, HistogramPoint, Metric, MetricData, NumberPoint, Point, ResourceMetrics,
    },
    model::{KeyValue, Resource, Scope, SpanContext, SpanData, Value},
    otlp::{self, unix_nanos},
};

const WIRE_VARINT: u64 = 0;
const WIRE_FIXED64: u64 = 1;
const WIRE_LENGTH_DELIMITED: u64 = 2;
const WIRE_FIXED32: u64 = 5;
const WIRE_TYPE_BITS: u32 = 3;
const VARINT_GROUP_BITS: u32 = 7;
const VARINT_CONTINUATION: u8 = 0x80;
const VARINT_BYTES_MAX: usize = u64::BITS.div_ceil(VARINT_GROUP_BITS) as usize;

mod request {
    pub const RESOURCE: u32 = 1;
}

mod resource_group {
    pub const RESOURCE: u32 = 1;
    pub const SCOPES: u32 = 2;
    pub const SCHEMA_URL: u32 = 3;
}

mod scope_group {
    pub const SCOPE: u32 = 1;
    pub const ITEMS: u32 = 2;
}

mod resource {
    pub const ATTRIBUTES: u32 = 1;
}

mod scope {
    pub const NAME: u32 = 1;
    pub const VERSION: u32 = 2;
}

mod key_value {
    pub const KEY: u32 = 1;
    pub const VALUE: u32 = 2;
}

mod any_value {
    pub const STRING: u32 = 1;
    pub const BOOL: u32 = 2;
    pub const INT: u32 = 3;
    pub const DOUBLE: u32 = 4;
    pub const ARRAY: u32 = 5;
}

mod array_value {
    pub const VALUES: u32 = 1;
}

mod span {
    pub const TRACE_ID: u32 = 1;
    pub const SPAN_ID: u32 = 2;
    pub const PARENT_SPAN_ID: u32 = 4;
    pub const NAME: u32 = 5;
    pub const KIND: u32 = 6;
    pub const START_TIME_UNIX_NANO: u32 = 7;
    pub const END_TIME_UNIX_NANO: u32 = 8;
    pub const ATTRIBUTES: u32 = 9;
    pub const DROPPED_ATTRIBUTES_COUNT: u32 = 10;
    pub const EVENTS: u32 = 11;
    pub const DROPPED_EVENTS_COUNT: u32 = 12;
    pub const LINKS: u32 = 13;
    pub const DROPPED_LINKS_COUNT: u32 = 14;
    pub const STATUS: u32 = 15;
    pub const FLAGS: u32 = 16;
}

mod event {
    pub const TIME_UNIX_NANO: u32 = 1;
    pub const NAME: u32 = 2;
    pub const ATTRIBUTES: u32 = 3;
    pub const DROPPED_ATTRIBUTES_COUNT: u32 = 4;
}

mod link {
    pub const TRACE_ID: u32 = 1;
    pub const SPAN_ID: u32 = 2;
    pub const FLAGS: u32 = 6;
}

mod status {
    pub const MESSAGE: u32 = 2;
    pub const CODE: u32 = 3;
}

mod metric {
    pub const NAME: u32 = 1;
    pub const GAUGE: u32 = 5;
    pub const SUM: u32 = 7;
    pub const HISTOGRAM: u32 = 9;
}

mod aggregation {
    pub const DATA_POINTS: u32 = 1;
    pub const AGGREGATION_TEMPORALITY: u32 = 2;
    pub const IS_MONOTONIC: u32 = 3;
}

mod number_point {
    pub const START_TIME_UNIX_NANO: u32 = 2;
    pub const TIME_UNIX_NANO: u32 = 3;
    pub const AS_DOUBLE: u32 = 4;
    pub const AS_INT: u32 = 6;
    pub const ATTRIBUTES: u32 = 7;
    pub const FLAGS: u32 = 8;
}

mod histogram_point {
    pub const START_TIME_UNIX_NANO: u32 = 2;
    pub const TIME_UNIX_NANO: u32 = 3;
    pub const COUNT: u32 = 4;
    pub const SUM: u32 = 5;
    pub const BUCKET_COUNTS: u32 = 6;
    pub const EXPLICIT_BOUNDS: u32 = 7;
    pub const ATTRIBUTES: u32 = 9;
    pub const FLAGS: u32 = 10;
    pub const MIN: u32 = 11;
    pub const MAX: u32 = 12;
}

fn varint_bytes(mut value: u64) -> ([u8; VARINT_BYTES_MAX], usize) {
    let mut bytes = [0_u8; VARINT_BYTES_MAX];
    let mut length = 0;
    while value >= u64::from(VARINT_CONTINUATION) {
        bytes[length] = value as u8 | VARINT_CONTINUATION;
        value >>= VARINT_GROUP_BITS;
        length += 1;
    }
    bytes[length] = value as u8;
    (bytes, length + 1)
}

fn varint(out: &mut Vec<u8>, value: u64) {
    let (bytes, length) = varint_bytes(value);
    out.extend_from_slice(&bytes[..length]);
}

fn key(out: &mut Vec<u8>, field: u32, wire: u64) {
    varint(out, (u64::from(field) << WIRE_TYPE_BITS) | wire);
}

fn message(out: &mut Vec<u8>, field: u32, body: impl FnOnce(&mut Vec<u8>)) {
    key(out, field, WIRE_LENGTH_DELIMITED);
    let start = out.len();
    body(out);
    let length = out.len() - start;
    let (prefix, prefix_length) = varint_bytes(length as u64);
    out.extend_from_slice(&prefix[..prefix_length]);
    out.copy_within(start..start + length, start + prefix_length);
    out[start..start + prefix_length].copy_from_slice(&prefix[..prefix_length]);
    assert_eq!(out.len(), start + prefix_length + length);
}

fn length_delimited(out: &mut Vec<u8>, field: u32, bytes: &[u8]) {
    key(out, field, WIRE_LENGTH_DELIMITED);
    varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn string(out: &mut Vec<u8>, field: u32, text: &str) {
    if !text.is_empty() {
        length_delimited(out, field, text.as_bytes());
    }
}

fn bytes(out: &mut Vec<u8>, field: u32, bytes: &[u8]) {
    if !bytes.is_empty() {
        length_delimited(out, field, bytes);
    }
}

fn uint32(out: &mut Vec<u8>, field: u32, value: u32) {
    if value != 0 {
        key(out, field, WIRE_VARINT);
        varint(out, u64::from(value));
    }
}

fn enumeration(out: &mut Vec<u8>, field: u32, value: i32) {
    if value != 0 {
        key(out, field, WIRE_VARINT);
        varint(out, i64::from(value) as u64);
    }
}

fn boolean(out: &mut Vec<u8>, field: u32, value: bool) {
    if value {
        key(out, field, WIRE_VARINT);
        varint(out, u64::from(value));
    }
}

fn fixed64_always(out: &mut Vec<u8>, field: u32, value: u64) {
    key(out, field, WIRE_FIXED64);
    out.extend_from_slice(&value.to_le_bytes());
}

fn fixed64(out: &mut Vec<u8>, field: u32, value: u64) {
    if value != 0 {
        fixed64_always(out, field, value);
    }
}

fn fixed32(out: &mut Vec<u8>, field: u32, value: u32) {
    if value != 0 {
        key(out, field, WIRE_FIXED32);
        out.extend_from_slice(&value.to_le_bytes());
    }
}

fn packed<T>(
    out: &mut Vec<u8>,
    field: u32,
    values: impl Iterator<Item = T>,
    mut each: impl FnMut(&mut Vec<u8>, T),
) {
    let mut values = values.peekable();
    if values.peek().is_none() {
        return;
    }
    message(out, field, |out| values.for_each(|value| each(out, value)));
}

fn any_string(out: &mut Vec<u8>, value: &str) {
    length_delimited(out, any_value::STRING, value.as_bytes());
}

fn any(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Bool(value) => {
            key(out, any_value::BOOL, WIRE_VARINT);
            varint(out, u64::from(*value));
        }
        Value::I64(value) => {
            key(out, any_value::INT, WIRE_VARINT);
            varint(out, *value as u64);
        }
        Value::F64(value) => fixed64_always(out, any_value::DOUBLE, value.to_bits()),
        Value::String(value) => any_string(out, value),
        Value::Strings(values) => message(out, any_value::ARRAY, |out| {
            for value in values {
                message(out, array_value::VALUES, |out| any_string(out, value));
            }
        }),
    }
}

fn attributes<'a>(out: &mut Vec<u8>, field: u32, pairs: impl IntoIterator<Item = &'a KeyValue>) {
    for pair in pairs {
        message(out, field, |out| {
            string(out, key_value::KEY, &pair.key);
            message(out, key_value::VALUE, |out| any(out, &pair.value));
        });
    }
}

fn scope(out: &mut Vec<u8>, held: &Scope) {
    string(out, scope::NAME, &held.name);
    string(
        out,
        scope::VERSION,
        held.version.as_deref().unwrap_or_default(),
    );
}

#[derive(Debug)]
pub(crate) struct Origin {
    resource: Vec<u8>,
    schema_url: String,
    scope: Scope,
}

impl Origin {
    pub(crate) fn of(held: &Resource, held_scope: &Scope) -> Self {
        let mut encoded = Vec::new();
        attributes(&mut encoded, resource::ATTRIBUTES, &held.attributes);
        Self {
            resource: encoded,
            schema_url: held.schema_url.as_deref().unwrap_or_default().to_owned(),
            scope: held_scope.clone(),
        }
    }
}

pub(crate) fn traces(out: &mut Vec<u8>, origin: &Origin, batch: &[SpanData]) {
    message(out, request::RESOURCE, |out| {
        message(out, resource_group::RESOURCE, |out| {
            out.extend_from_slice(&origin.resource);
        });
        message(out, resource_group::SCOPES, |out| {
            message(out, scope_group::SCOPE, |out| scope(out, &origin.scope));
            for data in batch {
                message(out, scope_group::ITEMS, |out| span(out, data));
            }
        });
        string(out, resource_group::SCHEMA_URL, &origin.schema_url);
    });
}

fn link(out: &mut Vec<u8>, linked: &SpanContext) {
    message(out, span::LINKS, |out| {
        bytes(out, link::TRACE_ID, &linked.trace_id.to_be_bytes());
        bytes(out, link::SPAN_ID, &linked.span_id.to_be_bytes());
        fixed32(out, link::FLAGS, otlp::flags(linked));
    });
}

fn span(out: &mut Vec<u8>, data: &SpanData) {
    let context = &data.context;
    bytes(out, span::TRACE_ID, &context.trace_id.to_be_bytes());
    bytes(out, span::SPAN_ID, &context.span_id.to_be_bytes());
    if data.parent_span_id != 0 {
        bytes(
            out,
            span::PARENT_SPAN_ID,
            &data.parent_span_id.to_be_bytes(),
        );
    }
    string(out, span::NAME, &data.name);
    enumeration(out, span::KIND, otlp::kind(data.kind));
    fixed64(out, span::START_TIME_UNIX_NANO, unix_nanos(data.start_time));
    fixed64(out, span::END_TIME_UNIX_NANO, unix_nanos(data.end_time));
    attributes(out, span::ATTRIBUTES, &data.attributes);
    uint32(
        out,
        span::DROPPED_ATTRIBUTES_COUNT,
        data.dropped_attributes_count,
    );
    for held in &data.events {
        message(out, span::EVENTS, |out| {
            fixed64(out, event::TIME_UNIX_NANO, unix_nanos(held.timestamp));
            string(out, event::NAME, &held.name);
            attributes(out, event::ATTRIBUTES, &held.attributes);
            uint32(
                out,
                event::DROPPED_ATTRIBUTES_COUNT,
                held.dropped_attributes_count,
            );
        });
    }
    uint32(out, span::DROPPED_EVENTS_COUNT, data.dropped_events_count);
    for linked in &data.links {
        link(out, linked);
    }
    uint32(out, span::DROPPED_LINKS_COUNT, data.dropped_links_count);
    let (description, code) = otlp::status(&data.status);
    message(out, span::STATUS, |out| {
        string(out, status::MESSAGE, description);
        enumeration(out, status::CODE, code);
    });
    fixed32(out, span::FLAGS, otlp::flags(context));
}

pub(crate) fn metrics(out: &mut Vec<u8>, metrics: &ResourceMetrics<'_>) {
    message(out, request::RESOURCE, |out| {
        message(out, resource_group::RESOURCE, |out| {
            attributes(out, resource::ATTRIBUTES, &metrics.resource.attributes);
        });
        message(out, resource_group::SCOPES, |out| {
            message(out, scope_group::SCOPE, |out| scope(out, metrics.scope));
            for held in &metrics.metrics {
                message(out, scope_group::ITEMS, |out| metric(out, held));
            }
        });
        string(
            out,
            resource_group::SCHEMA_URL,
            metrics.resource.schema_url.as_deref().unwrap_or_default(),
        );
    });
}

fn metric(out: &mut Vec<u8>, held: &Metric) {
    string(out, metric::NAME, &held.name);
    match &held.data {
        MetricData::Gauge {
            points,
            start,
            time,
        } => {
            let times = (unix_nanos(*start), unix_nanos(*time));
            message(out, metric::GAUGE, |out| {
                for point in points {
                    message(out, aggregation::DATA_POINTS, |out| {
                        number_point(out, point, times);
                    });
                }
            });
        }
        MetricData::Sum {
            points,
            start,
            time,
            delta,
            monotonic,
        } => {
            let times = (unix_nanos(*start), unix_nanos(*time));
            message(out, metric::SUM, |out| {
                for point in points {
                    message(out, aggregation::DATA_POINTS, |out| {
                        number_point(out, point, times);
                    });
                }
                enumeration(
                    out,
                    aggregation::AGGREGATION_TEMPORALITY,
                    otlp::temporality(*delta),
                );
                boolean(out, aggregation::IS_MONOTONIC, *monotonic);
            });
        }
        MetricData::Histogram {
            points,
            start,
            time,
            delta,
        } => {
            let times = (unix_nanos(*start), unix_nanos(*time));
            message(out, metric::HISTOGRAM, |out| {
                for point in points {
                    message(out, aggregation::DATA_POINTS, |out| {
                        histogram_point(out, point, times);
                    });
                }
                enumeration(
                    out,
                    aggregation::AGGREGATION_TEMPORALITY,
                    otlp::temporality(*delta),
                );
            });
        }
    }
}

fn number_point(out: &mut Vec<u8>, point: &NumberPoint, (start, time): (u64, u64)) {
    fixed64(out, number_point::START_TIME_UNIX_NANO, start);
    fixed64(out, number_point::TIME_UNIX_NANO, time);
    match point.value {
        Point::Double(value) => fixed64_always(out, number_point::AS_DOUBLE, value.to_bits()),
        Point::Int(value) => fixed64_always(out, number_point::AS_INT, value as u64),
    }
    attributes(out, number_point::ATTRIBUTES, &point.attributes);
    uint32(out, number_point::FLAGS, otlp::DATA_POINT_FLAGS_NONE);
}

fn histogram_point(out: &mut Vec<u8>, point: &HistogramPoint, (start, time): (u64, u64)) {
    fixed64(out, histogram_point::START_TIME_UNIX_NANO, start);
    fixed64(out, histogram_point::TIME_UNIX_NANO, time);
    fixed64(out, histogram_point::COUNT, point.count);
    fixed64_always(out, histogram_point::SUM, point.sum.to_bits());
    packed(
        out,
        histogram_point::BUCKET_COUNTS,
        point.bucket_counts.iter(),
        |out, count| out.extend_from_slice(&count.to_le_bytes()),
    );
    packed(
        out,
        histogram_point::EXPLICIT_BOUNDS,
        HISTOGRAM_BOUNDS.iter(),
        |out, bound| out.extend_from_slice(&bound.to_bits().to_le_bytes()),
    );
    attributes(out, histogram_point::ATTRIBUTES, &point.attributes);
    uint32(out, histogram_point::FLAGS, otlp::DATA_POINT_FLAGS_NONE);
    fixed64_always(out, histogram_point::MIN, point.min.to_bits());
    fixed64_always(out, histogram_point::MAX, point.max.to_bits());
}
