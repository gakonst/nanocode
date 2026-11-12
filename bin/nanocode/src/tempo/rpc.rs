use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use reqwest::Url;
use serde_json::{Value, json};

const MAX_RATE_LIMIT_RETRIES: u32 = 20;
const INITIAL_BACKOFF: Duration = Duration::from_millis(10);
const COMPUTE_UNITS_PER_SECOND: u64 = 1_000;
const AVERAGE_REQUEST_COST: u64 = 20;
const MAX_BACKOFF_HINT: Duration = Duration::from_secs(5 * 60);
const TOO_MANY_REQUESTS: u16 = 429;
const SERVICE_UNAVAILABLE: u16 = 503;
const RATE_LIMIT_CODES: [i64; 4] = [429, -32005, -32055, 1008];
const RATE_LIMIT_MESSAGES: [&str; 6] = [
    "rate limit",
    "rate exceeded",
    "too many requests",
    "credits limited",
    "request limit",
    "maximum number of concurrent requests",
];

#[derive(Debug, Clone)]
pub(crate) enum RpcFault {
    Http {
        status: u16,
        body: String,
        retry_after: Option<Duration>,
    },
    Response {
        code: i64,
        message: String,
        data: Option<Value>,
    },
    Transport(String),
    Decode(String),
    NullResponse,
    MaxRetries(String),
}

impl fmt::Display for RpcFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http { status, body, .. } if body.is_empty() => {
                write!(f, "HTTP error {status} with empty body")
            }
            Self::Http { status, body, .. } => write!(f, "HTTP error {status} with body: {body}"),
            Self::Response {
                code,
                message,
                data,
            } => {
                write!(
                    f,
                    "server returned an error response: error code {code}: {message}"
                )?;
                match data {
                    Some(data) => write!(f, ", data: {data}"),
                    None => Ok(()),
                }
            }
            Self::Transport(error) => write!(f, "{error}"),
            Self::Decode(error) => write!(f, "deserialization error: {error}"),
            Self::NullResponse => {
                f.write_str("server returned a null response when a non-null response was expected")
            }
            Self::MaxRetries(error) => write!(f, "Max retries exceeded {error}"),
        }
    }
}

impl std::error::Error for RpcFault {}

impl RpcFault {
    pub(crate) fn message(&self) -> Option<&str> {
        match self {
            Self::Response { message, .. } => Some(message),
            _ => None,
        }
    }

    pub(crate) const fn code(&self) -> Option<i64> {
        match self {
            Self::Response { code, .. } => Some(*code),
            _ => None,
        }
    }

    pub(crate) const fn data(&self) -> Option<&Value> {
        match self {
            Self::Response { data, .. } => data.as_ref(),
            _ => None,
        }
    }

    fn retryable(&self) -> bool {
        match self {
            Self::Http {
                status,
                retry_after: Some(_),
                ..
            } => *status >= 400,
            Self::Http { status, .. } => {
                *status == TOO_MANY_REQUESTS || *status == SERVICE_UNAVAILABLE
            }
            Self::Response { code, message, .. } => rate_limited(*code, message),
            Self::Transport(error) => error.contains("429 Too Many Requests"),
            Self::NullResponse => true,
            Self::Decode(_) | Self::MaxRetries(_) => false,
        }
    }

    fn backoff_hint(&self) -> Option<Duration> {
        let hint = match self {
            Self::Http {
                retry_after: Some(after),
                ..
            } => Some(*after),
            Self::Response { message, data, .. } => {
                let seconds = data.as_ref().map(|data| &data["rate"]["backoff_seconds"]);
                match seconds {
                    Some(seconds) if seconds.as_u64().is_some() => {
                        seconds.as_u64().map(Duration::from_secs)
                    }
                    Some(seconds) if seconds.as_f64().is_some() => seconds
                        .as_f64()
                        .map(|seconds| Duration::from_secs((seconds as u64).saturating_add(1))),
                    _ => retry_after(message),
                }
            }
            _ => None,
        };
        hint.map(|hint| hint.min(MAX_BACKOFF_HINT))
    }
}

fn rate_limited(code: i64, message: &str) -> bool {
    RATE_LIMIT_CODES.contains(&code)
        || (code == -32016 && message.contains("rate limit"))
        || (code == -32012 && message.contains("credits"))
        || (code == -32007 && message.contains("request limit reached"))
        || matches!(
            message,
            "header not found" | "daily request count exceeded, request rate limited"
        )
        || RATE_LIMIT_MESSAGES
            .iter()
            .any(|needle| message.contains(needle))
}

