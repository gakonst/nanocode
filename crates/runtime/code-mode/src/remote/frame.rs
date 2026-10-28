use std::borrow::Cow;

use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor, value::MapAccessDeserializer},
};
use serde_json::Value;

use super::{RemoteContext, RemoteFrame, RemoteOperation};
use crate::ToolDefinition;

#[derive(Deserialize)]
struct Catalog {
    definitions: Vec<ToolDefinition>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    runtime_id: Option<String>,
    #[serde(default)]
    max_frame_bytes: Option<usize>,
    #[serde(default)]
    resumable: bool,
    #[serde(default)]
    tool_selection: bool,
    #[serde(default)]
    cancels_calls: bool,
    #[serde(default)]
    caches_local_tools: bool,
}

#[derive(Deserialize)]
struct Request {
    id: u64,
    operation: RemoteOperation,
    #[serde(default)]
    max_frame_bytes: Option<usize>,
    #[serde(default)]
    compact: bool,
    #[serde(default)]
    output_prefix: bool,
    #[serde(default)]
    resumable: bool,
    #[serde(default)]
    answers: bool,
    #[serde(default)]
    asks: bool,
}

#[derive(Deserialize)]
struct Reply {
    id: u64,
    result: Result<Option<Value>, String>,
}

#[derive(Deserialize)]
struct Update {
    id: u64,
    update: Value,
}

#[derive(Deserialize)]
struct Callback {
    id: u64,
    name: String,
    input: Value,
    freeform: bool,
    context: RemoteContext,
    #[serde(default)]
    hand: Option<String>,
    #[serde(default)]
    compact: bool,
    #[serde(default)]
    output_prefix: bool,
    #[serde(default)]
    answers: bool,
    #[serde(default)]
    asked: bool,
}

#[derive(Deserialize)]
struct Id {
    id: u64,
}

#[derive(Deserialize)]
struct Detached {
    session_id: String,
    origin_call_id: String,
    #[serde(default)]
    update: Option<Value>,
}

#[derive(Deserialize)]
struct CallbackResult {
    id: u64,
    result: Result<Value, String>,
}

#[derive(Deserialize)]
struct Ids {
    ids: Vec<u64>,
}

#[derive(Deserialize)]
struct Pending {
    pending: Vec<u64>,
}

const TYPES: &[&str] = &[
    "catalog",
    "request",
    "reply",
    "update",
    "callback",
    "callback_cancel",
    "detached_update",
    "detached_complete",
    "callback_result",
    "ack",
    "resume",
];

fn body<'de, D: Deserializer<'de>>(tag: &str, body: D) -> Result<RemoteFrame, D::Error> {
    Ok(match tag {
        "catalog" => {
            let Catalog {
                definitions,
                name,
                workspace,
                runtime_id,
                max_frame_bytes,
                resumable,
                tool_selection,
                cancels_calls,
                caches_local_tools,
            } = Catalog::deserialize(body)?;
            RemoteFrame::Catalog {
                definitions,
                name,
                workspace,
                runtime_id,
                max_frame_bytes,
                resumable,
                tool_selection,
                cancels_calls,
                caches_local_tools,
            }
        }
        "request" => {
            let Request {
                id,
                operation,
                max_frame_bytes,
                compact,
                output_prefix,
                resumable,
                answers,
                asks,
            } = Request::deserialize(body)?;
            RemoteFrame::Request {
                id,
                operation,
                max_frame_bytes,
                compact,
                output_prefix,
                resumable,
                answers,
                asks,
            }
        }
        "reply" => {
            let Reply { id, result } = Reply::deserialize(body)?;
            RemoteFrame::Reply { id, result }
        }
        "update" => {
            let Update { id, update } = Update::deserialize(body)?;
            RemoteFrame::Update { id, update }
        }
        "callback" => {
            let Callback {
                id,
                name,
                input,
                freeform,
                context,
                hand,
                compact,
                output_prefix,
                answers,
                asked,
            } = Callback::deserialize(body)?;
            RemoteFrame::Callback {
                id,
                name,
                input,
                freeform,
                context,
                hand,
                compact,
                output_prefix,
                answers,
                asked,
            }
        }
        "callback_cancel" => RemoteFrame::CallbackCancel {
            id: Id::deserialize(body)?.id,
        },
        "detached_update" => {
            let Detached {
                session_id,
                origin_call_id,
                update,
            } = Detached::deserialize(body)?;
            RemoteFrame::DetachedUpdate {
                session_id,
                origin_call_id,
                update: update.ok_or_else(|| de::Error::missing_field("update"))?,
            }
        }
        "detached_complete" => {
            let Detached {
                session_id,
                origin_call_id,
                ..
            } = Detached::deserialize(body)?;
            RemoteFrame::DetachedComplete {
                session_id,
                origin_call_id,
            }
        }
        "callback_result" => {
            let CallbackResult { id, result } = CallbackResult::deserialize(body)?;
            RemoteFrame::CallbackResult { id, result }
        }
        "ack" => RemoteFrame::Ack {
            ids: Ids::deserialize(body)?.ids,
        },
        "resume" => RemoteFrame::Resume {
            pending: Pending::deserialize(body)?.pending,
        },
        other => return Err(de::Error::unknown_variant(other, TYPES)),
    })
}

struct FrameVisitor;

impl<'de> Visitor<'de> for FrameVisitor {
    type Value = RemoteFrame;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a remote frame object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<RemoteFrame, A::Error> {
        let Some(key) = map.next_key::<Cow<'de, str>>()? else {
            return Err(de::Error::missing_field("type"));
        };
        if key == "type" {
            let tag = map.next_value::<Cow<'de, str>>()?;
            return body(&tag, MapAccessDeserializer::new(map));
        }
        let mut fields = serde_json::Map::new();
        fields.insert(key.into_owned(), map.next_value()?);
        while let Some((key, value)) = map.next_entry::<String, Value>()? {
            fields.insert(key, value);
        }
        let Some(Value::String(tag)) = fields.remove("type") else {
            return Err(de::Error::missing_field("type"));
        };
        body(&tag, Value::Object(fields)).map_err(de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for RemoteFrame {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(FrameVisitor)
    }
}
