use tracing_core::{
    Event, Interest, Level, LevelFilter, Metadata,
    callsite::{Callsite, Identifier},
    dispatcher,
    field::{Field, FieldSet, Value, Visit},
    identify_callsite,
    metadata::Kind,
};

const FIELD_NAMES: &[&str] = &[
    "message",
    "log.target",
    "log.module_path",
    "log.file",
    "log.line",
];

macro_rules! log_callsite {
    ($level:expr, $callsite:ident, $metadata:ident, $kind:ident) => {
        struct $kind;
        static $callsite: $kind = $kind;
        static $metadata: Metadata<'static> = Metadata::new(
            "log event",
            "log",
            $level,
            None,
            None,
            None,
            FieldSet::new(FIELD_NAMES, identify_callsite!(&$callsite)),
            Kind::EVENT,
        );
        impl Callsite for $kind {
            fn set_interest(&self, _: Interest) {}

            fn metadata(&self) -> &Metadata<'_> {
                &$metadata
            }
        }
    };
}

log_callsite!(Level::TRACE, TRACE_CALLSITE, TRACE_METADATA, TraceCallsite);
log_callsite!(Level::DEBUG, DEBUG_CALLSITE, DEBUG_METADATA, DebugCallsite);
log_callsite!(Level::INFO, INFO_CALLSITE, INFO_METADATA, InfoCallsite);
log_callsite!(Level::WARN, WARN_CALLSITE, WARN_METADATA, WarnCallsite);
log_callsite!(Level::ERROR, ERROR_CALLSITE, ERROR_METADATA, ErrorCallsite);

fn callsite(level: Level) -> (Identifier, &'static Metadata<'static>) {
    match level {
        Level::TRACE => (identify_callsite!(&TRACE_CALLSITE), &TRACE_METADATA),
        Level::DEBUG => (identify_callsite!(&DEBUG_CALLSITE), &DEBUG_METADATA),
        Level::INFO => (identify_callsite!(&INFO_CALLSITE), &INFO_METADATA),
        Level::WARN => (identify_callsite!(&WARN_CALLSITE), &WARN_METADATA),
        Level::ERROR => (identify_callsite!(&ERROR_CALLSITE), &ERROR_METADATA),
    }
}

const fn level(level: log::Level) -> Level {
    match level {
        log::Level::Error => Level::ERROR,
        log::Level::Warn => Level::WARN,
        log::Level::Info => Level::INFO,
        log::Level::Debug => Level::DEBUG,
        log::Level::Trace => Level::TRACE,
    }
}

pub(crate) const fn log_level_filter(filter: LevelFilter) -> log::LevelFilter {
    match filter.into_level() {
        None => log::LevelFilter::Off,
        Some(Level::ERROR) => log::LevelFilter::Error,
        Some(Level::WARN) => log::LevelFilter::Warn,
        Some(Level::INFO) => log::LevelFilter::Info,
        Some(Level::DEBUG) => log::LevelFilter::Debug,
        Some(Level::TRACE) => log::LevelFilter::Trace,
    }
}

fn filter_metadata<'a>(
    target: &'a str,
    log_level: log::Level,
    file: Option<&'a str>,
    line: Option<u32>,
    module_path: Option<&'a str>,
    name: &'static str,
) -> Metadata<'a> {
    let level = level(log_level);
    Metadata::new(
        name,
        target,
        level,
        file,
        line,
        module_path,
        FieldSet::new(FIELD_NAMES, callsite(level).0),
        Kind::EVENT,
    )
}

pub(crate) struct LogBridge;

pub(crate) static LOG_BRIDGE: LogBridge = LogBridge;

impl log::Log for LogBridge {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        if level(metadata.level()) > LevelFilter::current() {
            return false;
        }
        let metadata = filter_metadata(
            metadata.target(),
            metadata.level(),
            None,
            None,
            None,
            "log record",
        );
        dispatcher::get_default(|dispatch| dispatch.enabled(&metadata))
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            dispatch_record(record);
        }
    }

    fn flush(&self) {}
}

fn dispatch_record(record: &log::Record<'_>) {
    dispatcher::get_default(|dispatch| {
        let filter = filter_metadata(
            record.target(),
            record.level(),
            record.file(),
            record.line(),
            record.module_path(),
            "log record",
        );
        if !dispatch.enabled(&filter) {
            return;
        }
        let metadata = callsite(level(record.level())).1;
        let fields = metadata.fields();
        let mut names = fields.iter();
        let mut field = || names.next().expect("a log event has its five fields");
        let (message, target, module, file, line) = (field(), field(), field(), field(), field());
        let module_path = record.module_path();
        let file_path = record.file();
        let line_number = record.line();
        let target_name = record.target();
        let values = [
            (&message, Some(record.args() as &dyn Value)),
            (&target, Some(&target_name as &dyn Value)),
            (&module, module_path.as_ref().map(|path| path as &dyn Value)),
            (&file, file_path.as_ref().map(|path| path as &dyn Value)),
            (&line, line_number.as_ref().map(|line| line as &dyn Value)),
        ];
        dispatch.event(&Event::new(metadata, &fields.value_set(&values)));
    });
}

pub(crate) struct Normalized {
    pub(crate) target: Option<String>,
    pub(crate) file: Option<String>,
    pub(crate) line: Option<u32>,
}

pub(crate) fn normalize(event: &Event<'_>) -> Option<Normalized> {
    let metadata = event.metadata();
    if metadata.callsite() != callsite(*metadata.level()).0 {
        return None;
    }
    let mut visitor = LogVisitor {
        target: None,
        file: None,
        line: None,
    };
    event.record(&mut visitor);
    Some(Normalized {
        target: visitor.target,
        file: visitor.file,
        line: visitor
            .line
            .map(|line| u32::try_from(line).expect("a log record's line is a u32")),
    })
}

struct LogVisitor {
    target: Option<String>,
    file: Option<String>,
    line: Option<u64>,
}

impl Visit for LogVisitor {
    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "log.line" {
            self.line = Some(value);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "log.file" => self.file = Some(value.to_owned()),
            "log.target" => self.target = Some(value.to_owned()),
            _ => {}
        }
    }
}

