use std::fmt;

use opentelemetry::{Array, InstrumentationScope, Key, KeyValue, Value, trace::SpanId};
use opentelemetry_sdk::{
    Resource,
    metrics::data::{
        AggregatedMetrics, Exemplar, ExponentialBucket, ExponentialHistogram,
        ExponentialHistogramDataPoint, Histogram, HistogramDataPoint, Metric, MetricData,
        ResourceMetrics, ScopeMetrics,
    },
    trace::SpanData,
};

use crate::otlp::{self, Number, Point, pair, unix_nanos};

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
    pub const SCHEMA_URL: u32 = 3;
}

mod resource {
    pub const ATTRIBUTES: u32 = 1;
}

mod scope {
    pub const NAME: u32 = 1;
    pub const VERSION: u32 = 2;
    pub const ATTRIBUTES: u32 = 3;
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
    pub const TRACE_STATE: u32 = 3;
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
    pub const TRACE_STATE: u32 = 3;
    pub const ATTRIBUTES: u32 = 4;
    pub const DROPPED_ATTRIBUTES_COUNT: u32 = 5;
    pub const FLAGS: u32 = 6;
}

mod status {
    pub const MESSAGE: u32 = 2;
    pub const CODE: u32 = 3;
}

mod metric {
    pub const NAME: u32 = 1;
    pub const DESCRIPTION: u32 = 2;
    pub const UNIT: u32 = 3;
    pub const GAUGE: u32 = 5;
    pub const SUM: u32 = 7;
    pub const HISTOGRAM: u32 = 9;
    pub const EXPONENTIAL_HISTOGRAM: u32 = 10;
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
    pub const EXEMPLARS: u32 = 5;
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
    pub const EXEMPLARS: u32 = 8;
    pub const ATTRIBUTES: u32 = 9;
    pub const FLAGS: u32 = 10;
    pub const MIN: u32 = 11;
    pub const MAX: u32 = 12;
}

mod exponential_point {
    pub const ATTRIBUTES: u32 = 1;
    pub const START_TIME_UNIX_NANO: u32 = 2;
    pub const TIME_UNIX_NANO: u32 = 3;
    pub const COUNT: u32 = 4;
    pub const SUM: u32 = 5;
    pub const SCALE: u32 = 6;
    pub const ZERO_COUNT: u32 = 7;
    pub const POSITIVE: u32 = 8;
    pub const NEGATIVE: u32 = 9;
    pub const FLAGS: u32 = 10;
    pub const EXEMPLARS: u32 = 11;
    pub const MIN: u32 = 12;
    pub const MAX: u32 = 13;
    pub const ZERO_THRESHOLD: u32 = 14;
}

mod buckets {
    pub const OFFSET: u32 = 1;
    pub const BUCKET_COUNTS: u32 = 2;
}

mod exemplar {
    pub const TIME_UNIX_NANO: u32 = 2;
    pub const AS_DOUBLE: u32 = 3;
    pub const SPAN_ID: u32 = 4;
    pub const TRACE_ID: u32 = 5;
    pub const AS_INT: u32 = 6;
    pub const FILTERED_ATTRIBUTES: u32 = 7;
}

#[derive(Debug)]
pub(crate) struct Undefined(&'static str);

impl fmt::Display for Undefined {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} OTLP does not define", self.0)
    }
}

type Encoded = Result<(), Undefined>;

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

