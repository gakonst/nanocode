use std::time::{SystemTime, UNIX_EPOCH};

use opentelemetry::{
    Array, InstrumentationScope, Key, KeyValue, Value,
    trace::{Event, Link, SpanContext, SpanId, SpanKind, Status},
};
use opentelemetry_sdk::{
    Resource,
    metrics::{
        Temporality,
        data::{
            AggregatedMetrics, Exemplar, ExponentialBucket, ExponentialHistogram,
            ExponentialHistogramDataPoint, Histogram, HistogramDataPoint, Metric, MetricData,
            ResourceMetrics,
        },
    },
    trace::SpanData,
};

pub(crate) const SPAN_KIND_INTERNAL: i32 = 1;
pub(crate) const SPAN_KIND_SERVER: i32 = 2;
pub(crate) const SPAN_KIND_CLIENT: i32 = 3;
pub(crate) const SPAN_KIND_PRODUCER: i32 = 4;
pub(crate) const SPAN_KIND_CONSUMER: i32 = 5;
pub(crate) const STATUS_CODE_UNSET: i32 = 0;
pub(crate) const STATUS_CODE_OK: i32 = 1;
pub(crate) const STATUS_CODE_ERROR: i32 = 2;
pub(crate) const SPAN_FLAGS_CONTEXT_HAS_IS_REMOTE_MASK: u32 = 0x100;
pub(crate) const SPAN_FLAGS_CONTEXT_IS_REMOTE_MASK: u32 = 0x200;
pub(crate) const AGGREGATION_TEMPORALITY_DELTA: i32 = 1;
pub(crate) const AGGREGATION_TEMPORALITY_CUMULATIVE: i32 = 2;
pub(crate) const DATA_POINT_FLAGS_NONE: u32 = 0;

pub(crate) const fn kind(kind: &SpanKind) -> i32 {
    match kind {
        SpanKind::Client => SPAN_KIND_CLIENT,
        SpanKind::Consumer => SPAN_KIND_CONSUMER,
        SpanKind::Internal => SPAN_KIND_INTERNAL,
        SpanKind::Producer => SPAN_KIND_PRODUCER,
        SpanKind::Server => SPAN_KIND_SERVER,
    }
}

pub(crate) fn status(status: &Status) -> (&str, i32) {
    match status {
        Status::Unset => ("", STATUS_CODE_UNSET),
        Status::Error { description } => (description, STATUS_CODE_ERROR),
        Status::Ok => ("", STATUS_CODE_OK),
    }
}

pub(crate) fn flags(context: &SpanContext, remote: bool) -> u32 {
    let flags = u32::from(context.trace_flags().to_u8()) | SPAN_FLAGS_CONTEXT_HAS_IS_REMOTE_MASK;
    if remote {
        flags | SPAN_FLAGS_CONTEXT_IS_REMOTE_MASK
    } else {
        flags
    }
}

pub(crate) const fn temporality(temporality: Temporality) -> i32 {
    match temporality {
        Temporality::Delta => AGGREGATION_TEMPORALITY_DELTA,
        _ => AGGREGATION_TEMPORALITY_CUMULATIVE,
    }
}

pub(crate) fn unix_nanos(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64)
}

pub(crate) const fn pair(pair: &KeyValue) -> (&Key, &Value) {
    (&pair.key, &pair.value)
}

#[derive(Clone, Copy)]
pub(crate) enum Point {
    Int(i64),
    Double(f64),
}

pub(crate) trait Number: Copy {
    fn point(self) -> Point;

    fn float(self) -> f64;
}

impl Number for u64 {
    fn point(self) -> Point {
        Point::Int(i64::try_from(self).unwrap_or_default())
    }

    fn float(self) -> f64 {
        self as f64
    }
}

impl Number for i64 {
    fn point(self) -> Point {
        Point::Int(self)
    }

    fn float(self) -> f64 {
        self as f64
    }
}

impl Number for f64 {
    fn point(self) -> Point {
        Point::Double(self)
    }

    fn float(self) -> f64 {
        self
    }
}

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

fn quoted(json: &mut Vec<u8>, value: u64) {
    json.push(b'"');
    decimal(json, value);
    json.push(b'"');
}

fn nanos(json: &mut Vec<u8>, time: SystemTime) {
    quoted(json, unix_nanos(time));
}

fn float(json: &mut Vec<u8>, value: f64) -> serde_json::Result<()> {
    serde_json::to_writer(&mut *json, &value)
}

