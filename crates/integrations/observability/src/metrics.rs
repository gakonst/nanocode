use std::{
    borrow::Cow,
    cell::Cell,
    env, fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    time::{Duration, Instant, SystemTime},
};

use nanocode_sync::{Atom, Map};
use tracing::{
    Event, Metadata,
    field::{Field, Visit},
    span::Attributes,
    subscriber::Interest,
};

use crate::subscriber::{Context, Filter, Layer, SpanRef};

use crate::model::{ExportError, Exported, KeyValue, Resource, Scope, Text, Value};

const METRIC_PREFIX_MONOTONIC_COUNTER: &str = "monotonic_counter.";
const METRIC_PREFIX_COUNTER: &str = "counter.";
const METRIC_PREFIX_HISTOGRAM: &str = "histogram.";
const METRIC_PREFIX_GAUGE: &str = "gauge.";
const I64_MAX: u64 = i64::MAX as u64;
const OTEL_METRIC_EXPORT_INTERVAL: &str = "OTEL_METRIC_EXPORT_INTERVAL";
const EXPORT_INTERVAL_DEFAULT: Duration = Duration::from_secs(60);
const READER_SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
const CARDINALITY_LIMIT: usize = 2_000;
const OVERFLOW_ATTRIBUTE: &str = "otel.metric.overflow";
const INSTRUMENT_NAME_BYTES_MAX: usize = 255;
const INSTRUMENT_NAME_PUNCTUATION: [char; 4] = ['_', '.', '-', '/'];
pub(crate) const HISTOGRAM_BOUNDS: [f64; 15] = [
    0.0, 5.0, 10.0, 25.0, 50.0, 75.0, 100.0, 250.0, 500.0, 750.0, 1000.0, 2500.0, 5000.0, 7500.0,
    10000.0,
];
const HISTOGRAM_BUCKETS: usize = HISTOGRAM_BOUNDS.len() + 1;
const READER_THREAD: &str = "OpenTelemetry.Metrics.PeriodicReader";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Temporality {
    #[default]
    Cumulative,
    Delta,
    LowMemory,
}

