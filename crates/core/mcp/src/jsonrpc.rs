use std::{fmt, time::Duration};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::SerializeMap};
use serde_json::Value;

pub const JSONRPC_VERSION: &str = "2.0";
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

impl RequestId {
    #[must_use]
    pub fn numeric_equivalent(&self) -> Option<Self> {
        match self {
            Self::String(value) => value.parse::<i64>().ok().map(Self::Number),
            Self::Number(_) => None,
        }
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(value) => write!(f, "{value}"),
            Self::String(value) => f.write_str(value),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip)]
    status: Option<u16>,
    #[serde(skip)]
    retry_after: Option<Duration>,
}

impl RpcError {
    #[must_use]
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
            status: None,
            retry_after: None,
        }
    }

    #[must_use]
    pub const fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    #[must_use]
    pub const fn with_retry_after(mut self, retry_after: Option<Duration>) -> Self {
        self.retry_after = retry_after;
        self
    }

    #[must_use]
    pub const fn status(&self) -> Option<u16> {
        self.status
    }

    #[must_use]
    pub const fn retry_after(&self) -> Option<Duration> {
        self.retry_after
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if let Some(data) = &self.data {
            write!(f, "({data})")?;
        }
        Ok(())
    }
}

impl std::error::Error for RpcError {}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Request {
        id: RequestId,
        method: String,
        params: Option<Value>,
    },
    Notification {
        method: String,
        params: Option<Value>,
    },
    Response {
        id: RequestId,
        result: Value,
    },
    Error {
        id: Option<RequestId>,
        error: RpcError,
    },
}

impl Message {
    #[must_use]
    pub fn request(id: RequestId, method: impl Into<String>, params: Option<Value>) -> Self {
        Self::Request {
            id,
            method: method.into(),
            params,
        }
    }

    #[must_use]
    pub fn notification(method: impl Into<String>, params: Option<Value>) -> Self {
        Self::Notification {
            method: method.into(),
            params,
        }
    }

    #[must_use]
    pub fn method(&self) -> Option<&str> {
        match self {
            Self::Request { method, .. } | Self::Notification { method, .. } => Some(method),
            Self::Response { .. } | Self::Error { .. } => None,
        }
    }

    #[must_use]
    pub const fn request_id(&self) -> Option<&RequestId> {
        match self {
            Self::Request { id, .. } => Some(id),
            Self::Notification { .. } | Self::Response { .. } | Self::Error { .. } => None,
        }
    }

    #[must_use]
    pub const fn response_id(&self) -> Option<&RequestId> {
        match self {
            Self::Response { id, .. } => Some(id),
            Self::Error { id, .. } => id.as_ref(),
            Self::Request { .. } | Self::Notification { .. } => None,
        }
    }

    #[must_use]
    pub const fn is_response(&self) -> bool {
        matches!(self, Self::Response { .. } | Self::Error { .. })
    }

    #[must_use]
    pub const fn is_request(&self) -> bool {
        matches!(self, Self::Request { .. })
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    pub fn to_vec(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

impl Serialize for Message {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("jsonrpc", JSONRPC_VERSION)?;
        match self {
            Self::Request { id, method, params } => {
                map.serialize_entry("id", id)?;
                map.serialize_entry("method", method)?;
                if let Some(params) = params {
                    map.serialize_entry("params", params)?;
                }
            }
            Self::Notification { method, params } => {
                map.serialize_entry("method", method)?;
                if let Some(params) = params {
                    map.serialize_entry("params", params)?;
                }
            }
            Self::Response { id, result } => {
                map.serialize_entry("id", id)?;
                map.serialize_entry("result", result)?;
            }
            Self::Error { id, error } => {
                map.serialize_entry("id", id)?;
                map.serialize_entry("error", error)?;
            }
        }
        map.end()
    }
}

fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
struct RawMessage {
    jsonrpc: String,
    #[serde(default, deserialize_with = "present")]
    id: Option<Value>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default, deserialize_with = "present")]
    params: Option<Value>,
    #[serde(default, deserialize_with = "present")]
    result: Option<Value>,
    #[serde(default, deserialize_with = "present")]
    error: Option<Value>,
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawMessage::deserialize(deserializer)?;
        if raw.jsonrpc != JSONRPC_VERSION {
            return Err(D::Error::custom(format!(
                "unsupported JSON-RPC version {:?}",
                raw.jsonrpc
            )));
        }
        let id = match raw.id {
            None | Some(Value::Null) => None,
            Some(id) => Some(RequestId::deserialize(id).map_err(D::Error::custom)?),
        };
        if let Some(method) = raw.method {
            return Ok(match id {
                Some(id) => Self::Request {
                    id,
                    method,
                    params: raw.params,
                },
                None => Self::Notification {
                    method,
                    params: raw.params,
                },
            });
        }
        if let Some(error) = raw.error {
            let error = RpcError::deserialize(error).map_err(D::Error::custom)?;
            return Ok(Self::Error { id, error });
        }
        match (id, raw.result) {
            (Some(id), Some(result)) => Ok(Self::Response { id, result }),
            (None, Some(_)) => Err(D::Error::custom("JSON-RPC response is missing its id")),
            (_, None) => Err(D::Error::custom(
                "JSON-RPC message has no method, result or error",
            )),
        }
    }
}
