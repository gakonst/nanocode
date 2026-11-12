use std::{
    collections::BTreeMap,
    fmt,
    io,
};

use serde::ser::{SerializeMap, SerializeSeq, Serializer as _};
use serde_json::{Serializer, Value};
use tracing_core::{
    Event, Level,
    field::{Field, Visit},
    span::Record,
};

use crate::{
    format::{Fields, SpanFields, current_scope, event_meta, write_timestamp},
    log_bridge,
    subscriber::{Context, SpanRef},
};

struct WriteAdaptor<'a>(&'a mut String);

impl io::Write for WriteAdaptor<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let text = std::str::from_utf8(buffer)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        self.0.push_str(text);
        Ok(text.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

const fn level_name(level: Level) -> &'static str {
    match level {
        Level::ERROR => "ERROR",
        Level::WARN => "WARN",
        Level::INFO => "INFO",
        Level::DEBUG => "DEBUG",
        Level::TRACE => "TRACE",
    }
}

pub(crate) fn format_event(
    context: &Context<'_, SpanFields>,
    writer: &mut String,
    event: &Event<'_>,
) -> fmt::Result {
    let mut timestamp = String::new();
    write_timestamp(&mut timestamp)?;
    let normalized = log_bridge::normalize(event);
    let meta = event_meta(event, &normalized);
    let has_current = current_scope(context, event).is_some();
    let serialized = (|| {
        let mut serializer = Serializer::new(WriteAdaptor(writer));
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("timestamp", &timestamp)?;
        map.serialize_entry("level", level_name(meta.level))?;
        map.serialize_entry("fields", &EventFields(event))?;
        map.serialize_entry("target", meta.target)?;
        if has_current {
            map.serialize_entry("spans", &CurrentSpans(context))?;
        }
        SerializeMap::end(map)
    })();
    serialized.map_err(|_| fmt::Error)?;
    writer.push('\n');
    Ok(())
}

struct EventFields<'a, 'b>(&'a Event<'b>);

impl serde::Serialize for EventFields<'_, '_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let map = serializer.serialize_map(Some(self.0.fields().count()))?;
        let mut visitor = MapVisitor {
            map,
            state: Ok(()),
        };
        self.0.record(&mut visitor);
        visitor.state?;
        visitor.map.end()
    }
}

struct MapVisitor<M: SerializeMap> {
    map: M,
    state: Result<(), M::Error>,
}

impl<M: SerializeMap> Visit for MapVisitor<M> {
    fn record_bool(&mut self, field: &Field, value: bool) {
        if self.state.is_ok() {
            self.state = self.map.serialize_entry(field.name(), &value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if self.state.is_ok() {
            self.state = self
                .map
                .serialize_entry(field.name(), &format_args!("{value:?}"));
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if self.state.is_ok() {
            self.state = self.map.serialize_entry(field.name(), &value);
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if self.state.is_ok() {
            self.state = self.map.serialize_entry(field.name(), &value);
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if self.state.is_ok() {
            self.state = self.map.serialize_entry(field.name(), &value);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if self.state.is_ok() {
            self.state = self.map.serialize_entry(field.name(), &value);
        }
    }
}

struct CurrentSpans<'a, 'b>(&'a Context<'b, SpanFields>);

impl serde::Serialize for CurrentSpans<'_, '_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(None)?;
        if let Some(leaf) = self.0.lookup_current() {
            for span in leaf.scope().from_root() {
                sequence.serialize_element(&SerializableSpan(&span))?;
            }
        }
        sequence.end()
    }
}

struct SerializableSpan<'a, 'b>(&'a SpanRef<'b, SpanFields>);

impl serde::Serialize for SerializableSpan<'_, '_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        let data = self.0.data().expect("a visible span has its layer's data");
        let fields = data.text(|text| {
            let text = text.expect("Unable to find FormattedFields in extensions; this is a bug");
            match serde_json::from_str::<Value>(text) {
                Ok(Value::Object(fields)) => fields,
                Ok(_) | Err(_) => panic!(
                    "span '{}' had malformed fields! this is a bug.\n  fields: {:?}",
                    self.0.name(),
                    text
                ),
            }
        });
        for (name, value) in &fields {
            map.serialize_entry(name, value)?;
        }
        map.serialize_entry("name", self.0.name())?;
        map.end()
    }
}

pub(crate) fn format_fields(writer: &mut String, fields: &dyn Fields) -> fmt::Result {
    let mut visitor = JsonVisitor {
        values: BTreeMap::new(),
    };
    fields.record(&mut visitor);
    visitor.finish(writer)
}

pub(crate) fn add_fields(current: &mut String, values: &Record<'_>) -> fmt::Result {
    if current.is_empty() {
        return format_fields(current, values);
    }
    let map: BTreeMap<&str, Value> = serde_json::from_str(current).map_err(|_| fmt::Error)?;
    let mut visitor = JsonVisitor { values: map };
    values.record(&mut visitor);
    let mut new = String::new();
    visitor.finish(&mut new)?;
    *current = new;
    Ok(())
}

struct JsonVisitor<'a> {
    values: BTreeMap<&'a str, Value>,
}

impl JsonVisitor<'_> {
    fn finish(self, writer: &mut String) -> fmt::Result {
        let serialized = (|| {
            let mut serializer = Serializer::new(WriteAdaptor(writer));
            let mut map = serializer.serialize_map(None)?;
            for (key, value) in &self.values {
                map.serialize_entry(key, value)?;
            }
            SerializeMap::end(map)
        })();
        serialized.map_err(|_| fmt::Error)
    }
}

impl Visit for JsonVisitor<'_> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.values.insert(field.name(), Value::from(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.values.insert(field.name(), Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.values.insert(field.name(), Value::from(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.values.insert(field.name(), Value::from(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.values.insert(field.name(), Value::from(value));
    }

    fn record_bytes(&mut self, field: &Field, value: &[u8]) {
        self.values.insert(field.name(), Value::from(value));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        match field.name() {
            name if name.starts_with("log.") => {}
            name if name.starts_with("r#") => {
                self.values
                    .insert(&name[2..], Value::from(format!("{value:?}")));
            }
            name => {
                self.values.insert(name, Value::from(format!("{value:?}")));
            }
        }
    }
}


