use std::{borrow::Cow, collections::BTreeMap};

use nanocode_subagents::{AgentId, AgentUpdate, ScopedAgentUpdate};
use serde::{Serialize, Serializer, de::Error as _, ser::SerializeMap};
use serde_json::{
    Value,
    value::{RawValue, to_raw_value},
};

use crate::{
    SessionEvent,
    events::{AgentEvent, AgentEventKind},
    session_log::LoggedEvent,
};

const IMPLIED_PROTOCOL_VERSION: u32 = 1;
const DERIVED: &str = "derived";

type Object = BTreeMap<String, Box<RawValue>>;
type Expanded = Result<Option<Box<RawValue>>, serde_json::Error>;

pub(crate) struct Stored<'a> {
    pub(crate) event: &'a SessionEvent,
    pub(crate) compact: Option<&'a str>,
}

impl Serialize for Stored<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match (self.event, self.compact) {
            (SessionEvent::Agent { event, .. }, compact) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("source", "agent")?;
                map.serialize_entry(
                    "event",
                    &Event {
                        event,
                        root: true,
                        compact,
                    },
                )?;
                map.end()
            }
            (
                SessionEvent::Subagent(ScopedAgentUpdate {
                    detach,
                    root_session_id,
                    update: AgentUpdate::Event { id, parent, event },
                }),
                compact @ Some(_),
            ) => {
                let mut map = serializer.serialize_map(Some(4))?;
                map.serialize_entry("source", "subagent")?;
                map.serialize_entry("root_session_id", root_session_id)?;
                map.serialize_entry("detach", detach)?;
                map.serialize_entry(
                    "update",
                    &ChildEvent {
                        id,
                        parent: *parent,
                        event: Event {
                            event,
                            root: false,
                            compact,
                        },
                    },
                )?;
                map.end()
            }
            (event, _) => event.serialize(serializer),
        }
    }
}

struct ChildEvent<'a> {
    id: &'a AgentId,
    parent: Option<AgentId>,
    event: Event<'a>,
}

impl Serialize for ChildEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("type", "event")?;
        map.serialize_entry("id", self.id)?;
        map.serialize_entry("parent", &self.parent)?;
        map.serialize_entry("event", &self.event)?;
        map.end()
    }
}

struct Event<'a> {
    event: &'a AgentEvent,
    root: bool,
    compact: Option<&'a str>,
}

impl Serialize for Event<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Self {
            event,
            root,
            compact,
        } = *self;
        let slim = root.then(|| without_timing(&event.payload)).flatten();
        let payload = slim.as_deref().unwrap_or(&event.payload);
        let deduplicated = (compact.is_some() && event.kind == AgentEventKind::ToolResult)
            .then(|| deduplicated(payload))
            .flatten();
        let mut map = serializer.serialize_map(None)?;
        if compact.is_none() || event.protocol_version != IMPLIED_PROTOCOL_VERSION {
            map.serialize_entry("protocol_version", &event.protocol_version)?;
        }
        if !root || compact != Some(&*event.request_id) {
            map.serialize_entry("request_id", &event.request_id)?;
        }
        map.serialize_entry("seq", &event.seq)?;
        map.serialize_entry("type", &event.kind)?;
        map.serialize_entry("payload", deduplicated.as_deref().unwrap_or(payload))?;
        map.end()
    }
}

fn without_timing(payload: &RawValue) -> Option<Box<RawValue>> {
    if !payload.get().contains("\"timing\"") {
        return None;
    }
    let Ok(Value::Object(mut fields)) = serde_json::from_str::<Value>(payload.get()) else {
        return None;
    };
    fields.remove("timing")?;
    to_raw_value(&fields).ok()
}

fn deduplicated(payload: &RawValue) -> Option<Box<RawValue>> {
    let mut fields: Object = serde_json::from_str(payload.get()).ok()?;
    let structured = fields.remove("structured_result")?;
    let result = fields.remove("result")?;
    let mut wrapper = serde_json::from_str::<Object>(result.get()).ok();
    let text = match &wrapper {
        Some(wrapper) => wrapper.get("text")?,
        None => &result,
    };
    let derived = if same(text, &structured) {
        fields.insert("result".into(), result);
        "structured_result"
    } else if encodes(text, &structured) {
        fields.insert("structured_result".into(), structured);
        let Some(mut wrapper) = wrapper.take() else {
            return finish(fields, "result");
        };
        wrapper.remove("text");
        fields.insert("result".into(), to_raw_value(&wrapper).ok()?);
        "result.text"
    } else {
        return None;
    };
    finish(fields, derived)
}

fn finish(mut fields: Object, derived: &str) -> Option<Box<RawValue>> {
    fields.insert(DERIVED.into(), to_raw_value(derived).ok()?);
    to_raw_value(&fields).ok()
}

