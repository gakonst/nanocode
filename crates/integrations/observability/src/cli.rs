use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum, builder::NonEmptyStringValueParser};

use crate::{LogFormat, LogOutput, METRICS_TARGET, ObservabilityBuilder, Telemetry};

#[derive(Clone, Debug, Args)]
pub struct ObservabilityOutputArgs {
    #[arg(
        long = "log",
        env = "NANOCODE_LOG",
        global = true,
        value_parser = NonEmptyStringValueParser::new(),
        help = "Tracing filter in RUST_LOG syntax for the log file and OpenTelemetry export"
    )]
    log_filter: Option<String>,

    #[arg(
        long,
        env = "NANOCODE_LOG_FORMAT",
        global = true,
        default_value_t,
        value_enum
    )]
    log_format: LogFormatArg,

    #[arg(
        long,
        env = "NANOCODE_LOG_FILE",
        global = true,
        help = "Write tracing logs to this file"
    )]
    log_file: Option<PathBuf>,

    #[arg(
        long,
        env = "NANOCODE_TELEMETRY",
        global = true,
        value_enum,
        help = "OpenTelemetry traces and metrics to record locally and export [default: agent, and off for nanocode hand]"
    )]
    telemetry: Option<TelemetryArg>,

    #[arg(
        long,
        env = "OTEL_EXPORTER_OTLP_ENDPOINT",
        global = true,
        value_parser = NonEmptyStringValueParser::new(),
        help = "OTLP/HTTP collector base URL for trace and metric export"
    )]
    otel_endpoint: Option<String>,

    #[arg(
        long,
        env = "OTEL_DEPLOYMENT_ENVIRONMENT",
        global = true,
        default_value = "development",
        value_parser = NonEmptyStringValueParser::new()
    )]
    otel_environment: String,
}

impl ObservabilityOutputArgs {
    #[must_use]
    pub fn log_file(&self) -> Option<&Path> {
        self.log_file.as_deref()
    }

    #[must_use]
    pub fn log_filter(&self) -> Option<&str> {
        self.log_filter.as_deref()
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.otel_endpoint.is_some() || self.log_file.is_some() || self.log_filter.is_some()
    }

    #[must_use]
    pub fn builder(
        &self,
        service_name: impl Into<String>,
        service_version: impl Into<String>,
        default_output: LogOutput,
        default_telemetry: Telemetry,
    ) -> ObservabilityBuilder {
        let output = self
            .log_file
            .clone()
            .map_or(default_output, LogOutput::File);
        let mut builder = ObservabilityBuilder::new(service_name, service_version)
            .format(self.log_format.into())
            .output(output)
            .environment(self.otel_environment.clone())
            .telemetry(self.telemetry.map_or(default_telemetry, Into::into));
        if let Some(filter) = &self.log_filter {
            builder = builder
                .filter(format!("{filter},{METRICS_TARGET}=off"))
                .otel_filter(format!("{filter},{METRICS_TARGET}=info"));
        }
        if let Some(endpoint) = &self.otel_endpoint {
            builder = builder.otlp_endpoint(endpoint.clone());
        }
        builder
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum TelemetryArg {
    #[value(help = "Record no traces or metrics")]
    Off,
    #[value(
        help = "Agent turns, model calls, tools and durable writes, plus terminal frame metrics"
    )]
    Agent,
    #[value(help = "Agent work and a span for every terminal frame, layout, render and draw")]
    All,
}

impl From<TelemetryArg> for Telemetry {
    fn from(telemetry: TelemetryArg) -> Self {
        match telemetry {
            TelemetryArg::Off => Self::Off,
            TelemetryArg::Agent => Self::Agent,
            TelemetryArg::All => Self::All,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum LogFormatArg {
    Pretty,
    #[default]
    Compact,
    Json,
}

impl From<LogFormatArg> for LogFormat {
    fn from(format: LogFormatArg) -> Self {
        match format {
            LogFormatArg::Pretty => Self::Pretty,
            LogFormatArg::Compact => Self::Compact,
            LogFormatArg::Json => Self::Json,
        }
    }
}