impl std::str::FromStr for Temporality {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.to_lowercase().as_str() {
            "cumulative" => Ok(Self::Cumulative),
            "delta" => Ok(Self::Delta),
            "lowmemory" => Ok(Self::LowMemory),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    Counter,
    UpDownCounter,
    Histogram,
    Gauge,
}

impl Kind {
    const fn delta(self, preference: Temporality) -> bool {
        match preference {
            Temporality::Cumulative => false,
            Temporality::Delta => !matches!(self, Self::UpDownCounter),
            Temporality::LowMemory => matches!(self, Self::Counter | Self::Histogram),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Number {
    U64,
    I64,
    F64,
}

#[derive(Clone, Copy, Debug)]
enum Measurement {
    U64(u64),
    I64(i64),
    F64(f64),
}

impl Measurement {
    const fn number(self) -> Number {
        match self {
            Self::U64(_) => Number::U64,
            Self::I64(_) => Number::I64,
            Self::F64(_) => Number::F64,
        }
    }

    const fn zero(number: Number) -> Self {
        match number {
            Number::U64 => Self::U64(0),
            Number::I64 => Self::I64(0),
            Number::F64 => Self::F64(0.0),
        }
    }

    const fn lowest(number: Number) -> Self {
        match number {
            Number::U64 => Self::U64(u64::MIN),
            Number::I64 => Self::I64(i64::MIN),
            Number::F64 => Self::F64(f64::MIN),
        }
    }

    const fn highest(number: Number) -> Self {
        match number {
            Number::U64 => Self::U64(u64::MAX),
            Number::I64 => Self::I64(i64::MAX),
            Number::F64 => Self::F64(f64::MAX),
        }
    }

    fn add(self, other: Self) -> Self {
        match (self, other) {
            (Self::U64(left), Self::U64(right)) => Self::U64(left.wrapping_add(right)),
            (Self::I64(left), Self::I64(right)) => Self::I64(left.wrapping_add(right)),
            (Self::F64(left), Self::F64(right)) => Self::F64(left + right),
            _ => unreachable!("a stream holds one number type"),
        }
    }

    fn less(self, other: Self) -> bool {
        match (self, other) {
            (Self::U64(left), Self::U64(right)) => left < right,
            (Self::I64(left), Self::I64(right)) => left < right,
            (Self::F64(left), Self::F64(right)) => left < right,
            _ => unreachable!("a stream holds one number type"),
        }
    }

    const fn float(self) -> f64 {
        match self {
            Self::U64(value) => value as f64,
            Self::I64(value) => value as f64,
            Self::F64(value) => value,
        }
    }

    pub(crate) fn point(self) -> Point {
        match self {
            Self::U64(value) => Point::Int(i64::try_from(value).unwrap_or_default()),
            Self::I64(value) => Point::Int(value),
            Self::F64(value) => Point::Double(value),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Point {
    Int(i64),
    Double(f64),
}

#[derive(Clone, Debug)]
struct Buckets {
    counts: [u64; HISTOGRAM_BUCKETS],
    count: u64,
    total: Measurement,
    min: Measurement,
    max: Measurement,
}

#[derive(Clone, Debug)]
enum Aggregate {
    Sum(Measurement),
    Gauge(Measurement),
    Histogram(Buckets),
}

impl Aggregate {
    const fn new(kind: Kind, number: Number) -> Self {
        match kind {
            Kind::Counter | Kind::UpDownCounter => Self::Sum(Measurement::zero(number)),
            Kind::Gauge => Self::Gauge(Measurement::zero(number)),
            Kind::Histogram => Self::Histogram(Buckets {
                counts: [0; HISTOGRAM_BUCKETS],
                count: 0,
                total: Measurement::zero(number),
                min: Measurement::highest(number),
                max: Measurement::lowest(number),
            }),
        }
    }

    fn update(&mut self, value: Measurement) {
        match self {
            Self::Sum(total) => *total = total.add(value),
            Self::Gauge(last) => *last = value,
            Self::Histogram(buckets) => {
                let float = value.float();
                let index = HISTOGRAM_BOUNDS.partition_point(|bound| *bound < float);
                buckets.total = buckets.total.add(value);
                buckets.count += 1;
                buckets.counts[index] += 1;
                if value.less(buckets.min) {
                    buckets.min = value;
                }
                if buckets.max.less(value) {
                    buckets.max = value;
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Tracker {
    attributes: Vec<KeyValue>,
    aggregate: Aggregate,
    updated: bool,
    sequence: u64,
}

#[derive(Clone, Debug)]
struct Values {
    empty: Tracker,
    trackers: Map<Vec<KeyValue>, Tracker>,
    count: usize,
    sequence: u64,
    started: SystemTime,
}

fn sorted(attributes: &[KeyValue]) -> Vec<KeyValue> {
    let mut sorted = attributes.to_vec();
    sorted.sort_by(|left, right| left.key.cmp(&right.key));
    sorted.dedup_by(|later, earlier| later.key == earlier.key);
    sorted
}

fn overflow() -> Vec<KeyValue> {
    vec![KeyValue::new(OVERFLOW_ATTRIBUTE, true)]
}

impl Values {
    fn new(aggregate: Aggregate) -> Self {
        Self {
            empty: Tracker {
                attributes: Vec::new(),
                aggregate,
                updated: false,
                sequence: 0,
            },
            trackers: Map::new(),
            count: 0,
            sequence: 0,
            started: SystemTime::now(),
        }
    }

    fn measure(&mut self, fresh: &Aggregate, value: Measurement, attributes: &[KeyValue]) {
        if attributes.is_empty() {
            self.empty.aggregate.update(value);
            self.empty.updated = true;
            return;
        }
        let key = sorted(attributes);
        let key = if self.trackers.contains_key(&key) || self.count < CARDINALITY_LIMIT {
            key
        } else {
            overflow()
        };
        if let Some(tracker) = self.trackers.get_mut(&key) {
            tracker.aggregate.update(value);
            tracker.updated = true;
            return;
        }
        let counted = key != overflow() || self.count < CARDINALITY_LIMIT;
        self.sequence += 1;
        let mut tracker = Tracker {
            attributes: if counted {
                attributes.to_vec()
            } else {
                overflow()
            },
            aggregate: fresh.clone(),
            updated: true,
            sequence: self.sequence,
        };
        tracker.aggregate.update(value);
        self.trackers.insert(key, tracker);
        if counted {
            self.count += 1;
        }
    }

    fn collect(
        &mut self,
        delta: bool,
        fresh: &Aggregate,
    ) -> (SystemTime, SystemTime, Vec<Tracker>) {
        let now = SystemTime::now();
        let start = self.started;
        if delta {
            self.started = now;
        }
        let mut points = Vec::new();
        if self.empty.updated {
            points.push(self.empty.clone());
            if delta {
                self.empty.updated = false;
                self.empty.aggregate = fresh.clone();
            }
        }
        let mut trackers: Vec<(&Vec<KeyValue>, &Tracker)> = self.trackers.iter().collect();
        trackers.sort_by_key(|(_, tracker)| tracker.sequence);
        let mut stale = Vec::new();
        for (key, tracker) in trackers {
            if !delta || tracker.updated {
                points.push(tracker.clone());
            } else if *key != overflow() {
                stale.push(key.clone());
            }
        }
        if delta {
            for key in &stale {
                self.trackers.remove(key);
            }
            self.count -= stale.len();
            for (_, tracker) in self.trackers.iter_mut() {
                if tracker.updated {
                    tracker.updated = false;
                    tracker.aggregate = fresh.clone();
                }
            }
        }
        (start, now, points)
    }
}

struct Stream {
    name: Text,
    kind: Kind,
    delta: bool,
    fresh: Aggregate,
    values: Atom<Values>,
}

impl Stream {
    fn measure(&self, value: Measurement, attributes: &[KeyValue]) {
        self.values
            .update(|values| values.measure(&self.fresh, value, attributes));
    }

    fn collect(&self) -> Option<Metric> {
        let (start, time, trackers) = self
            .values
            .update(|values| values.collect(self.delta, &self.fresh));
        if trackers.is_empty() {
            return None;
        }
        let number = |tracker: Tracker| match tracker.aggregate {
            Aggregate::Sum(value) | Aggregate::Gauge(value) => NumberPoint {
                attributes: tracker.attributes,
                value: value.point(),
            },
            Aggregate::Histogram(_) => unreachable!("a sum or gauge stream holds no buckets"),
        };
        let data = match self.kind {
            Kind::Counter | Kind::UpDownCounter => MetricData::Sum {
                points: trackers.into_iter().map(number).collect(),
                start,
                time,
                delta: self.delta,
                monotonic: self.kind == Kind::Counter,
            },
            Kind::Gauge => MetricData::Gauge {
                points: trackers.into_iter().map(number).collect(),
                start,
                time,
            },
            Kind::Histogram => MetricData::Histogram {
                points: trackers
                    .into_iter()
                    .map(|tracker| match tracker.aggregate {
                        Aggregate::Histogram(buckets) => HistogramPoint {
                            attributes: tracker.attributes,
                            count: buckets.count,
                            sum: buckets.total.float(),
                            bucket_counts: buckets.counts,
                            min: buckets.min.float(),
                            max: buckets.max.float(),
                        },
                        Aggregate::Sum(_) | Aggregate::Gauge(_) => {
                            unreachable!("a histogram stream holds buckets")
                        }
                    })
                    .collect(),
                start,
                time,
                delta: self.delta,
            },
        };
        Some(Metric {
            name: self.name.clone(),
            data,
        })
    }
}

pub(crate) struct NumberPoint {
    pub(crate) attributes: Vec<KeyValue>,
    pub(crate) value: Point,
}

pub(crate) struct HistogramPoint {
    pub(crate) attributes: Vec<KeyValue>,
    pub(crate) count: u64,
    pub(crate) sum: f64,
    pub(crate) bucket_counts: [u64; HISTOGRAM_BUCKETS],
    pub(crate) min: f64,
    pub(crate) max: f64,
}

pub(crate) enum MetricData {
    Sum {
        points: Vec<NumberPoint>,
        start: SystemTime,
        time: SystemTime,
        delta: bool,
        monotonic: bool,
    },
    Gauge {
        points: Vec<NumberPoint>,
        start: SystemTime,
        time: SystemTime,
    },
    Histogram {
        points: Vec<HistogramPoint>,
        start: SystemTime,
        time: SystemTime,
        delta: bool,
    },
}

pub(crate) struct Metric {
    pub(crate) name: Text,
    pub(crate) data: MetricData,
}

pub(crate) struct ResourceMetrics<'a> {
    pub(crate) resource: &'a Resource,
    pub(crate) scope: &'a Scope,
    pub(crate) metrics: Vec<Metric>,
}

pub(crate) trait MetricExporter: Send + Sync + 'static {
    fn export(&self, metrics: &ResourceMetrics<'_>) -> Exported;

    fn shutdown(&self) -> Exported;

    fn temporality(&self) -> Temporality;
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct StreamId {
    name: String,
    kind: Kind,
    number: Number,
}

#[derive(Clone, Default)]
struct Streams {
    by_id: Map<StreamId, Arc<Stream>>,
    order: Vec<Arc<Stream>>,
}

struct Collector {
    streams: Atom<Streams>,
    exporter: Box<dyn MetricExporter>,
    resource: Resource,
    scope: Scope,
}

impl Collector {
    fn stream(&self, name: &'static str, kind: Kind, number: Number) -> Arc<Stream> {
        let id = StreamId {
            name: name.to_lowercase(),
            kind,
            number,
        };
        let delta = kind.delta(self.exporter.temporality());
        self.streams.update(|streams| {
            if let Some(stream) = streams.by_id.get(&id) {
                return Arc::clone(stream);
            }
            let fresh = Aggregate::new(kind, number);
            let stream = Arc::new(Stream {
                name: Cow::Borrowed(name),
                kind,
                delta,
                values: Atom::new(Values::new(fresh.clone())),
                fresh,
            });
            streams.by_id.insert(id.clone(), Arc::clone(&stream));
            streams.order.push(Arc::clone(&stream));
            stream
        })
    }

    fn collect_and_export(&self) -> Exported {
        let streams = self.streams.get();
        let metrics: Vec<Metric> = streams
            .order
            .iter()
            .filter_map(|stream| stream.collect())
            .collect();
        if metrics.is_empty() {
            return Ok(());
        }
        self.exporter.export(&ResourceMetrics {
            resource: &self.resource,
            scope: &self.scope,
            metrics,
        })
    }

    fn run(&self, received: &Receiver<Sender<bool>>, interval: Duration) {
        super::trace::suppress_on_this_thread();
        let mut remaining = interval;
        loop {
            match received.recv_timeout(remaining) {
                Ok(reply) => {
                    let exported = self.collect_and_export();
                    let shut = self.exporter.shutdown();
                    let _ = reply.send(exported.is_ok() && shut.is_ok());
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {
                    let started = Instant::now();
                    drop(self.collect_and_export());
                    let took = started.elapsed();
                    remaining = interval.checked_sub(took).unwrap_or(Duration::ZERO);
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

struct Pipeline {
    collector: Arc<Collector>,
    shutdown: Sender<Sender<bool>>,
}

impl Pipeline {
    fn shutdown(&self) -> Exported {
        let (reply, response) = mpsc::channel();
        self.shutdown
            .send(reply)
            .map_err(|error| ExportError::InternalFailure(error.to_string()))?;
        match response.recv_timeout(READER_SHUTDOWN_WAIT) {
            Ok(true) => Ok(()),
            Ok(false) | Err(RecvTimeoutError::Disconnected) => Err(ExportError::InternalFailure(
                "Failed to shutdown".to_owned(),
            )),
            Err(RecvTimeoutError::Timeout) => Err(ExportError::Timeout(READER_SHUTDOWN_WAIT)),
        }
    }
}

pub(crate) struct Meter {
    pipelines: Vec<Pipeline>,
    shut_down: AtomicBool,
}

fn export_interval() -> Duration {
    env::var(OTEL_METRIC_EXPORT_INTERVAL)
        .ok()
        .and_then(|interval| interval.parse().map(Duration::from_millis).ok())
        .unwrap_or(EXPORT_INTERVAL_DEFAULT)
}

impl Meter {
    pub(crate) const fn new() -> Self {
        Self {
            pipelines: Vec::new(),
            shut_down: AtomicBool::new(false),
        }
    }

    pub(crate) fn add(
        &mut self,
        exporter: Box<dyn MetricExporter>,
        resource: &Resource,
        scope: &Scope,
    ) {
        let collector = Arc::new(Collector {
            streams: Atom::default(),
            exporter,
            resource: resource.clone(),
            scope: scope.clone(),
        });
        let (shutdown, received) = mpsc::channel();
        let interval = export_interval();
        let running = Arc::clone(&collector);
        let _ = std::thread::Builder::new()
            .name(READER_THREAD.to_owned())
            .spawn(move || running.run(&received, interval));
        self.pipelines.push(Pipeline {
            collector,
            shutdown,
        });
    }

    pub(crate) fn shutdown(&self) -> Exported {
        if self.shut_down.swap(true, Ordering::SeqCst) {
            return Err(ExportError::AlreadyShutdown);
        }
        let errors: Vec<ExportError> = self
            .pipelines
            .iter()
            .filter_map(|pipeline| pipeline.shutdown().err())
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ExportError::InternalFailure(format!("{errors:?}")))
        }
    }

    fn instrument(
        &self,
        name: &'static str,
        kind: Kind,
        number: Number,
    ) -> Option<Vec<Arc<Stream>>> {
        valid_name(name).then(|| {
            self.pipelines
                .iter()
                .map(|pipeline| pipeline.collector.stream(name, kind, number))
                .collect()
        })
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= INSTRUMENT_NAME_BYTES_MAX
        && name.starts_with(|character: char| character.is_ascii_alphabetic())
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || INSTRUMENT_NAME_PUNCTUATION.contains(&character)
        })
}

type Instruments = Map<(&'static str, Kind, Number), Arc<[Arc<Stream>]>>;
type Measured = Vec<(&'static str, Kind, Measurement)>;

thread_local! {
    static BUFFERS: Cell<(Vec<KeyValue>, Measured)> = const { Cell::new((Vec::new(), Vec::new())) };
}

pub(crate) struct MetricsLayer {
    meter: Arc<Meter>,
    instruments: Atom<Instruments>,
}

impl MetricsLayer {
    pub(crate) fn new(meter: Arc<Meter>) -> Self {
        Self {
            meter,
            instruments: Atom::default(),
        }
    }

    fn measure(&self, name: &'static str, kind: Kind, value: Measurement, attributes: &[KeyValue]) {
        let key = (name, kind, value.number());
        let streams = match self
            .instruments
            .read(|instruments| instruments.get(&key).cloned())
        {
            Some(streams) => streams,
            None => self.instruments.update(|instruments| {
                instruments
                    .entry(key)
                    .or_insert_with(|| {
                        self.meter
                            .instrument(name, kind, value.number())
                            .map_or_else(|| Arc::from([]), Arc::from)
                    })
                    .clone()
            }),
        };
        for stream in streams.iter() {
            stream.measure(value, attributes);
        }
    }
}

struct Visited {
    attributes: Vec<KeyValue>,
    metrics: Measured,
}

impl Visit for Visited {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.attributes
            .push(KeyValue::new(field.name(), format!("{value:?}")));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        let name = field.name();
        if let Some(metric) = name.strip_prefix(METRIC_PREFIX_GAUGE) {
            self.metrics
                .push((metric, Kind::Gauge, Measurement::U64(value)));
        } else if let Some(metric) = name.strip_prefix(METRIC_PREFIX_MONOTONIC_COUNTER) {
            self.metrics
                .push((metric, Kind::Counter, Measurement::U64(value)));
        } else if let Some(metric) = name.strip_prefix(METRIC_PREFIX_COUNTER) {
            if value <= I64_MAX {
                self.metrics
                    .push((metric, Kind::UpDownCounter, Measurement::I64(value as i64)));
            } else {
                eprintln!(
                    "[tracing-opentelemetry]: Received Counter metric, but provided u64: {value} is greater than i64::MAX. Ignoring this metric."
                );
            }
        } else if let Some(metric) = name.strip_prefix(METRIC_PREFIX_HISTOGRAM) {
            self.metrics
                .push((metric, Kind::Histogram, Measurement::U64(value)));
        } else if value <= I64_MAX {
            self.attributes
                .push(KeyValue::new(name, Value::I64(value as i64)));
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        let name = field.name();
        if let Some(metric) = name.strip_prefix(METRIC_PREFIX_GAUGE) {
            self.metrics
                .push((metric, Kind::Gauge, Measurement::F64(value)));
        } else if let Some(metric) = name.strip_prefix(METRIC_PREFIX_MONOTONIC_COUNTER) {
            self.metrics
                .push((metric, Kind::Counter, Measurement::F64(value)));
        } else if let Some(metric) = name.strip_prefix(METRIC_PREFIX_COUNTER) {
            self.metrics
                .push((metric, Kind::UpDownCounter, Measurement::F64(value)));
        } else if let Some(metric) = name.strip_prefix(METRIC_PREFIX_HISTOGRAM) {
            self.metrics
                .push((metric, Kind::Histogram, Measurement::F64(value)));
        } else {
            self.attributes.push(KeyValue::new(name, value));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        let name = field.name();
        if let Some(metric) = name.strip_prefix(METRIC_PREFIX_GAUGE) {
            self.metrics
                .push((metric, Kind::Gauge, Measurement::I64(value)));
        } else if let Some(metric) = name.strip_prefix(METRIC_PREFIX_MONOTONIC_COUNTER) {
            self.metrics
                .push((metric, Kind::Counter, Measurement::U64(value as u64)));
        } else if let Some(metric) = name.strip_prefix(METRIC_PREFIX_COUNTER) {
            self.metrics
                .push((metric, Kind::UpDownCounter, Measurement::I64(value)));
        } else {
            self.attributes.push(KeyValue::new(name, value));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.attributes
            .push(KeyValue::new(field.name(), value.to_owned()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.attributes.push(KeyValue::new(field.name(), value));
    }
}

impl Layer for MetricsLayer {
    type Span = ();

    fn new_span(&self, _: &Attributes<'_>, _: &SpanRef<'_, ()>, _: &Context<'_, ()>) {}

    fn on_event(&self, event: &Event<'_>, _: &Context<'_, ()>) {
        let (attributes, metrics) = BUFFERS.take();
        let mut visited = Visited {
            attributes,
            metrics,
        };
        event.record(&mut visited);
        for (name, kind, value) in visited.metrics.drain(..) {
            self.measure(name, kind, value, &visited.attributes);
        }
        visited.attributes.clear();
        BUFFERS.set((visited.attributes, visited.metrics));
    }
}

pub(crate) struct MetricsFilter;

fn metrics_event(metadata: &Metadata<'_>) -> bool {
    metadata.is_event()
        && metadata.fields().iter().any(|field| {
            let name = field.name();
            [
                METRIC_PREFIX_COUNTER,
                METRIC_PREFIX_MONOTONIC_COUNTER,
                METRIC_PREFIX_HISTOGRAM,
                METRIC_PREFIX_GAUGE,
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix))
        })
}

impl Filter for MetricsFilter {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metrics_event(metadata)
    }

    fn callsite_enabled(&self, metadata: &'static Metadata<'static>) -> Interest {
        if metrics_event(metadata) {
            Interest::always()
        } else {
            Interest::never()
        }
    }
}