fn same(left: &RawValue, right: &RawValue) -> bool {
    if left.get() == right.get() {
        return true;
    }
    left.get().len() == right.get().len()
        && matches!(
            (
                serde_json::from_str::<Value>(left.get()),
                serde_json::from_str::<Value>(right.get()),
            ),
            (Ok(left), Ok(right)) if left == right
        )
}

fn encodes(text: &RawValue, structured: &RawValue) -> bool {
    text.get().len() >= structured.get().len() + 2
        && serde_json::from_str::<Cow<'_, str>>(text.get())
            .is_ok_and(|text| text == structured.get())
}

impl LoggedEvent {
    pub fn expanded(&self, session_id: &str) -> Result<Self, serde_json::Error> {
        Ok(Self {
            event: expand_body(&self.event, session_id)?.unwrap_or_else(|| self.event.clone()),
            request_id: self.request_id.clone(),
            ..*self
        })
    }
}

fn expand_body(body: &RawValue, session_id: &str) -> Expanded {
    let Ok(fields) = serde_json::from_str::<Object>(body.get()) else {
        return Ok(None);
    };
    let source = fields
        .get("source")
        .map(|source| serde_json::from_str::<Cow<'_, str>>(source.get()))
        .transpose()?;
    let agent = match source.as_deref() {
        Some("agent") => true,
        Some("subagent") => false,
        _ => return Ok(None),
    };
    if agent {
        return replace(fields, "event", |event| {
            expand_event(event, Some(session_id))
        });
    }
    replace(fields, "update", |update| {
        let fields: Object = serde_json::from_str(update.get())?;
        if !fields.contains_key("event") {
            return Ok(None);
        }
        replace(fields, "event", |event| expand_event(event, None))
    })
}

fn replace(mut fields: Object, key: &str, expand: impl FnOnce(&RawValue) -> Expanded) -> Expanded {
    let Some(value) = expand(field(&fields, key)?)? else {
        return Ok(None);
    };
    fields.insert(key.into(), value);
    to_raw_value(&fields).map(Some)
}

fn expand_event(event: &RawValue, session_id: Option<&str>) -> Expanded {
    let mut fields: Object = serde_json::from_str(event.get())?;
    let mut changed = false;
    if !fields.contains_key("protocol_version") {
        fields.insert(
            "protocol_version".into(),
            to_raw_value(&IMPLIED_PROTOCOL_VERSION)?,
        );
        changed = true;
    }
    if let Some(session_id) = session_id
        && !fields.contains_key("request_id")
    {
        fields.insert("request_id".into(), to_raw_value(session_id)?);
        changed = true;
    }
    let kind: Cow<'_, str> = serde_json::from_str(field(&fields, "type")?.get())?;
    if kind == "tool.result"
        && let Some(payload) = expand_tool_result(field(&fields, "payload")?)?
    {
        fields.insert("payload".into(), payload);
        changed = true;
    }
    changed.then(|| to_raw_value(&fields)).transpose()
}

fn expand_tool_result(payload: &RawValue) -> Expanded {
    let mut fields: Object = serde_json::from_str(payload.get())?;
    let Some(derived) = fields.remove(DERIVED) else {
        return Ok(None);
    };
    let derived: Cow<'_, str> = serde_json::from_str(derived.get())?;
    let (name, value) = match &*derived {
        "structured_result" => {
            let result = field(&fields, "result")?;
            let text = match serde_json::from_str::<Object>(result.get()) {
                Ok(mut wrapper) => wrapper
                    .remove("text")
                    .ok_or_else(|| missing("result.text"))?,
                Err(_) => result.to_owned(),
            };
            ("structured_result", text)
        }
        "result" => (
            "result",
            to_raw_value(field(&fields, "structured_result")?.get())?,
        ),
        "result.text" => {
            let mut wrapper: Object = serde_json::from_str(field(&fields, "result")?.get())?;
            wrapper.insert(
                "text".into(),
                to_raw_value(field(&fields, "structured_result")?.get())?,
            );
            ("result", to_raw_value(&wrapper)?)
        }
        other => {
            return Err(serde_json::Error::custom(format!(
                "unknown derived tool result field `{other}`"
            )));
        }
    };
    fields.insert(name.into(), value);
    to_raw_value(&fields).map(Some)
}

fn field<'a>(fields: &'a Object, name: &str) -> Result<&'a RawValue, serde_json::Error> {
    fields
        .get(name)
        .map(Box::as_ref)
        .ok_or_else(|| missing(name))
}

fn missing(field: &str) -> serde_json::Error {
    serde_json::Error::custom(format!("stored agent event is missing `{field}`"))
}
