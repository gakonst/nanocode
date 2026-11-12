use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    metrics::{
        HISTOGRAM_BOUNDS, HistogramPoint, Metric, MetricData, NumberPoint, Point, ResourceMetrics,
    },
    model::{Event, KeyValue, Resource, Scope, SpanContext, SpanData, SpanKind, Status, Value},
};

pub(crate) const SPAN_KIND_INTERNAL: i32 = 1;
pub(crate) const SPAN_KIND_SERVER: i32 = 2;
pub(crate) const SPAN_KIND_CLIENT: i32 = 3;
pub(crate) const SPAN_KIND_PRODUCER: i32 = 4;
pub(crate) const SPAN_KIND_CONSUMER: i32 = 5;
pub(crate) const STATUS_CODE_UNSET: i32 = 0;
pub(crate) const STATUS_CODE_OK: i32 = 1;
pub(crate) const STATUS_CODE_ERROR: i32 = 2;
pub(crate) const SPAN_FLAGS_SAMPLED: u32 = 0x1;
pub(crate) const SPAN_FLAGS_CONTEXT_HAS_IS_REMOTE_MASK: u32 = 0x100;
pub(crate) const AGGREGATION_TEMPORALITY_DELTA: i32 = 1;
pub(crate) const AGGREGATION_TEMPORALITY_CUMULATIVE: i32 = 2;
pub(crate) const DATA_POINT_FLAGS_NONE: u32 = 0;

pub(crate) const fn kind(kind: SpanKind) -> i32 {
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
        Status::Error(description) => (description, STATUS_CODE_ERROR),
        Status::Ok => ("", STATUS_CODE_OK),
    }
}

pub(crate) const fn flags(context: &SpanContext) -> u32 {
    if context.sampled {
        SPAN_FLAGS_SAMPLED | SPAN_FLAGS_CONTEXT_HAS_IS_REMOTE_MASK
    } else {
        SPAN_FLAGS_CONTEXT_HAS_IS_REMOTE_MASK
    }
}

pub(crate) const fn temporality(delta: bool) -> i32 {
    if delta {
        AGGREGATION_TEMPORALITY_DELTA
    } else {
        AGGREGATION_TEMPORALITY_CUMULATIVE
    }
}

