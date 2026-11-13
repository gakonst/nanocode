#![cfg(not(target_family = "wasm"))]
#![deny(rustdoc::broken_intra_doc_links)]

extern crate self as nanocode_observability;

mod batch;
mod cli;
pub mod env_filter;
mod file;
pub mod filter;
pub mod format;
mod json;
mod layer;
mod log_bridge;
mod metrics;
mod model;
mod otlp;
mod otlp_http;
mod pattern;
mod pretty;
mod protobuf;
mod style;
pub mod subscriber;
mod trace;
mod writer;

use std::{borrow::Cow, fs::OpenOptions, io, path::PathBuf, sync::Arc, time::Duration};


pub use batch::{BatchConfig, BatchSpanProcessor, SpanExporter};
pub use cli::ObservabilityOutputArgs;
pub use layer::{OtelLayer, OtelSpan, add_link, span_context};
pub use model::{
    Event, ExportError, KeyValue, Resource, Scope, SpanContext, SpanData, SpanKind, Status, Text,
    Value,
};
pub use otlp_http::{OtlpError, SpanExporter as OtlpSpanExporter};
pub use trace::{SpanProcessor, Tracer};

use crate::{
    filter::{And, FilterFn, ParseError},
    format::{FmtLayer, Format},
    metrics::{Meter, MetricsFilter, MetricsLayer},
    subscriber::{Filtered, Registry},
};

pub const DEFAULT_FILTER: &str = "warn,nanocode=info,nanocode_backend_codex=info,nanocode_backend_claude=info,nanocode_claude=info,nanocode_agent=info,nanocode_code_mode=info,nanocode_subagents=info,nanocode_durability=info,nanocode_store_jsonl=info,nanocode=info";
pub const METRICS_TARGET: &str = "nc_metrics";
pub const TUI_TARGET: &str = "nanocode_tui";

const SCHEMA_URL: &str = "https://opentelemetry.io/schemas/1.42.0";
const SERVICE_VERSION: &str = "service.version";
const DEPLOYMENT_ENVIRONMENT_NAME: &str = "deployment.environment.name";
const PROCESS_PID: &str = "process.pid";
const PROCESS_EXECUTABLE_PATH: &str = "process.executable.path";
const EXECUTABLE_MODIFIED: &str = "nanocode.executable.modified";
const LOCAL_LOG_BUFFERED_LINES_LIMIT: usize = 4_096;
const SERVICE_NAME: &str = "service.name";
const TELEMETRY_SDK_NAME: &str = "telemetry.sdk.name";
const TELEMETRY_SDK_LANGUAGE: &str = "telemetry.sdk.language";
const TELEMETRY_SDK_VERSION: &str = "telemetry.sdk.version";
const TELEMETRY_SDK: (&str, &str, &str) = ("opentelemetry", "rust", "0.32.1");
const OTEL_SERVICE_NAME: &str = "OTEL_SERVICE_NAME";
const OTEL_RESOURCE_ATTRIBUTES: &str = "OTEL_RESOURCE_ATTRIBUTES";
const UNKNOWN_SERVICE: &str = "unknown_service";
const METER_SCOPE: &str = "tracing/tracing-opentelemetry";
const METER_SCOPE_VERSION: &str = "0.33.0";
const TRACER_SHUTDOWN_DEFAULT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LogFormat {
    Pretty,

    #[default]
    Compact,

    Json,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Telemetry {
    Off,

    #[default]
    Agent,

    All,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum LogOutput {
    #[default]
    Stderr,

    File(PathBuf),

    Disabled,
}

#[derive(Clone, Debug)]
pub struct ObservabilityBuilder {
    filter: String,
    otel_filter: String,
    format: LogFormat,
    output: LogOutput,
    service_name: String,
    service_version: String,
    environment: Option<String>,
    otlp_endpoint: Option<String>,
    otlp_directory: Option<PathBuf>,
    telemetry: Telemetry,
    shutdown_timeout: Option<Duration>,
}

pub struct ObservabilityGuard {
    tracer_provider: Option<Tracer>,
    meter_provider: Option<Arc<Meter>>,
    shutdown_timeout: Option<Duration>,
    _writer: Option<writer::WriterGuard>,
}

#[derive(Debug)]
pub enum ObservabilityError {
    Filter(ParseError),

    Output(io::Error),

    Otlp(OtlpError),

    OTelSdk(ExportError),

    Subscriber,
}

impl std::fmt::Display for ObservabilityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Filter(value) => write!(formatter, "invalid tracing filter: {value}"),
            Self::Output(value) => write!(formatter, "failed to open tracing output: {value}"),
            Self::Otlp(value) => write!(formatter, "failed to configure OTLP exporter: {value}"),
            Self::OTelSdk(value) => write!(
                formatter,
                "failed to flush or shut down the OpenTelemetry exporter: {value}"
            ),
            Self::Subscriber => {
                formatter.write_str("a global tracing subscriber is already installed")
            }
        }
    }
}

