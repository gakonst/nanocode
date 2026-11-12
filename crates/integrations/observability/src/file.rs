use std::{
    fs::{DirBuilder, File, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use opentelemetry::{Context, InstrumentationScope};
use opentelemetry_sdk::{
    Resource,
    error::{OTelSdkError, OTelSdkResult},
    metrics::{Temporality, data::ResourceMetrics, exporter::PushMetricExporter},
    trace::{Span, SpanData, SpanProcessor},
};
use tokio::sync::mpsc::{
    UnboundedReceiver, UnboundedSender, error::TryRecvError, unbounded_channel,
};

const OTEL_BSP_MAX_QUEUE_SIZE: &str = "OTEL_BSP_MAX_QUEUE_SIZE";
const OTEL_BSP_MAX_QUEUE_SIZE_DEFAULT: usize = 2_048;

pub struct Exporters {
    pub traces: TraceProcessor,
    pub metrics: MetricExporter,
}

impl Exporters {
    pub fn create(directory: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let directory = directory.join(format!("{timestamp}-{}", std::process::id()));
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
        Ok(Self {
            traces: TraceProcessor::spawn(
                create_file(&directory.join("traces.jsonl"))?,
                max_queue_size()?,
            )?,
            metrics: MetricExporter {
                output: Jsonl::create(&directory.join("metrics.jsonl"))?,
                json_bytes: AtomicUsize::new(0),
            },
        })
    }
}

#[derive(Debug)]
struct Jsonl {
    sender: Sender<JsonlMessage>,
}

#[derive(Debug)]
enum JsonlMessage {
    Write(Vec<u8>, SyncSender<io::Result<()>>),
    Shutdown(SyncSender<()>),
}

fn max_queue_size() -> io::Result<usize> {
    let invalid = |error: &dyn std::fmt::Display| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid {OTEL_BSP_MAX_QUEUE_SIZE}: {error}"),
        )
    };
    match std::env::var(OTEL_BSP_MAX_QUEUE_SIZE) {
        Ok(value) => value.parse().map_err(|error| invalid(&error)),
        Err(std::env::VarError::NotPresent) => Ok(OTEL_BSP_MAX_QUEUE_SIZE_DEFAULT),
        Err(error) => Err(invalid(&error)),
    }
}

fn create_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn write_failed(error: impl std::fmt::Display) -> OTelSdkError {
    eprintln!("nanocode: failed to write local OTLP telemetry: {error}");
    OTelSdkError::InternalFailure(error.to_string())
}

impl Jsonl {
    fn create(path: &Path) -> io::Result<Self> {
        let file = create_file(path)?;
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("nanocode-otlp-metrics".to_owned())
            .spawn(move || Self::run(file, &receiver))?;
        Ok(Self { sender })
    }

    fn run(file: File, receiver: &Receiver<JsonlMessage>) {
        let mut file = Some(file);
        while let Ok(message) = receiver.recv() {
            match message {
                JsonlMessage::Write(encoded, reply) => {
                    let result = file.as_mut().map_or_else(
                        || Err(io::Error::other("OTLP file exporter is shut down")),
                        |file| file.write_all(&encoded),
                    );
                    drop(reply.send(result));
                }
                JsonlMessage::Shutdown(reply) => {
                    drop(file.take());
                    let _ = reply.send(());
                }
            }
        }
    }

    fn write(&self, mut encoded: Vec<u8>) -> OTelSdkResult {
        let result = (|| {
            encoded.push(b'\n');
            let (reply, response) = mpsc::sync_channel(1);
            self.sender
                .send(JsonlMessage::Write(encoded, reply))
                .map_err(|_| io::Error::other("OTLP file exporter stopped"))?;
            response
                .recv()
                .map_err(|_| io::Error::other("OTLP file exporter stopped"))?
        })();
        result.map_err(write_failed)
    }

    fn shutdown(&self) -> OTelSdkResult {
        let stopped = || OTelSdkError::InternalFailure("OTLP file exporter stopped".to_owned());
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .send(JsonlMessage::Shutdown(reply))
            .map_err(|_| stopped())?;
        response.recv().map_err(|_| stopped())
    }
}