pub(crate) fn unix_nanos(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64)
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
            if integers && let Some(value) = unsigned_integer(value) {
                whole(json, value);
            } else {
                text(json, value);
            }
        }
        Value::Strings(values) => {
            json.extend_from_slice(br#"{"arrayValue":{"values":"#);
            listed(json, values, |json, value| {
                text(json, value);
                Ok(())
            })?;
            json.extend_from_slice(b"}}");
        }
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
        string(json, &pair.key);
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

fn link(json: &mut Vec<u8>, context: &SpanContext) {
    json.extend_from_slice(br#"{"traceId":"#);
    hex(json, &context.trace_id.to_be_bytes());
    json.extend_from_slice(br#","spanId":"#);
    hex(json, &context.span_id.to_be_bytes());
    json.extend_from_slice(
        br#","traceState":"","attributes":[],"droppedAttributesCount":0,"flags":"#,
    );
    decimal(json, u64::from(flags(context)));
    json.push(b'}');
}

pub(crate) fn span(data: &SpanData, json: &mut Vec<u8>) -> serde_json::Result<()> {
    let context = &data.context;
    let (message, code) = status(&data.status);
    json.extend_from_slice(br#"{"traceId":"#);
    hex(json, &context.trace_id.to_be_bytes());
    json.extend_from_slice(br#","spanId":"#);
    hex(json, &context.span_id.to_be_bytes());
    json.extend_from_slice(br#","traceState":"","parentSpanId":"#);
    if data.parent_span_id == 0 {
        hex(json, &[]);
    } else {
        hex(json, &data.parent_span_id.to_be_bytes());
    }
    json.extend_from_slice(br#","flags":"#);
    decimal(json, u64::from(flags(context)));
    json.extend_from_slice(br#","name":"#);
    string(json, &data.name);
    json.extend_from_slice(br#","kind":"#);
    integer(json, i64::from(kind(data.kind)));
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
        data.dropped_events_count,
    );
    json.extend_from_slice(br#","links":"#);
    listed(json, data.links.iter(), |json, context| {
        link(json, context);
        Ok(())
    })?;
    count(json, br#","droppedLinksCount":"#, data.dropped_links_count);
    json.extend_from_slice(br#","status":{"message":"#);
    string(json, message);
    json.extend_from_slice(br#","code":"#);
    integer(json, i64::from(code));
    json.extend_from_slice(b"}}");
    Ok(())
}

fn resource(json: &mut Vec<u8>, resource: &Resource) -> serde_json::Result<()> {
    json.extend_from_slice(br#"{"attributes":"#);
    attributes(json, &resource.attributes, false)?;
    json.extend_from_slice(br#","droppedAttributesCount":0,"entityRefs":[]}"#);
    Ok(())
}

fn scope(json: &mut Vec<u8>, scope: &Scope) {
    json.extend_from_slice(br#"{"name":"#);
    string(json, &scope.name);
    json.extend_from_slice(br#","version":"#);
    string(json, scope.version.as_deref().unwrap_or_default());
    json.extend_from_slice(br#","attributes":[],"droppedAttributesCount":0}"#);
}

#[derive(Debug)]
pub(crate) struct Origin {
    resource: Vec<u8>,
    schema_url: Vec<u8>,
    scope: Vec<u8>,
}

impl Origin {
    pub(crate) fn of(held: &Resource, held_scope: &Scope) -> serde_json::Result<Self> {
        let mut json = Vec::new();
        resource(&mut json, held)?;
        let mut schema_url = Vec::new();
        string(
            &mut schema_url,
            held.schema_url.as_deref().unwrap_or_default(),
        );
        let mut encoded_scope = Vec::new();
        scope(&mut encoded_scope, held_scope);
        Ok(Self {
            resource: json,
            schema_url,
            scope: encoded_scope,
        })
    }
}

#[derive(Default)]
pub(crate) struct Batch {
    spans: Vec<u8>,
}

impl Batch {
    pub(crate) const fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    pub(crate) fn push(&mut self, span: &[u8]) {
        if !self.spans.is_empty() {
            self.spans.push(b',');
        }
        self.spans.extend_from_slice(span);
    }

    pub(crate) fn line(&mut self, origin: &Origin, line: &mut Vec<u8>) {
        line.extend_from_slice(br#"{"resourceSpans":[{"resource":"#);
        line.extend_from_slice(&origin.resource);
        line.extend_from_slice(br#","scopeSpans":[{"scope":"#);
        line.extend_from_slice(&origin.scope);
        line.extend_from_slice(br#","spans":["#);
        line.extend_from_slice(&self.spans);
        line.extend_from_slice(br#"],"schemaUrl":""}],"schemaUrl":"#);
        line.extend_from_slice(&origin.schema_url);
        line.extend_from_slice(b"}]}\n");
        self.spans.clear();
    }
}

pub(crate) fn metrics(metrics: &ResourceMetrics<'_>, json: &mut Vec<u8>) -> serde_json::Result<()> {
    json.extend_from_slice(br#"{"resourceMetrics":[{"resource":"#);
    resource(json, metrics.resource)?;
    json.extend_from_slice(br#","scopeMetrics":[{"scope":"#);
    scope(json, metrics.scope);
    json.extend_from_slice(br#","metrics":"#);
    listed(json, &metrics.metrics, metric)?;
    json.extend_from_slice(br#","schemaUrl":""}],"schemaUrl":"#);
    string(
        json,
        metrics.resource.schema_url.as_deref().unwrap_or_default(),
    );
    json.extend_from_slice(b"}]}");
    Ok(())
}

fn metric(json: &mut Vec<u8>, metric: &Metric) -> serde_json::Result<()> {
    json.extend_from_slice(br#"{"name":"#);
    string(json, &metric.name);
    json.extend_from_slice(br#","description":"","unit":"","metadata":[],"#);
    match &metric.data {
        MetricData::Gauge {
            points,
            start,
            time,
        } => {
            let span = (unix_nanos(*start), unix_nanos(*time));
            json.extend_from_slice(br#""gauge":{"dataPoints":"#);
            listed(json, points, |json, point| number_point(json, point, span))?;
            json.push(b'}');
        }
        MetricData::Sum {
            points,
            start,
            time,
            delta,
            monotonic,
        } => {
            let span = (unix_nanos(*start), unix_nanos(*time));
            json.extend_from_slice(br#""sum":{"dataPoints":"#);
            listed(json, points, |json, point| number_point(json, point, span))?;
            json.extend_from_slice(br#","aggregationTemporality":"#);
            integer(json, i64::from(temporality(*delta)));
            json.extend_from_slice(if *monotonic {
                br#","isMonotonic":true}"#
            } else {
                br#","isMonotonic":false}"#
            });
        }
        MetricData::Histogram {
            points,
            start,
            time,
            delta,
        } => {
            let span = (unix_nanos(*start), unix_nanos(*time));
            json.extend_from_slice(br#""histogram":{"dataPoints":"#);
            listed(json, points, |json, point| {
                histogram_point(json, point, span)
            })?;
            json.extend_from_slice(br#","aggregationTemporality":"#);
            integer(json, i64::from(temporality(*delta)));
            json.push(b'}');
        }
    }
    json.push(b'}');
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

fn number_point(
    json: &mut Vec<u8>,
    point: &NumberPoint,
    span: (u64, u64),
) -> serde_json::Result<()> {
    json.extend_from_slice(br#"{"attributes":"#);
    attributes(json, &point.attributes, false)?;
    times(json, span);
    json.extend_from_slice(br#","exemplars":[],"flags":"#);
    decimal(json, u64::from(DATA_POINT_FLAGS_NONE));
    json.push(b',');
    point_value(json, point.value)?;
    json.push(b'}');
    Ok(())
}

fn histogram_point(
    json: &mut Vec<u8>,
    point: &HistogramPoint,
    span: (u64, u64),
) -> serde_json::Result<()> {
    json.extend_from_slice(br#"{"attributes":"#);
    attributes(json, &point.attributes, false)?;
    times(json, span);
    json.extend_from_slice(br#","count":"#);
    quoted(json, point.count);
    json.extend_from_slice(br#","sum":"#);
    float(json, point.sum)?;
    json.extend_from_slice(br#","bucketCounts":"#);
    listed(json, &point.bucket_counts, |json, count| {
        quoted(json, *count);
        Ok(())
    })?;
    json.extend_from_slice(br#","explicitBounds":"#);
    listed(json, &HISTOGRAM_BOUNDS, |json, bound| float(json, *bound))?;
    json.extend_from_slice(br#","exemplars":[],"flags":"#);
    decimal(json, u64::from(DATA_POINT_FLAGS_NONE));
    json.extend_from_slice(br#","min":"#);
    float(json, point.min)?;
    json.extend_from_slice(br#","max":"#);
    float(json, point.max)?;
    json.push(b'}');
    Ok(())
}