fn message(out: &mut Vec<u8>, field: u32, body: impl FnOnce(&mut Vec<u8>) -> Encoded) -> Encoded {
    key(out, field, WIRE_LENGTH_DELIMITED);
    let start = out.len();
    body(out)?;
    let length = out.len() - start;
    let (prefix, prefix_length) = varint_bytes(length as u64);
    out.extend_from_slice(&prefix[..prefix_length]);
    out.copy_within(start..start + length, start + prefix_length);
    out[start..start + prefix_length].copy_from_slice(&prefix[..prefix_length]);
    assert_eq!(out.len(), start + prefix_length + length);
    Ok(())
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

fn sint32(out: &mut Vec<u8>, field: u32, value: i32) {
    if value != 0 {
        key(out, field, WIRE_VARINT);
        varint(
            out,
            u64::from(((value << 1) ^ (value >> (i32::BITS - 1))) as u32),
        );
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

fn double(out: &mut Vec<u8>, field: u32, value: f64) {
    if value != 0.0 {
        fixed64_always(out, field, value.to_bits());
    }
}

fn optional_double(out: &mut Vec<u8>, field: u32, value: Option<f64>) {
    if let Some(value) = value {
        fixed64_always(out, field, value.to_bits());
    }
}

fn packed<T>(
    out: &mut Vec<u8>,
    field: u32,
    values: impl Iterator<Item = T>,
    mut each: impl FnMut(&mut Vec<u8>, T),
) -> Encoded {
    let mut values = values.peekable();
    if values.peek().is_none() {
        return Ok(());
    }
    message(out, field, |out| {
        values.for_each(|value| each(out, value));
        Ok(())
    })
}

fn element(out: &mut Vec<u8>, write: impl FnOnce(&mut Vec<u8>)) -> Encoded {
    message(out, array_value::VALUES, |out| {
        write(out);
        Ok(())
    })
}

fn any_bool(out: &mut Vec<u8>, value: bool) {
    key(out, any_value::BOOL, WIRE_VARINT);
    varint(out, u64::from(value));
}

fn any_int(out: &mut Vec<u8>, value: i64) {
    key(out, any_value::INT, WIRE_VARINT);
    varint(out, value as u64);
}

fn any_double(out: &mut Vec<u8>, value: f64) {
    fixed64_always(out, any_value::DOUBLE, value.to_bits());
}

fn any_string(out: &mut Vec<u8>, value: &str) {
    length_delimited(out, any_value::STRING, value.as_bytes());
}

fn any(out: &mut Vec<u8>, value: &Value) -> Encoded {
    match value {
        Value::Bool(value) => any_bool(out, *value),
        Value::I64(value) => any_int(out, *value),
        Value::F64(value) => any_double(out, *value),
        Value::String(value) => any_string(out, value.as_str()),
        Value::Array(values) => {
            message(out, any_value::ARRAY, |out| match values {
                Array::Bool(values) => values
                    .iter()
                    .try_for_each(|value| element(out, |out| any_bool(out, *value))),
                Array::I64(values) => values
                    .iter()
                    .try_for_each(|value| element(out, |out| any_int(out, *value))),
                Array::F64(values) => values
                    .iter()
                    .try_for_each(|value| element(out, |out| any_double(out, *value))),
                Array::String(values) => values
                    .iter()
                    .try_for_each(|value| element(out, |out| any_string(out, value.as_str()))),
                _ => Err(Undefined("an attribute array")),
            })?;
        }
        _ => return Err(Undefined("an attribute value")),
    }
    Ok(())
}

fn attributes<'a>(
    out: &mut Vec<u8>,
    field: u32,
    pairs: impl IntoIterator<Item = (&'a Key, &'a Value)>,
) -> Encoded {
    pairs.into_iter().try_for_each(|(name, value)| {
        message(out, field, |out| {
            string(out, key_value::KEY, name.as_str());
            message(out, key_value::VALUE, |out| any(out, value))
        })
    })
}

fn scope(out: &mut Vec<u8>, held: &InstrumentationScope) -> Encoded {
    string(out, scope::NAME, held.name());
    string(out, scope::VERSION, held.version().unwrap_or_default());
    attributes(out, scope::ATTRIBUTES, held.attributes().map(pair))
}

#[derive(Debug, Default)]
pub(crate) struct Origin {
    resource: Vec<u8>,
    schema_url: String,
}

impl Origin {
    pub(crate) fn of(held: &Resource) -> Result<Self, Undefined> {
        let mut encoded = Vec::new();
        attributes(&mut encoded, resource::ATTRIBUTES, held.iter())?;
        Ok(Self {
            resource: encoded,
            schema_url: held.schema_url().unwrap_or_default().to_owned(),
        })
    }
}

pub(crate) fn traces(out: &mut Vec<u8>, origin: &Origin, batch: &[SpanData]) -> Encoded {
    let mut scopes: Vec<&InstrumentationScope> = Vec::new();
    let mut groups = Vec::with_capacity(batch.len());
    for data in batch {
        let group = match scopes
            .iter()
            .position(|held| **held == data.instrumentation_scope)
        {
            Some(group) => group,
            None => {
                scopes.push(&data.instrumentation_scope);
                scopes.len() - 1
            }
        };
        groups.push(group);
    }
    assert_eq!(groups.len(), batch.len());
    message(out, request::RESOURCE, |out| {
        message(out, resource_group::RESOURCE, |out| {
            out.extend_from_slice(&origin.resource);
            Ok(())
        })?;
        for (group, held) in scopes.into_iter().enumerate() {
            message(out, resource_group::SCOPES, |out| {
                message(out, scope_group::SCOPE, |out| scope(out, held))?;
                batch
                    .iter()
                    .zip(&groups)
                    .filter(|(_, member)| **member == group)
                    .try_for_each(|(data, _)| {
                        message(out, scope_group::ITEMS, |out| span(out, data))
                    })?;
                string(
                    out,
                    scope_group::SCHEMA_URL,
                    held.schema_url().unwrap_or_default(),
                );
                Ok(())
            })?;
        }
        string(out, resource_group::SCHEMA_URL, &origin.schema_url);
        Ok(())
    })
}

fn span(out: &mut Vec<u8>, data: &SpanData) -> Encoded {
    let context = &data.span_context;
    bytes(out, span::TRACE_ID, &context.trace_id().to_bytes());
    bytes(out, span::SPAN_ID, &context.span_id().to_bytes());
    string(out, span::TRACE_STATE, &context.trace_state().header());
    if data.parent_span_id != SpanId::INVALID {
        bytes(out, span::PARENT_SPAN_ID, &data.parent_span_id.to_bytes());
    }
    string(out, span::NAME, &data.name);
    enumeration(out, span::KIND, otlp::kind(&data.span_kind));
    fixed64(out, span::START_TIME_UNIX_NANO, unix_nanos(data.start_time));
    fixed64(out, span::END_TIME_UNIX_NANO, unix_nanos(data.end_time));
    attributes(out, span::ATTRIBUTES, data.attributes.iter().map(pair))?;
    uint32(
        out,
        span::DROPPED_ATTRIBUTES_COUNT,
        data.dropped_attributes_count,
    );
    data.events.iter().try_for_each(|held| {
        message(out, span::EVENTS, |out| {
            fixed64(out, event::TIME_UNIX_NANO, unix_nanos(held.timestamp));
            string(out, event::NAME, &held.name);
            attributes(out, event::ATTRIBUTES, held.attributes.iter().map(pair))?;
            uint32(
                out,
                event::DROPPED_ATTRIBUTES_COUNT,
                held.dropped_attributes_count,
            );
            Ok(())
        })
    })?;
    uint32(out, span::DROPPED_EVENTS_COUNT, data.events.dropped_count);
    data.links.iter().try_for_each(|held| {
        let linked = &held.span_context;
        message(out, span::LINKS, |out| {
            bytes(out, link::TRACE_ID, &linked.trace_id().to_bytes());
            bytes(out, link::SPAN_ID, &linked.span_id().to_bytes());
            string(out, link::TRACE_STATE, &linked.trace_state().header());
            attributes(out, link::ATTRIBUTES, held.attributes.iter().map(pair))?;
            uint32(
                out,
                link::DROPPED_ATTRIBUTES_COUNT,
                held.dropped_attributes_count,
            );
            fixed32(out, link::FLAGS, otlp::flags(linked, linked.is_remote()));
            Ok(())
        })
    })?;
    uint32(out, span::DROPPED_LINKS_COUNT, data.links.dropped_count);
    let (description, code) = otlp::status(&data.status);
    message(out, span::STATUS, |out| {
        string(out, status::MESSAGE, description);
        enumeration(out, status::CODE, code);
        Ok(())
    })?;
    fixed32(
        out,
        span::FLAGS,
        otlp::flags(context, data.parent_span_is_remote),
    );
    Ok(())
}

pub(crate) fn metrics(out: &mut Vec<u8>, metrics: &ResourceMetrics) -> Encoded {
    let held = metrics.resource();
    message(out, request::RESOURCE, |out| {
        message(out, resource_group::RESOURCE, |out| {
            attributes(out, resource::ATTRIBUTES, held.iter())
        })?;
        metrics.scope_metrics().try_for_each(|group| {
            message(out, resource_group::SCOPES, |out| scope_metrics(out, group))
        })?;
        string(
            out,
            resource_group::SCHEMA_URL,
            held.schema_url().unwrap_or_default(),
        );
        Ok(())
    })
}

fn scope_metrics(out: &mut Vec<u8>, group: &ScopeMetrics) -> Encoded {
    message(out, scope_group::SCOPE, |out| scope(out, group.scope()))?;
    group
        .metrics()
        .try_for_each(|held| message(out, scope_group::ITEMS, |out| metric(out, held)))?;
    string(
        out,
        scope_group::SCHEMA_URL,
        group.scope().schema_url().unwrap_or_default(),
    );
    Ok(())
}

fn metric(out: &mut Vec<u8>, held: &Metric) -> Encoded {
    string(out, metric::NAME, held.name());
    string(out, metric::DESCRIPTION, held.description());
    string(out, metric::UNIT, held.unit());
    match held.data() {
        AggregatedMetrics::F64(data) => metric_data(out, data),
        AggregatedMetrics::U64(data) => metric_data(out, data),
        AggregatedMetrics::I64(data) => metric_data(out, data),
    }
}

fn metric_data<T: Number>(out: &mut Vec<u8>, data: &MetricData<T>) -> Encoded {
    match data {
        MetricData::Gauge(gauge) => {
            let times = (
                gauge.start_time().map_or(0, unix_nanos),
                unix_nanos(gauge.time()),
            );
            message(out, metric::GAUGE, |out| {
                gauge.data_points().try_for_each(|point| {
                    message(out, aggregation::DATA_POINTS, |out| {
                        number_point(
                            out,
                            point.attributes(),
                            times,
                            point.exemplars(),
                            point.value(),
                        )
                    })
                })
            })
        }
        MetricData::Sum(sum) => {
            let times = (unix_nanos(sum.start_time()), unix_nanos(sum.time()));
            message(out, metric::SUM, |out| {
                sum.data_points().try_for_each(|point| {
                    message(out, aggregation::DATA_POINTS, |out| {
                        number_point(
                            out,
                            point.attributes(),
                            times,
                            point.exemplars(),
                            point.value(),
                        )
                    })
                })?;
                enumeration(
                    out,
                    aggregation::AGGREGATION_TEMPORALITY,
                    otlp::temporality(sum.temporality()),
                );
                boolean(out, aggregation::IS_MONOTONIC, sum.is_monotonic());
                Ok(())
            })
        }
        MetricData::Histogram(histogram) => message(out, metric::HISTOGRAM, |out| {
            histogram.data_points().try_for_each(|point| {
                message(out, aggregation::DATA_POINTS, |out| {
                    histogram_point(out, histogram, point)
                })
            })?;
            enumeration(
                out,
                aggregation::AGGREGATION_TEMPORALITY,
                otlp::temporality(histogram.temporality()),
            );
            Ok(())
        }),
        MetricData::ExponentialHistogram(histogram) => {
            message(out, metric::EXPONENTIAL_HISTOGRAM, |out| {
                histogram.data_points().try_for_each(|point| {
                    message(out, aggregation::DATA_POINTS, |out| {
                        exponential_point(out, histogram, point)
                    })
                })?;
                enumeration(
                    out,
                    aggregation::AGGREGATION_TEMPORALITY,
                    otlp::temporality(histogram.temporality()),
                );
                Ok(())
            })
        }
    }
}

fn point_value(out: &mut Vec<u8>, value: Point, (double, int): (u32, u32)) {
    match value {
        Point::Double(value) => fixed64_always(out, double, value.to_bits()),
        Point::Int(value) => fixed64_always(out, int, value as u64),
    }
}

fn number_point<'a, T: Number + 'a>(
    out: &mut Vec<u8>,
    pairs: impl Iterator<Item = &'a KeyValue>,
    (start, time): (u64, u64),
    exemplars: impl Iterator<Item = &'a Exemplar<T>>,
    value: T,
) -> Encoded {
    fixed64(out, number_point::START_TIME_UNIX_NANO, start);
    fixed64(out, number_point::TIME_UNIX_NANO, time);
    point_value(
        out,
        value.point(),
        (number_point::AS_DOUBLE, number_point::AS_INT),
    );
    exemplars
        .into_iter()
        .try_for_each(|held| message(out, number_point::EXEMPLARS, |out| exemplar(out, held)))?;
    attributes(out, number_point::ATTRIBUTES, pairs.map(pair))?;
    uint32(out, number_point::FLAGS, otlp::DATA_POINT_FLAGS_NONE);
    Ok(())
}

fn exemplar<T: Number>(out: &mut Vec<u8>, held: &Exemplar<T>) -> Encoded {
    fixed64(out, exemplar::TIME_UNIX_NANO, unix_nanos(held.time()));
    point_value(
        out,
        held.value.point(),
        (exemplar::AS_DOUBLE, exemplar::AS_INT),
    );
    bytes(out, exemplar::SPAN_ID, held.span_id());
    bytes(out, exemplar::TRACE_ID, held.trace_id());
    attributes(
        out,
        exemplar::FILTERED_ATTRIBUTES,
        held.filtered_attributes().map(pair),
    )
}

fn histogram_point<T: Number>(
    out: &mut Vec<u8>,
    histogram: &Histogram<T>,
    point: &HistogramDataPoint<T>,
) -> Encoded {
    fixed64(
        out,
        histogram_point::START_TIME_UNIX_NANO,
        unix_nanos(histogram.start_time()),
    );
    fixed64(
        out,
        histogram_point::TIME_UNIX_NANO,
        unix_nanos(histogram.time()),
    );
    fixed64(out, histogram_point::COUNT, point.count());
    optional_double(out, histogram_point::SUM, Some(point.sum().float()));
    packed(
        out,
        histogram_point::BUCKET_COUNTS,
        point.bucket_counts(),
        |out, count| {
            out.extend_from_slice(&count.to_le_bytes());
        },
    )?;
    packed(
        out,
        histogram_point::EXPLICIT_BOUNDS,
        point.bounds(),
        |out, bound| {
            out.extend_from_slice(&bound.to_bits().to_le_bytes());
        },
    )?;
    point
        .exemplars()
        .try_for_each(|held| message(out, histogram_point::EXEMPLARS, |out| exemplar(out, held)))?;
    attributes(
        out,
        histogram_point::ATTRIBUTES,
        point.attributes().map(pair),
    )?;
    uint32(out, histogram_point::FLAGS, otlp::DATA_POINT_FLAGS_NONE);
    optional_double(out, histogram_point::MIN, point.min().map(Number::float));
    optional_double(out, histogram_point::MAX, point.max().map(Number::float));
    Ok(())
}

fn buckets(out: &mut Vec<u8>, field: u32, held: &ExponentialBucket) -> Encoded {
    message(out, field, |out| {
        sint32(out, buckets::OFFSET, held.offset());
        packed(out, buckets::BUCKET_COUNTS, held.counts(), varint)
    })
}

fn exponential_point<T: Number>(
    out: &mut Vec<u8>,
    histogram: &ExponentialHistogram<T>,
    point: &ExponentialHistogramDataPoint<T>,
) -> Encoded {
    attributes(
        out,
        exponential_point::ATTRIBUTES,
        point.attributes().map(pair),
    )?;
    fixed64(
        out,
        exponential_point::START_TIME_UNIX_NANO,
        unix_nanos(histogram.start_time()),
    );
    fixed64(
        out,
        exponential_point::TIME_UNIX_NANO,
        unix_nanos(histogram.time()),
    );
    fixed64(out, exponential_point::COUNT, point.count() as u64);
    optional_double(out, exponential_point::SUM, Some(point.sum().float()));
    sint32(out, exponential_point::SCALE, i32::from(point.scale()));
    fixed64(out, exponential_point::ZERO_COUNT, point.zero_count());
    buckets(out, exponential_point::POSITIVE, point.positive_bucket())?;
    buckets(out, exponential_point::NEGATIVE, point.negative_bucket())?;
    uint32(out, exponential_point::FLAGS, otlp::DATA_POINT_FLAGS_NONE);
    point.exemplars().try_for_each(|held| {
        message(out, exponential_point::EXEMPLARS, |out| exemplar(out, held))
    })?;
    optional_double(out, exponential_point::MIN, point.min().map(Number::float));
    optional_double(out, exponential_point::MAX, point.max().map(Number::float));
    double(
        out,
        exponential_point::ZERO_THRESHOLD,
        point.zero_threshold(),
    );
    Ok(())
}