fn retry_after(message: &str) -> Option<Duration> {
    let after = message.split_once("try again in ")?.1.trim_start();
    let length = after.bytes().take_while(u8::is_ascii_digit).count();
    let (digits, rest) = after.split_at(length);
    let value: u64 = digits.parse().ok()?;
    match rest
        .trim()
        .trim_end_matches(|character: char| character.is_ascii_punctuation())
    {
        "ms" => Some(Duration::from_millis(value)),
        "s" => Some(Duration::from_secs(value)),
        _ => None,
    }
}

#[derive(Clone)]
pub(crate) struct Rpc {
    client: reqwest::Client,
    url: Url,
    queued: Arc<AtomicU32>,
    ids: Arc<AtomicU64>,
}

struct Queued(Arc<AtomicU32>);

impl Drop for Queued {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Rpc {
    pub(crate) fn new(client: Option<&reqwest::Client>, url: Url) -> Self {
        Self {
            client: client.cloned().unwrap_or_default(),
            url,
            queued: Arc::default(),
            ids: Arc::default(),
        }
    }

    async fn once(&self, body: &[u8]) -> Result<Value, RpcFault> {
        let response = self
            .client
            .post(self.url.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_vec())
            .send()
            .await
            .map_err(|error| RpcFault::Transport(error.to_string()))?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let text = response
            .text()
            .await
            .map_err(|error| RpcFault::Transport(error.to_string()))?;
        if !status.is_success() {
            return Err(RpcFault::Http {
                status: status.as_u16(),
                body: text,
                retry_after,
            });
        }
        let answer: Value =
            serde_json::from_str(&text).map_err(|error| RpcFault::Decode(format!("{error}")))?;
        if let Some(error) = answer.get("error") {
            return Err(RpcFault::Response {
                code: error
                    .get("code")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                data: error.get("data").filter(|data| !data.is_null()).cloned(),
            });
        }
        match answer.get("result") {
            Some(result) => Ok(result.clone()),
            None => Err(RpcFault::Decode(format!("no result in {text}"))),
        }
    }

    pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value, RpcFault> {
        let id = self.ids.fetch_add(1, Ordering::Relaxed);
        let body = serde_json::to_vec(&json!({
            "method": method,
            "params": params,
            "id": id,
            "jsonrpc": "2.0",
        }))
        .map_err(|error| RpcFault::Decode(error.to_string()))?;
        self.send(&body).await
    }

    pub(crate) async fn call_without_params(&self, method: &str) -> Result<Value, RpcFault> {
        let id = self.ids.fetch_add(1, Ordering::Relaxed);
        let body = serde_json::to_vec(&json!({
            "method": method,
            "id": id,
            "jsonrpc": "2.0",
        }))
        .map_err(|error| RpcFault::Decode(error.to_string()))?;
        self.send(&body).await
    }

    async fn send(&self, body: &[u8]) -> Result<Value, RpcFault> {
        let ahead = u64::from(self.queued.fetch_add(1, Ordering::SeqCst));
        let _queued = Queued(self.queued.clone());
        let mut retries = 0;
        loop {
            let error = match self.once(body).await {
                Ok(result) => return Ok(result),
                Err(error) => error,
            };
            if !error.retryable() {
                return Err(error);
            }
            retries += 1;
            if retries > MAX_RATE_LIMIT_RETRIES {
                return Err(RpcFault::MaxRetries(error.to_string()));
            }
            let current = u64::from(self.queued.load(Ordering::SeqCst));
            let capacity = (COMPUTE_UNITS_PER_SECOND / AVERAGE_REQUEST_COST).max(1);
            let budget = match current > capacity {
                true => current.min(ahead) / capacity,
                false => 0,
            };
            let backoff = error.backoff_hint().unwrap_or(INITIAL_BACKOFF);
            tokio::time::sleep(backoff.saturating_add(Duration::from_secs(budget))).await;
        }
    }

    pub(crate) async fn call_as<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, RpcFault> {
        let result = self.call(method, params).await?;
        if result.is_null() {
            return Err(RpcFault::NullResponse);
        }
        serde_json::from_value(result).map_err(|error| RpcFault::Decode(error.to_string()))
    }
}