impl std::error::Error for ObservabilityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Filter(value) => Some(value),
            Self::Output(value) => Some(value),
            Self::Otlp(value) => Some(value),
            Self::OTelSdk(value) => Some(value),
            _ => None,
        }
    }
}

impl From<ParseError> for ObservabilityError {
    fn from(source: ParseError) -> Self {
        Self::Filter(source)
    }
}

impl From<io::Error> for ObservabilityError {
    fn from(source: io::Error) -> Self {
        Self::Output(source)
    }
}

impl From<OtlpError> for ObservabilityError {
    fn from(source: OtlpError) -> Self {
        Self::Otlp(source)
    }
}

impl From<ExportError> for ObservabilityError {
    fn from(source: ExportError) -> Self {
        Self::OTelSdk(source)
    }
}

impl ObservabilityBuilder {
    #[must_use]
    pub fn new(service_name: impl Into<String>, service_version: impl Into<String>) -> Self {
        Self {
            otel_filter: format!("{DEFAULT_FILTER},{METRICS_TARGET}=info"),
            filter: DEFAULT_FILTER.to_owned(),
            format: LogFormat::Compact,
            output: LogOutput::Stderr,
            service_name: service_name.into(),
            service_version: service_version.into(),
            environment: None,
            otlp_endpoint: None,
            otlp_directory: None,
            telemetry: Telemetry::Agent,
            shutdown_timeout: None,
        }
    }

    #[must_use]
    pub fn filter(mut self, filter: impl Into<String>) -> Self {
        self.filter = filter.into();
        self
    }

    #[must_use]
    pub fn otel_filter(mut self, filter: impl Into<String>) -> Self {
        self.otel_filter = filter.into();
        self
    }

    #[must_use]
    pub const fn format(mut self, format: LogFormat) -> Self {
        self.format = format;
        self
    }

    #[must_use]
    pub fn output(mut self, output: LogOutput) -> Self {
        self.output = output;
        self
    }

    #[must_use]
    pub fn environment(mut self, environment: impl Into<String>) -> Self {
        self.environment = Some(environment.into());
        self
    }

