use std::path::{Path, PathBuf};

use nanocode_arguments::{Argument, Choice, Group, Matches, Parser};

use crate::{LogFormat, LogOutput, METRICS_TARGET, ObservabilityBuilder, Telemetry};

#[derive(Clone, Debug)]
pub struct ObservabilityOutputArgs {
    log_filter: Option<String>,
    log_format: LogFormatArg,
    log_file: Option<PathBuf>,
    telemetry: Option<TelemetryArg>,
    otel_endpoint: Option<String>,
    otel_environment: String,
}

impl ObservabilityOutputArgs {
    pub const ARGUMENTS: Group = Group::named(
        "ObservabilityOutputArgs",
        &[
            Argument::value("log_filter", "LOG_FILTER", Parser::NonEmptyText)
                .long("log")
                .env("NANOCODE_LOG")
                .global()
                .help("Tracing filter in RUST_LOG syntax for the log file and OpenTelemetry export"),
            Argument::value("log_format", "LOG_FORMAT", Parser::Choice(&LogFormatArg::CHOICES))
                .long("log-format")
                .env("NANOCODE_LOG_FORMAT")
                .global()
                .default("compact"),
            Argument::value("log_file", "LOG_FILE", Parser::Path)
                .long("log-file")
                .env("NANOCODE_LOG_FILE")
                .global()
                .help("Write tracing logs to this file"),
            Argument::value("telemetry", "TELEMETRY", Parser::Choice(&TelemetryArg::CHOICES))
                .long("telemetry")
                .env("NANOCODE_TELEMETRY")
                .global()
                .help(
                    "OpenTelemetry traces and metrics to record locally and export [default: agent, and off for nanocode hand]",
                ),
            Argument::value("otel_endpoint", "OTEL_ENDPOINT", Parser::NonEmptyText)
                .long("otel-endpoint")
                .env("OTEL_EXPORTER_OTLP_ENDPOINT")
                .global()
                .help("OTLP/HTTP collector base URL for trace and metric export"),
            Argument::value("otel_environment", "OTEL_ENVIRONMENT", Parser::NonEmptyText)
                .long("otel-environment")
                .env("OTEL_DEPLOYMENT_ENVIRONMENT")
                .global()
                .default("development"),
        ],
    );

    #[must_use]
    pub fn from_matches(matches: &Matches) -> Self {
        Self {
            log_filter: matches.text("log_filter"),
            log_format: LogFormatArg::ALL[matches
                .choice("log_format")
                .expect("log_format has a default")],
            log_file: matches.path("log_file"),
            telemetry: matches
                .choice("telemetry")
                .map(|index| TelemetryArg::ALL[index]),
            otel_endpoint: matches.text("otel_endpoint"),
            otel_environment: matches
                .text("otel_environment")
                .expect("otel_environment has a default"),
        }
    }

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

#[derive(Clone, Copy, Debug)]
enum TelemetryArg {
    Off,
    Agent,
    All,
}

impl TelemetryArg {
    const ALL: [Self; 3] = [Self::Off, Self::Agent, Self::All];
    const CHOICES: [Choice; 3] = [
        Choice::new("off").help("Record no traces or metrics"),
        Choice::new("agent").help(
            "Agent turns, model calls, tools and durable writes, plus terminal frame metrics",
        ),
        Choice::new("all")
            .help("Agent work and a span for every terminal frame, layout, render and draw"),
    ];
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

#[derive(Clone, Copy, Debug)]
enum LogFormatArg {
    Pretty,
    Compact,
    Json,
}

impl LogFormatArg {
    const ALL: [Self; 3] = [Self::Pretty, Self::Compact, Self::Json];
    const CHOICES: [Choice; 3] = [
        Choice::new("pretty"),
        Choice::new("compact"),
        Choice::new("json"),
    ];
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
