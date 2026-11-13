use std::{
    env,
    str::FromStr,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use nanocode_http::{
    blocking::{Client, Response},
    header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT},
};

use crate::{
    batch,
    metrics::{self, ResourceMetrics, Temporality},
    model::{ExportError, Exported, Resource, Scope, SpanData, Value},
    protobuf,
};

const PROTOBUF_CONTENT_TYPE: &str = "application/x-protobuf";
const EXPORTER_USER_AGENT: &str = "OTel-OTLP-Exporter-Rust/0.32.0";
const OTEL_EXPORTER_OTLP_HEADERS: &str = "OTEL_EXPORTER_OTLP_HEADERS";
const OTEL_EXPORTER_OTLP_COMPRESSION: &str = "OTEL_EXPORTER_OTLP_COMPRESSION";
const OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE: &str =
    "OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE";
const HEADER_SEPARATOR: char = ',';
const HEADER_ASSIGNMENT: char = '=';
const PERCENT: char = '%';
const PERCENT_ENCODED_DIGITS: usize = 2;
const DECODE_FAILURE: &str = "error decoding response body";
const HEX_RADIX: u32 = 16;
const ALREADY_SHUT_DOWN: &str = "Exporter already shutdown";

struct Signal {
    headers: &'static str,
    compression: &'static str,
}

const TRACES: Signal = Signal {
    headers: "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
    compression: "OTEL_EXPORTER_OTLP_TRACES_COMPRESSION",
};

const METRICS: Signal = Signal {
    headers: "OTEL_EXPORTER_OTLP_METRICS_HEADERS",
    compression: "OTEL_EXPORTER_OTLP_METRICS_COMPRESSION",
};

#[derive(Debug)]
pub enum OtlpError {
    Endpoint {
        endpoint: String,
        reason: String,
    },

    Compression {
        variable: &'static str,
        value: String,
    },

    Temporality {
        variable: &'static str,
        value: String,
    },
}

impl std::fmt::Display for OtlpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Endpoint { endpoint, reason } => {
                write!(formatter, "invalid URI {endpoint}. Reason {reason}")
            }
            Self::Compression { variable, value } => write!(
                formatter,
                "unsupported compression algorithm '{value}' in {variable}: OTLP is sent uncompressed"
            ),
            Self::Temporality { variable, value } => write!(
                formatter,
                "{variable}: Invalid value '{value}'. Expected: cumulative, delta, or lowmemory"
            ),
        }
    }
}

impl std::error::Error for OtlpError {}

#[derive(Debug)]
struct Http {
    client: Client,
    url: String,
    headers: HeaderMap,
    body_bytes: AtomicUsize,
}

impl Http {
    fn build(signal: &Signal, endpoint: &str) -> Result<Self, OtlpError> {
        let client = Client::builder()
            .default_headers(nanocode_http::accept_encoding(HeaderMap::new()))
            .build()
            .expect("the default blocking HTTP client builds");
        let url = http::Uri::from_str(endpoint)
            .map_err(|error| OtlpError::Endpoint {
                endpoint: endpoint.to_owned(),
                reason: error.to_string(),
            })?
            .to_string();
        compression(signal)?;
        Ok(Self {
            client,
            url,
            headers: headers(signal),
            body_bytes: AtomicUsize::new(0),
        })
    }

    fn body(&self) -> Vec<u8> {
        Vec::with_capacity(self.body_bytes.load(Ordering::Relaxed))
    }

    fn post(&self, body: Vec<u8>) -> Exported {
        self.body_bytes.store(body.len(), Ordering::Relaxed);
        let response = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .body(body)
            .send()
            .and_then(Response::error_for_status)
            .map_err(export_failure)?;
        let coding = nanocode_http::FlateCoding::from_headers(response.headers());
        let body = response.bytes().map_err(export_failure)?;
        coding.map_or(Ok(()), |coding| {
            nanocode_http::decode_body(coding, &body)
                .map(drop)
                .map_err(|_| {
                    ExportError::InternalFailure(format!("HTTP export failed: {DECODE_FAILURE}"))
                })
        })
    }
}

fn export_failure(error: nanocode_http::Error) -> ExportError {
    ExportError::InternalFailure(format!("HTTP export failed: {}", error.without_url()))
}

