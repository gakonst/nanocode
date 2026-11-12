use std::{
    env,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel},
    },
    time::{Duration, Instant},
};

use crate::{
    model::{ExportError, Exported, SpanData},
    trace::SpanProcessor,
};

const OTEL_BSP_MAX_QUEUE_SIZE: &str = "OTEL_BSP_MAX_QUEUE_SIZE";
const OTEL_BSP_SCHEDULE_DELAY: &str = "OTEL_BSP_SCHEDULE_DELAY";
const OTEL_BSP_MAX_EXPORT_BATCH_SIZE: &str = "OTEL_BSP_MAX_EXPORT_BATCH_SIZE";
const MAX_QUEUE_SIZE_DEFAULT: usize = 2_048;
const SCHEDULE_DELAY_DEFAULT: Duration = Duration::from_millis(5_000);
const MAX_EXPORT_BATCH_SIZE_DEFAULT: usize = 512;
const CONTROL_MESSAGES_MAX: usize = 64;
const SHUTDOWN_CHANNEL_FULL: &str = "Shutdown cannot be performed as Control channel is full. This can occur if user repeatedly calls force_flush/shutdown without finishing the previous call.";
const BATCH_THREAD: &str = "OpenTelemetry.Traces.BatchProcessor";

pub(crate) trait SpanExporter: Send + 'static {
    fn export(&self, batch: Vec<SpanData>) -> Exported;
}

enum Message {
    Export,
    Shutdown(SyncSender<()>),
}

#[derive(Clone, Copy)]
struct Config {
    max_queue_size: usize,
    scheduled_delay: Duration,
    max_export_batch_size: usize,
}

impl Config {
    fn from_env() -> Self {
        let read = |name: &str| {
            env::var(name)
                .ok()
                .and_then(|value| usize::from_str(&value).ok())
        };
        let max_queue_size = read(OTEL_BSP_MAX_QUEUE_SIZE).unwrap_or(MAX_QUEUE_SIZE_DEFAULT);
        let scheduled_delay = env::var(OTEL_BSP_SCHEDULE_DELAY)
            .ok()
            .and_then(|delay| u64::from_str(&delay).ok())
            .map_or(SCHEDULE_DELAY_DEFAULT, Duration::from_millis);
        let max_export_batch_size = read(OTEL_BSP_MAX_EXPORT_BATCH_SIZE)
            .unwrap_or(MAX_EXPORT_BATCH_SIZE_DEFAULT)
            .min(max_queue_size);
        Self {
            max_queue_size,
            scheduled_delay,
            max_export_batch_size,
        }
    }
}

pub(crate) struct BatchSpanProcessor {
    spans: SyncSender<SpanData>,
    messages: SyncSender<Message>,
    export_requested: Arc<AtomicBool>,
    batch_size: Arc<AtomicUsize>,
    max_export_batch_size: usize,
}

struct Worker<E> {
    exporter: E,
    spans: Receiver<SpanData>,
    batch: Vec<SpanData>,
    batch_size: Arc<AtomicUsize>,
    export_requested: Arc<AtomicBool>,
    config: Config,
}

impl<E: SpanExporter> Worker<E> {
    fn run(mut self, messages: &Receiver<Message>) {
        crate::trace::suppress_on_this_thread();
        let mut last_export = Instant::now();
        loop {
            let remaining = self
                .config
                .scheduled_delay
                .checked_sub(last_export.elapsed())
                .unwrap_or(self.config.scheduled_delay);
            match messages.recv_timeout(remaining) {
                Ok(Message::Export) => {
                    self.export_requested.store(false, Ordering::Relaxed);
                    drop(self.export(&mut last_export));
                }
                Ok(Message::Shutdown(reply)) => {
                    drop(self.export(&mut last_export));
                    let _ = reply.send(());
                    return;
                }
                Err(RecvTimeoutError::Timeout) => drop(self.export(&mut last_export)),
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn export(&mut self, last_export: &mut Instant) -> Exported {
        let target = self.batch_size.load(Ordering::Acquire);
        let mut result = Ok(());
        let mut exported = 0;
        while target > 0 && exported < target {
            let limit = self.config.max_export_batch_size.min(target - exported);
            while let Ok(span) = self.spans.try_recv() {
                self.batch.push(span);
                if self.batch.len() == limit {
                    break;
                }
            }
            let count = self.batch.len();
            if count == 0 {
                break;
            }
            exported += count;
            *last_export = Instant::now();
            result = self.exporter.export(self.batch.split_off(0));
            self.batch_size.fetch_sub(count, Ordering::AcqRel);
        }
        result
    }
}

impl BatchSpanProcessor {
    pub(crate) fn new(exporter: impl SpanExporter) -> Self {
        let config = Config::from_env();
        let (spans, received_spans) = sync_channel(config.max_queue_size);
        let (messages, received) = sync_channel(CONTROL_MESSAGES_MAX);
        let batch_size = Arc::new(AtomicUsize::new(0));
        let export_requested = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            exporter,
            spans: received_spans,
            batch: Vec::with_capacity(config.max_export_batch_size),
            batch_size: Arc::clone(&batch_size),
            export_requested: Arc::clone(&export_requested),
            config,
        };
        std::thread::Builder::new()
            .name(BATCH_THREAD.to_owned())
            .spawn(move || worker.run(&received))
            .expect("Failed to spawn thread");
        Self {
            spans,
            messages,
            export_requested,
            batch_size,
            max_export_batch_size: config.max_export_batch_size,
        }
    }
}

impl SpanProcessor for BatchSpanProcessor {
    fn on_end(&self, span: SpanData) {
        if self.spans.try_send(span).is_ok()
            && self.batch_size.fetch_add(1, Ordering::AcqRel) + 1 >= self.max_export_batch_size
            && !self.export_requested.load(Ordering::Relaxed)
            && !self.export_requested.swap(true, Ordering::Relaxed)
            && self.messages.try_send(Message::Export).is_err()
        {
            self.export_requested.store(false, Ordering::Relaxed);
        }
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> Exported {
        let (reply, response) = sync_channel(1);
        match self.messages.try_send(Message::Shutdown(reply)) {
            Ok(()) => response.recv_timeout(timeout).map_err(|error| match error {
                RecvTimeoutError::Timeout => ExportError::Timeout(timeout),
                RecvTimeoutError::Disconnected => ExportError::InternalFailure(error.to_string()),
            }),
            Err(TrySendError::Full(_)) => Err(ExportError::InternalFailure(
                SHUTDOWN_CHANNEL_FULL.to_owned(),
            )),
            Err(TrySendError::Disconnected(_)) => Err(ExportError::AlreadyShutdown),
        }
    }
}