    #[must_use]
    pub fn otlp_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_endpoint = Some(endpoint.into().trim_end_matches('/').to_owned());
        self
    }

    #[must_use]
    pub fn otlp_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.otlp_directory = Some(directory.into());
        self
    }

    #[must_use]
    pub const fn telemetry(mut self, telemetry: Telemetry) -> Self {
        self.telemetry = telemetry;
        self
    }

    #[must_use]
    pub const fn shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.shutdown_timeout = Some(timeout);
        self
    }

    pub fn install(self) -> Result<ObservabilityGuard, ObservabilityError> {
        nanocode_tls::install_default_rustls_crypto_provider();
        let (fmt_layer, writer_guard) = match self.writer()? {
            Some(output) => {
                let (writer, guard) = writer::spawn(output, LOCAL_LOG_BUFFERED_LINES_LIMIT)?;
                let filter = filter::scope(self.filter.as_str())?;
                let format = match self.format {
                    LogFormat::Pretty => Format::Pretty,
                    LogFormat::Compact => Format::Compact,
                    LogFormat::Json => Format::Json,
                };
                (
                    Some(Filtered::new(FmtLayer::new(format, writer), filter)),
                    Some(guard),
                )
            }
            None => (None, None),
        };
        let (tracer_provider, meter_provider) = match (
            self.otlp_endpoint.as_deref(),
            self.otlp_directory.as_deref(),
        ) {
            _ if self.telemetry == Telemetry::Off => (None, None),
            (endpoint, directory) if endpoint.is_some() || directory.is_some() => {
                let resource = self.resource();
                let tracer_scope = Scope {
                    name: Cow::Owned(self.service_name.clone()),
                    version: None,
                };
                let files = directory
                    .map(|directory| file::Exporters::create(directory, &resource, &tracer_scope))
                    .transpose()?;
                let (traces, metrics) = files.map_or((None, None), |files| {
                    (Some(files.traces), Some(files.metrics))
                });
                (
                    Some(tracer_provider(endpoint, traces, &resource, tracer_scope)?),
                    Some(meter_provider(endpoint, metrics, &resource)?),
                )
            }
            _ => (None, None),
        };
        let terminal = self.telemetry == Telemetry::All;
        let tracing_layer = match &tracer_provider {
            Some(provider) => Some(Filtered::new(
                OtelLayer::new(provider.clone()),
                And(
                    filter::scope(self.otel_filter.as_str())?,
                    FilterFn(move |metadata: &tracing::Metadata<'_>| {
                        metadata.target() != METRICS_TARGET
                            && (terminal || !terminal_target(metadata.target()))
                    }),
                ),
            )),
            None => None,
        };
        let metrics_layer = match &meter_provider {
            Some(provider) => Some(Filtered::nested(
                MetricsLayer::new(Arc::clone(provider)),
                MetricsFilter,
                And(
                    filter::scope(self.otel_filter.as_str())?,
                    FilterFn(move |metadata: &tracing::Metadata<'_>| {
                        terminal || !terminal_target(metadata.target())
                    }),
                ),
            )),
            None => None,
        };
        init(Registry::new([fmt_layer, tracing_layer, metrics_layer]))?;
        tracing::callsite::rebuild_interest_cache();
        Ok(ObservabilityGuard {
            tracer_provider,
            meter_provider,
            shutdown_timeout: self.shutdown_timeout,
            _writer: writer_guard,
        })
    }

    fn writer(&self) -> Result<Option<Box<dyn io::Write + Send>>, io::Error> {
        match &self.output {
            LogOutput::Stderr => Ok(Some(Box::new(io::stderr()))),
            LogOutput::File(path) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                Ok(Some(Box::new(
                    OpenOptions::new().create(true).append(true).open(path)?,
                )))
            }
            LogOutput::Disabled => Ok(None),
        }
    }

    fn resource(&self) -> Resource {
        let mut resource = Resource::detected();
        if std::env::var_os(OTEL_SERVICE_NAME).is_none() {
            resource = resource.merge(&Resource {
                attributes: vec![KeyValue::new(SERVICE_NAME, self.service_name.clone())],
                schema_url: None,
            });
        }
        let mut attributes = vec![
            KeyValue::new(SERVICE_VERSION, self.service_version.clone()),
            KeyValue::new(
                DEPLOYMENT_ENVIRONMENT_NAME,
                self.environment
                    .clone()
                    .unwrap_or_else(|| "development".to_owned()),
            ),
            KeyValue::new(PROCESS_PID, i64::from(std::process::id())),
        ];
        if let Ok(executable) = std::env::current_exe().and_then(std::fs::canonicalize) {
            if let Some(modified) = std::fs::metadata(&executable)
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|modified| i64::try_from(modified.as_micros()).ok())
            {
                attributes.push(KeyValue::new(EXECUTABLE_MODIFIED, modified));
            }
            attributes.push(KeyValue::new(
                PROCESS_EXECUTABLE_PATH,
                executable.to_string_lossy().into_owned(),
            ));
        }
        let mut base = Resource {
            attributes: Vec::new(),
            schema_url: Some(Cow::Borrowed(SCHEMA_URL)),
        };
        for pair in attributes {
            base.insert(pair);
        }
        base.merge(&resource)
    }
}

fn environment_attributes() -> Resource {
    let mut resource = Resource::default();
    if let Ok(attributes) = std::env::var(OTEL_RESOURCE_ATTRIBUTES)
        && !attributes.is_empty()
    {
        for (key, value) in attributes
            .split_terminator(',')
            .filter_map(|entry| entry.split_once('='))
        {
            resource.insert(KeyValue::new(
                key.trim().to_owned(),
                value.trim().to_owned(),
            ));
        }
    }
    resource
}