fn optional_float(json: &mut Vec<u8>, value: Option<f64>) -> serde_json::Result<()> {
    match value {
        Some(value) => float(json, value),
        None => {
            json.extend_from_slice(b"null");
            Ok(())
        }
    }
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
    float(json, value)?;
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
    pairs: impl IntoIterator<Item = (&'a Key, &'a Value)>,
    integers: bool,
) -> serde_json::Result<()> {
    listed(json, pairs, |json, (key, value)| {
        json.extend_from_slice(br#"{"key":"#);
        string(json, key.as_str());
        json.extend_from_slice(br#","value":"#);
        any(json, value, integers)?;
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
    attributes(json, event.attributes.iter().map(pair), false)?;
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
    attributes(json, link.attributes.iter().map(pair), false)?;
    count(
        json,
        br#","droppedAttributesCount":"#,
        link.dropped_attributes_count,
    );
    json.extend_from_slice(br#","flags":"#);
    decimal(json, u64::from(flags(context, context.is_remote())));
    json.push(b'}');
    Ok(())
}

pub(crate) fn span(data: &SpanData, json: &mut Vec<u8>) -> serde_json::Result<()> {
    let context = &data.span_context;
    let (message, code) = status(&data.status);
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
    decimal(json, u64::from(flags(context, data.parent_span_is_remote)));
    json.extend_from_slice(br#","name":"#);
    string(json, &data.name);
    json.extend_from_slice(br#","kind":"#);
    integer(json, i64::from(kind(&data.span_kind)));
    json.extend_from_slice(br#","startTimeUnixNano":"#);
    nanos(json, data.start_time);
    json.extend_from_slice(br#","endTimeUnixNano":"#);
    nanos(json, data.end_time);
    json.extend_from_slice(br#","attributes":"#);
    attributes(json, data.attributes.iter().map(pair), true)?;
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
    integer(json, i64::from(code));
    json.extend_from_slice(b"}}");
    Ok(())
}

fn resource(json: &mut Vec<u8>, resource: &Resource) -> serde_json::Result<()> {
    json.extend_from_slice(br#"{"attributes":"#);
    attributes(json, resource.iter(), false)?;
    json.extend_from_slice(br#","droppedAttributesCount":0,"entityRefs":[]}"#);
    Ok(())
}

fn scope(json: &mut Vec<u8>, scope: &InstrumentationScope) -> serde_json::Result<()> {
    json.extend_from_slice(br#"{"name":"#);
    string(json, scope.name());
    json.extend_from_slice(br#","version":"#);
    string(json, scope.version().unwrap_or_default());
    json.extend_from_slice(br#","attributes":"#);
    attributes(json, scope.attributes().map(pair), false)?;
    json.extend_from_slice(br#","droppedAttributesCount":0}"#);
    Ok(())
}

#[derive(Debug)]
pub(crate) struct Origin {
    resource: Vec<u8>,
    schema_url: Vec<u8>,
}

impl Origin {
    pub(crate) fn of(resource: &Resource) -> serde_json::Result<Self> {
        let mut json = Vec::new();
        self::resource(&mut json, resource)?;
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
        listed(line, self.scopes.drain(..), |line, (held, spans)| {
            line.extend_from_slice(br#"{"scope":"#);
            scope(line, &held)?;
            line.extend_from_slice(br#","spans":["#);
            line.extend_from_slice(&spans);
            line.extend_from_slice(br#"],"schemaUrl":"#);
            string(line, held.schema_url().unwrap_or_default());
            line.push(b'}');
            Ok(())
        })?;
        line.extend_from_slice(br#","schemaUrl":"#);
        line.extend_from_slice(&origin.schema_url);
        line.extend_from_slice(b"}]}\n");
        Ok(())
    }
}

pub(crate) fn metrics(metrics: &ResourceMetrics, json: &mut Vec<u8>) -> serde_json::Result<()> {
    json.extend_from_slice(br#"{"resourceMetrics":[{"resource":"#);
    resource(json, metrics.resource())?;
    json.extend_from_slice(br#","scopeMetrics":"#);
    listed(json, metrics.scope_metrics(), |json, held| {
        json.extend_from_slice(br#"{"scope":"#);
        scope(json, held.scope())?;
        json.extend_from_slice(br#","metrics":"#);
        listed(json, held.metrics(), metric)?;
        json.extend_from_slice(br#","schemaUrl":"#);
        string(json, held.scope().schema_url().unwrap_or_default());
        json.push(b'}');
        Ok(())
    })?;
    json.extend_from_slice(br#","schemaUrl":"#);
    string(json, metrics.resource().schema_url().unwrap_or_default());
    json.extend_from_slice(b"}]}");
    Ok(())
}

fn metric(json: &mut Vec<u8>, metric: &Metric) -> serde_json::Result<()> {
    json.extend_from_slice(br#"{"name":"#);
    string(json, metric.name());
    json.extend_from_slice(br#","description":"#);
    string(json, metric.description());
    json.extend_from_slice(br#","unit":"#);
    string(json, metric.unit());
    json.extend_from_slice(br#","metadata":[],"#);
    match metric.data() {
        AggregatedMetrics::F64(data) => metric_data(json, data)?,
        AggregatedMetrics::U64(data) => metric_data(json, data)?,
        AggregatedMetrics::I64(data) => metric_data(json, data)?,
    }
    json.push(b'}');
    Ok(())
}

fn metric_data<T: Number>(json: &mut Vec<u8>, data: &MetricData<T>) -> serde_json::Result<()> {
    match data {
        MetricData::Gauge(gauge) => {
            let start = gauge.start_time().map_or(0, unix_nanos);
            let time = unix_nanos(gauge.time());
            json.extend_from_slice(br#""gauge":{"dataPoints":"#);
            listed(json, gauge.data_points(), |json, point| {
                number_point(
                    json,
                    point.attributes(),
                    (start, time),
                    point.exemplars(),
                    point.value(),
                )
            })?;
            json.push(b'}');
        }
        MetricData::Sum(sum) => {
            let times = (unix_nanos(sum.start_time()), unix_nanos(sum.time()));
            json.extend_from_slice(br#""sum":{"dataPoints":"#);
            listed(json, sum.data_points(), |json, point| {
                number_point(
                    json,
                    point.attributes(),
                    times,
                    point.exemplars(),
                    point.value(),
                )
            })?;
            json.extend_from_slice(br#","aggregationTemporality":"#);
            integer(json, i64::from(temporality(sum.temporality())));
            json.extend_from_slice(if sum.is_monotonic() {
                br#","isMonotonic":true}"#
            } else {
                br#","isMonotonic":false}"#
            });
        }
        MetricData::Histogram(histogram) => {
            json.extend_from_slice(br#""histogram":{"dataPoints":"#);
            listed(json, histogram.data_points(), |json, point| {
                histogram_point(json, histogram, point)
            })?;
            json.extend_from_slice(br#","aggregationTemporality":"#);
            integer(json, i64::from(temporality(histogram.temporality())));
            json.push(b'}');
        }
        MetricData::ExponentialHistogram(histogram) => {
            json.extend_from_slice(br#""exponentialHistogram":{"dataPoints":"#);
            listed(json, histogram.data_points(), |json, point| {
                exponential_point(json, histogram, point)
            })?;
            json.extend_from_slice(br#","aggregationTemporality":"#);
            integer(json, i64::from(temporality(histogram.temporality())));
            json.push(b'}');
        }
    }
    Ok(())
}

fn point_value(json: &mut Vec<u8>, value: Point) -> serde_json::Result<()> {
    match value {
        Point::Double(value) => {
            json.extend_from_slice(br#""asDouble":"#);
            float(json, value)
        }
        Point::Int(value) => {
            json.extend_from_slice(br#""asInt":""#);
            integer(json, value);
            json.push(b'"');
            Ok(())
        }
    }
}

fn times(json: &mut Vec<u8>, (start, time): (u64, u64)) {
    json.extend_from_slice(br#","startTimeUnixNano":"#);
    quoted(json, start);
    json.extend_from_slice(br#","timeUnixNano":"#);
    quoted(json, time);
}

fn number_point<'a, T: Number + 'a>(
    json: &mut Vec<u8>,
    pairs: impl Iterator<Item = &'a KeyValue>,
    span: (u64, u64),
    exemplars: impl Iterator<Item = &'a Exemplar<T>>,
    value: T,
) -> serde_json::Result<()> {
    json.extend_from_slice(br#"{"attributes":"#);
    attributes(json, pairs.map(pair), false)?;
    times(json, span);
    json.extend_from_slice(br#","exemplars":"#);
    listed(json, exemplars, exemplar)?;
    json.extend_from_slice(br#","flags":"#);
    decimal(json, u64::from(DATA_POINT_FLAGS_NONE));
    json.push(b',');
    point_value(json, value.point())?;
    json.push(b'}');
    Ok(())
}

fn exemplar<T: Number>(json: &mut Vec<u8>, exemplar: &Exemplar<T>) -> serde_json::Result<()> {
    json.extend_from_slice(br#"{"filteredAttributes":"#);
    attributes(json, exemplar.filtered_attributes().map(pair), false)?;
    json.extend_from_slice(br#","timeUnixNano":"#);
    nanos(json, exemplar.time());
    json.extend_from_slice(br#","spanId":"#);
    hex(json, exemplar.span_id());
    json.extend_from_slice(br#","traceId":"#);
    hex(json, exemplar.trace_id());
    json.extend_from_slice(br#","value":{"#);
    point_value(json, exemplar.value.point())?;
    json.extend_from_slice(b"}}");
    Ok(())
}

fn histogram_point<T: Number>(
    json: &mut Vec<u8>,
    histogram: &Histogram<T>,
    point: &HistogramDataPoint<T>,
) -> serde_json::Result<()> {
    json.extend_from_slice(br#"{"attributes":"#);
    attributes(json, point.attributes().map(pair), false)?;
    times(
        json,
        (
            unix_nanos(histogram.start_time()),
            unix_nanos(histogram.time()),
        ),
    );
    json.extend_from_slice(br#","count":"#);
    quoted(json, point.count());
    json.extend_from_slice(br#","sum":"#);
    float(json, point.sum().float())?;
    json.extend_from_slice(br#","bucketCounts":"#);
    listed(json, point.bucket_counts(), |json, count| {
        quoted(json, count);
        Ok(())
    })?;
    json.extend_from_slice(br#","explicitBounds":"#);
    listed(json, point.bounds(), float)?;
    json.extend_from_slice(br#","exemplars":"#);
    listed(json, point.exemplars(), exemplar)?;
    json.extend_from_slice(br#","flags":"#);
    decimal(json, u64::from(DATA_POINT_FLAGS_NONE));
    json.extend_from_slice(br#","min":"#);
    optional_float(json, point.min().map(Number::float))?;
    json.extend_from_slice(br#","max":"#);
    optional_float(json, point.max().map(Number::float))?;
    json.push(b'}');
    Ok(())
}

fn buckets(json: &mut Vec<u8>, name: &[u8], buckets: &ExponentialBucket) -> serde_json::Result<()> {
    json.extend_from_slice(name);
    json.extend_from_slice(br#"{"offset":"#);
    integer(json, i64::from(buckets.offset()));
    json.extend_from_slice(br#","bucketCounts":"#);
    listed(json, buckets.counts(), |json, count| {
        quoted(json, count);
        Ok(())
    })?;
    json.push(b'}');
    Ok(())
}

fn exponential_point<T: Number>(
    json: &mut Vec<u8>,
    histogram: &ExponentialHistogram<T>,
    point: &ExponentialHistogramDataPoint<T>,
) -> serde_json::Result<()> {
    json.extend_from_slice(br#"{"attributes":"#);
    attributes(json, point.attributes().map(pair), false)?;
    times(
        json,
        (
            unix_nanos(histogram.start_time()),
            unix_nanos(histogram.time()),
        ),
    );
    json.extend_from_slice(br#","count":"#);
    quoted(json, point.count() as u64);
    json.extend_from_slice(br#","sum":"#);
    float(json, point.sum().float())?;
    json.extend_from_slice(br#","scale":"#);
    integer(json, i64::from(point.scale()));
    json.extend_from_slice(br#","zeroCount":"#);
    quoted(json, point.zero_count());
    buckets(json, br#","positive":"#, point.positive_bucket())?;
    buckets(json, br#","negative":"#, point.negative_bucket())?;
    json.extend_from_slice(br#","flags":"#);
    decimal(json, u64::from(DATA_POINT_FLAGS_NONE));
    json.extend_from_slice(br#","exemplars":"#);
    listed(json, point.exemplars(), exemplar)?;
    json.extend_from_slice(br#","min":"#);
    optional_float(json, point.min().map(Number::float))?;
    json.extend_from_slice(br#","max":"#);
    optional_float(json, point.max().map(Number::float))?;
    json.extend_from_slice(br#","zeroThreshold":"#);
    float(json, point.zero_threshold())?;
    json.push(b'}');
    Ok(())
}