fn compression(signal: &Signal) -> Result<(), OtlpError> {
    let (variable, value) = match env::var(signal.compression) {
        Ok(value) => (signal.compression, value),
        Err(_) => match env::var(OTEL_EXPORTER_OTLP_COMPRESSION) {
            Ok(value) => (OTEL_EXPORTER_OTLP_COMPRESSION, value),
            Err(_) => return Ok(()),
        },
    };
    Err(OtlpError::Compression { variable, value })
}

fn headers(signal: &Signal) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static(PROTOBUF_CONTENT_TYPE),
    );
    headers.insert(USER_AGENT, HeaderValue::from_static(EXPORTER_USER_AGENT));
    let Ok(input) = env::var(signal.headers).or_else(|_| env::var(OTEL_EXPORTER_OTLP_HEADERS))
    else {
        return headers;
    };
    for (name, value) in input
        .split_terminator(HEADER_SEPARATOR)
        .map(str::trim)
        .filter_map(|pair| pair.split_once(HEADER_ASSIGNMENT))
    {
        let name = name.trim();
        let value = percent_decoded(value.trim()).unwrap_or_else(|| value.to_owned());
        if name.is_empty() || value.is_empty() {
            continue;
        }
        if let (Ok(name), Ok(value)) = (HeaderName::from_str(name), HeaderValue::from_str(&value)) {
            headers.insert(name, value);
        }
    }
    headers
}

fn percent_decoded(value: &str) -> Option<String> {
    let mut decoded = String::with_capacity(value.len());
    let mut pending = Vec::new();
    let mut characters = value.chars();
    loop {
        let character = characters.next();
        if character == Some(PERCENT) {
            let mut digits = [0_u8; PERCENT_ENCODED_DIGITS * size_of::<char>()];
            let mut length = 0;
            for _ in 0..PERCENT_ENCODED_DIGITS {
                length += characters.next()?.encode_utf8(&mut digits[length..]).len();
            }
            let digits = std::str::from_utf8(&digits[..length]).ok()?;
            pending.push(u8::from_str_radix(digits, HEX_RADIX).ok()?);
            continue;
        }
        if !pending.is_empty() {
            decoded.push_str(std::str::from_utf8(&pending).ok()?);
            pending.clear();
        }
        match character {
            Some(character) => decoded.push(character),
            None => return Some(decoded),
        }
    }
}

#[derive(Debug)]
pub struct SpanExporter {
    http: Http,
    origin: protobuf::Origin,
}

impl SpanExporter {
    pub fn build(endpoint: &str, resource: &Resource, scope: &Scope) -> Result<Self, OtlpError> {
        Ok(Self {
            http: Http::build(&TRACES, endpoint)?,
            origin: protobuf::Origin::of(resource, scope),
        })
    }
}

fn restore_integers(span: &mut SpanData) {
    for attribute in &mut span.attributes {
        if let Value::String(text) = &attribute.value
            && let Some(integer) = crate::otlp::unsigned_integer(text)
        {
            attribute.value = Value::I64(integer);
        }
    }
}

impl batch::SpanExporter for SpanExporter {
    fn export(&self, mut batch: Vec<SpanData>) -> Exported {
        batch.iter_mut().for_each(restore_integers);
        let mut body = self.http.body();
        protobuf::traces(&mut body, &self.origin, &batch);
        self.http.post(body)
    }
}

#[derive(Debug)]
pub(crate) struct MetricExporter {
    http: Http,
    temporality: Temporality,
    shut_down: AtomicBool,
}

impl MetricExporter {
    pub(crate) fn build(endpoint: &str) -> Result<Self, OtlpError> {
        let temporality = match env::var(OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE) {
            Ok(value) => value.parse().map_err(|()| OtlpError::Temporality {
                variable: OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE,
                value,
            })?,
            Err(_) => Temporality::default(),
        };
        Ok(Self {
            http: Http::build(&METRICS, endpoint)?,
            temporality,
            shut_down: AtomicBool::new(false),
        })
    }
}

impl metrics::MetricExporter for MetricExporter {
    fn export(&self, metrics: &ResourceMetrics<'_>) -> Exported {
        if self.shut_down.load(Ordering::Acquire) {
            return Err(ExportError::InternalFailure(ALREADY_SHUT_DOWN.to_owned()));
        }
        let mut body = self.http.body();
        protobuf::metrics(&mut body, metrics);
        self.http.post(body)
    }

    fn shutdown(&self) -> Exported {
        self.shut_down.store(true, Ordering::Release);
        Ok(())
    }

    fn temporality(&self) -> Temporality {
        self.temporality
    }
}