impl Resource {
    #[must_use]
    pub fn detected() -> Self {
        let service = std::env::var(OTEL_SERVICE_NAME)
            .ok()
            .filter(|name| !name.is_empty())
            .map(model::Value::from)
            .or_else(|| {
                environment_attributes()
                    .attributes
                    .into_iter()
                    .find(|pair| pair.key == SERVICE_NAME)
                    .map(|pair| pair.value)
            })
            .unwrap_or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .map(|name| format!("{UNKNOWN_SERVICE}:{name}"))
                    })
                    .unwrap_or_else(|| UNKNOWN_SERVICE.to_owned())
                    .into()
            });
        let mut resource = Self::default();
        resource.insert(KeyValue {
            key: Cow::Borrowed(SERVICE_NAME),
            value: service,
        });
        resource.insert(KeyValue::new(TELEMETRY_SDK_NAME, TELEMETRY_SDK.0));
        resource.insert(KeyValue::new(TELEMETRY_SDK_LANGUAGE, TELEMETRY_SDK.1));
        resource.insert(KeyValue::new(TELEMETRY_SDK_VERSION, TELEMETRY_SDK.2));
        for pair in environment_attributes().attributes {
            resource.insert(pair);
        }
        resource
    }
}

pub fn init(registry: Registry) -> Result<(), ObservabilityError> {
    tracing::dispatcher::set_global_default(tracing::Dispatch::new(registry))
        .map_err(|_| ObservabilityError::Subscriber)?;
    log::set_logger(&log_bridge::LOG_BRIDGE).map_err(|_| ObservabilityError::Subscriber)?;
    log::set_max_level(log_bridge::log_level_filter(
        tracing::level_filters::LevelFilter::current(),
    ));
    Ok(())
}

fn tracer_provider(
    endpoint: Option<&str>,
    file: Option<file::TraceProcessor>,
    resource: &Resource,
    scope: Scope,
) -> Result<Tracer, ObservabilityError> {
    let mut processors: Vec<Box<dyn SpanProcessor>> = Vec::new();
    if let Some(file) = file {
        processors.push(Box::new(file));
    }
    if let Some(endpoint) = endpoint {
        let exporter =
            OtlpSpanExporter::build(&signal_endpoint(endpoint, "traces"), resource, &scope)?;
        processors.push(Box::new(BatchSpanProcessor::new(
            exporter,
            BatchConfig::default().with_env(),
        )));
    }
    Ok(Tracer::new(processors))
}

fn meter_provider(
    endpoint: Option<&str>,
    file: Option<file::MetricExporter>,
    resource: &Resource,
) -> Result<Arc<Meter>, ObservabilityError> {
    let scope = Scope {
        name: Cow::Borrowed(METER_SCOPE),
        version: Some(Cow::Borrowed(METER_SCOPE_VERSION)),
    };
    let mut meter = Meter::new();
    if let Some(file) = file {
        meter.add(Box::new(file), resource, &scope);
    }
    if let Some(endpoint) = endpoint {
        let exporter = otlp_http::MetricExporter::build(&signal_endpoint(endpoint, "metrics"))?;
        meter.add(Box::new(exporter), resource, &scope);
    }
    Ok(Arc::new(meter))
}

fn terminal_target(target: &str) -> bool {
    target
        .strip_prefix(TUI_TARGET)
        .is_some_and(|module| module.is_empty() || module.starts_with("::"))
}

fn signal_endpoint(endpoint: &str, signal: &str) -> String {
    let base = endpoint
        .strip_suffix("/v1/traces")
        .or_else(|| endpoint.strip_suffix("/v1/metrics"))
        .unwrap_or(endpoint);
    format!("{base}/v1/{signal}")
}

impl ObservabilityGuard {
    pub fn shutdown(&mut self) -> Result<(), ObservabilityError> {
        let timeout = self.shutdown_timeout;
        let tracer = self.tracer_provider.take();
        let meter = self.meter_provider.take();
        let (tracer, meter) = std::thread::scope(|scope| {
            let tracer = scope.spawn(move || {
                tracer.map(|provider| {
                    provider.shutdown_with_timeout(timeout.unwrap_or(TRACER_SHUTDOWN_DEFAULT))
                })
            });
            let meter = meter.map(|provider| provider.shutdown());
            (tracer.join(), meter)
        });
        if let Ok(tracer) = tracer {
            tracer.transpose()?;
        }
        meter.transpose()?;
        Ok(())
    }
}

impl Drop for ObservabilityGuard {
    fn drop(&mut self) {
        drop(self.shutdown());
    }
}