#[derive(Debug)]
enum TraceMessage {
    Span {
        scope: InstrumentationScope,
        json: Vec<u8>,
    },
    Unencoded(String),
    Resource(Resource),
    Flush(SyncSender<OTelSdkResult>),
    Shutdown(SyncSender<OTelSdkResult>),
}

#[derive(Debug, Default)]
struct TraceQueue {
    capacity: usize,
    queued: AtomicUsize,
    dropped: AtomicUsize,
    closed: AtomicBool,
    span_bytes: AtomicUsize,
}

#[derive(Debug)]
pub struct TraceProcessor {
    sender: UnboundedSender<TraceMessage>,
    queue: Arc<TraceQueue>,
}

impl TraceProcessor {
    fn spawn(file: File, capacity: usize) -> io::Result<Self> {
        let (sender, receiver) = unbounded_channel();
        let queue = Arc::new(TraceQueue {
            capacity,
            ..TraceQueue::default()
        });
        let writer_queue = Arc::clone(&queue);
        let origin = crate::otlp::Origin::of(&Resource::builder_empty().build())?;
        std::thread::Builder::new()
            .name("nanocode-otlp-traces".to_owned())
            .spawn(move || {
                TraceWriter {
                    file,
                    origin,
                    batch: crate::otlp::Batch::default(),
                    waiting: 0,
                    line: Vec::new(),
                    error: None,
                    queue: writer_queue,
                    dropped: 0,
                }
                .run(receiver);
            })?;
        Ok(Self { sender, queue })
    }

    fn closed(&self) {
        if !self.queue.closed.swap(true, Ordering::Relaxed) {
            write_failed("OTLP file exporter is shut down; later spans are not written");
        }
    }

    fn request(
        &self,
        message: fn(SyncSender<OTelSdkResult>) -> TraceMessage,
        timeout: Option<Duration>,
    ) -> OTelSdkResult {
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .send(message(reply))
            .map_err(|_| OTelSdkError::AlreadyShutdown)?;
        let stopped = || OTelSdkError::InternalFailure("OTLP file exporter stopped".to_owned());
        match timeout {
            Some(timeout) => response
                .recv_timeout(timeout)
                .map_err(|error| match error {
                    RecvTimeoutError::Timeout => OTelSdkError::Timeout(timeout),
                    RecvTimeoutError::Disconnected => stopped(),
                })?,
            None => response.recv().map_err(|_| stopped())?,
        }
    }
}

impl SpanProcessor for TraceProcessor {
    fn on_start(&self, _span: &mut Span, _cx: &Context) {}

    fn on_end(&self, span: SpanData) {
        if !span.span_context.is_sampled() {
            return;
        }
        let capacity = self.queue.capacity;
        if self
            .queue
            .queued
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |queued| {
                (queued < capacity).then_some(queued + 1)
            })
            .is_ok()
        {
            let mut json = Vec::with_capacity(self.queue.span_bytes.load(Ordering::Relaxed));
            let message = match crate::otlp::span(&span, &mut json) {
                Ok(()) => {
                    self.queue.span_bytes.store(json.len(), Ordering::Relaxed);
                    TraceMessage::Span {
                        scope: span.instrumentation_scope,
                        json,
                    }
                }
                Err(error) => TraceMessage::Unencoded(error.to_string()),
            };
            if self.sender.send(message).is_err() {
                self.queue.queued.fetch_sub(1, Ordering::Relaxed);
                self.closed();
            }
        } else {
            self.queue.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.request(TraceMessage::Flush, None)
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.request(TraceMessage::Shutdown, Some(timeout))
    }

    fn set_resource(&mut self, resource: &Resource) {
        if self
            .sender
            .send(TraceMessage::Resource(resource.clone()))
            .is_err()
        {
            self.closed();
        }
    }
}

struct TraceWriter {
    file: File,
    origin: crate::otlp::Origin,
    batch: crate::otlp::Batch,
    waiting: usize,
    line: Vec<u8>,
    error: Option<OTelSdkError>,
    queue: Arc<TraceQueue>,
    dropped: usize,
}

