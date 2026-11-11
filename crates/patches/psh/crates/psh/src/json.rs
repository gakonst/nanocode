//! A small JSON tree serialized through deser-json (D029).

use std::borrow::Cow;

use deser_core::ser::{Emit, SeqEmitter, Serialize, SerializeHandle, StructEmitter};
use deser_core::{Atom, Error, State};

/// A JSON value. Objects keep their keys in insertion order.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(u64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(&'static str, Json)>),
}

impl Json {
    /// An object builder: `Json::obj().set("a", 1)`.
    pub fn obj() -> Json {
        Json::Obj(Vec::new())
    }

    pub fn set(mut self, key: &'static str, value: impl Into<Json>) -> Json {
        if let Json::Obj(fields) = &mut self {
            fields.push((key, value.into()));
        }
        self
    }

    pub fn to_pretty(&self) -> String {
        let config = deser_json::SerializerConfigBuilder::new()
            .pretty(deser_json::Indent::Spaces(2))
            .inline(deser_json::InlinePolicy::LeafIfFits(100))
            .build();
        config
            .to_string(self)
            .expect("a JSON tree always serializes")
    }

    pub fn to_compact(&self) -> String {
        deser_json::to_string(self).expect("a JSON tree always serializes")
    }
}

impl From<bool> for Json {
    fn from(v: bool) -> Json {
        Json::Bool(v)
    }
}

impl From<u32> for Json {
    fn from(v: u32) -> Json {
        Json::Num(v as u64)
    }
}

impl From<usize> for Json {
    fn from(v: usize) -> Json {
        Json::Num(v as u64)
    }
}

impl From<&str> for Json {
    fn from(v: &str) -> Json {
        Json::Str(v.to_string())
    }
}

impl From<String> for Json {
    fn from(v: String) -> Json {
        Json::Str(v)
    }
}

impl From<Vec<Json>> for Json {
    fn from(v: Vec<Json>) -> Json {
        Json::Arr(v)
    }
}

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(v: Option<T>) -> Json {
        v.map_or(Json::Null, Into::into)
    }
}

impl Serialize for Json {
    fn serialize<'a>(value: &'a Self, state: &mut State) -> Result<Emit<'a>, Error> {
        Ok(match value {
            Json::Null => Emit::Atom(Atom::Null),
            Json::Bool(b) => Emit::Atom(Atom::Bool(*b)),
            Json::Num(n) => Emit::Atom(Atom::U64(*n)),
            Json::Str(s) => return String::serialize(s, state),
            Json::Arr(items) => Emit::seq(Items { items, index: 0 }, state),
            Json::Obj(fields) => Emit::structure(Fields { fields, index: 0 }, state),
        })
    }
}

struct Items<'a> {
    items: &'a [Json],
    index: usize,
}

impl SeqEmitter for Items<'_> {
    fn next(&mut self, _state: &mut State) -> Result<Option<SerializeHandle<'_>>, Error> {
        let item = self.items.get(self.index);
        self.index += 1;
        Ok(item.map(SerializeHandle::to))
    }
}

struct Fields<'a> {
    fields: &'a [(&'static str, Json)],
    index: usize,
}

impl StructEmitter for Fields<'_> {
    fn next(
        &mut self,
        _state: &mut State,
    ) -> Result<Option<(Cow<'_, str>, SerializeHandle<'_>)>, Error> {
        let field = self.fields.get(self.index);
        self.index += 1;
        Ok(field.map(|(k, v)| (Cow::Borrowed(*k), SerializeHandle::to(v))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes() {
        let j = Json::obj()
            .set("a", 1u32)
            .set("b", vec![Json::Null, true.into(), "x\"y".into()])
            .set("c", Json::obj());
        assert_eq!(j.to_compact(), r#"{"a":1,"b":[null,true,"x\"y"],"c":{}}"#);
    }
}