impl TraceWriter {
    fn run(mut self, mut receiver: UnboundedReceiver<TraceMessage>) {
        while let Some(message) = waited(&mut receiver) {
            let mut frontier = receiver.len();
            let mut message = Some(message);
            while let Some(next) = message.take() {
                match next {
                    TraceMessage::Span { scope, json } => {
                        self.batch.push(scope, &json);
                        self.waiting += 1;
                    }
                    TraceMessage::Unencoded(error) => {
                        self.error.get_or_insert(write_failed(error));
                        self.waiting += 1;
                    }
                    TraceMessage::Resource(resource) => {
                        self.write();
                        match crate::otlp::Origin::of(&resource) {
                            Ok(origin) => self.origin = origin,
                            Err(error) => {
                                self.error.get_or_insert(write_failed(error));
                            }
                        }
                    }
                    TraceMessage::Flush(reply) => drop(reply.send(self.result())),
                    TraceMessage::Shutdown(reply) => {
                        let result = self.result();
                        drop(self);
                        drop(reply.send(result));
                        return reject_after_shutdown(receiver);
                    }
                }
                if frontier > 0 {
                    frontier -= 1;
                    message = receiver.try_recv().ok();
                }
            }
            self.write();
        }
    }

    fn result(&mut self) -> OTelSdkResult {
        self.write();
        self.report_dropped();
        let dropped = std::mem::take(&mut self.dropped);
        match self.error.take() {
            Some(error) => Err(error),
            None if dropped > 0 => Err(OTelSdkError::InternalFailure(format!(
                "dropped {dropped} spans because {} spans were waiting for the local OTLP file ({OTEL_BSP_MAX_QUEUE_SIZE})",
                self.queue.capacity
            ))),
            None => Ok(()),
        }
    }

    fn report_dropped(&mut self) {
        let dropped = self.queue.dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            eprintln!(
                "nanocode: dropped {dropped} local OTLP spans because {} spans were waiting to be written ({OTEL_BSP_MAX_QUEUE_SIZE})",
                self.queue.capacity
            );
            self.dropped += dropped;
        }
    }

    fn write(&mut self) {
        let taken = std::mem::take(&mut self.waiting);
        if taken == 0 {
            return;
        }
        self.queue.queued.fetch_sub(taken, Ordering::Relaxed);
        if !self.batch.is_empty() {
            self.line.clear();
            let result = self
                .batch
                .line(&self.origin, &mut self.line)
                .map_err(io::Error::from)
                .and_then(|()| self.file.write_all(&self.line));
            if let Err(error) = result {
                self.error.get_or_insert(write_failed(error));
            }
        }
        self.report_dropped();
    }
}

fn waited(receiver: &mut UnboundedReceiver<TraceMessage>) -> Option<TraceMessage> {
    match receiver.try_recv() {
        Ok(message) => return Some(message),
        Err(TryRecvError::Disconnected) => return None,
        Err(TryRecvError::Empty) => {}
    }
    std::thread::yield_now();
    receiver.blocking_recv()
}

fn reject_after_shutdown(mut receiver: UnboundedReceiver<TraceMessage>) {
    receiver.close();
    let mut late = 0_usize;
    while let Ok(message) = receiver.try_recv() {
        match message {
            TraceMessage::Span { .. } | TraceMessage::Unencoded(_) => late += 1,
            TraceMessage::Resource(_) => {}
            TraceMessage::Flush(reply) | TraceMessage::Shutdown(reply) => {
                drop(reply.send(Err(OTelSdkError::AlreadyShutdown)));
            }
        }
    }
    if late > 0 {
        write_failed(format!(
            "OTLP file exporter is shut down; {late} spans ended during shutdown"
        ));
    }
}

pub struct MetricExporter {
    output: Jsonl,
    json_bytes: AtomicUsize,
}

impl PushMetricExporter for MetricExporter {
    async fn export(&self, metrics: &ResourceMetrics) -> OTelSdkResult {
        let mut json = Vec::with_capacity(self.json_bytes.load(Ordering::Relaxed));
        crate::otlp::metrics(metrics, &mut json)
            .map_err(|error| OTelSdkError::InternalFailure(error.to_string()))?;
        self.json_bytes.store(json.len(), Ordering::Relaxed);
        self.output.write(json)
    }

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }

    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        self.output.shutdown()
    }

    fn temporality(&self) -> Temporality {
        Temporality::Cumulative
    }
}
