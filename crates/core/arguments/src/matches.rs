use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Source {
    Default,
    Environment,
    CommandLine,
}

#[derive(Clone, Debug)]
pub(crate) enum Value {
    Flag(bool),
    Text(String),
    Path(PathBuf),
    U64(u64),
    U128(u128),
    Address(SocketAddr),
    Choice(usize),
}

#[derive(Clone, Debug)]
pub(crate) struct Matched {
    pub(crate) id: &'static str,
    pub(crate) source: Source,
    pub(crate) group: bool,
    pub(crate) values: Vec<Value>,
}

#[derive(Debug, Default)]
pub struct Matches {
    pub(crate) entries: Vec<Matched>,
    pub(crate) subcommand: Option<(&'static str, Box<Self>)>,
}

impl Matches {
    pub(crate) fn get(&self, id: &str) -> Option<&Matched> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub(crate) fn contains(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    pub(crate) fn is_explicit(&self, id: &str) -> bool {
        self.get(id)
            .is_some_and(|entry| entry.source != Source::Default)
    }

    pub(crate) fn explicit_ids(&self) -> Vec<&'static str> {
        self.entries
            .iter()
            .filter(|entry| entry.source != Source::Default)
            .map(|entry| entry.id)
            .collect()
    }

    pub(crate) fn remove(&mut self, id: &str) -> bool {
        let position = self.entries.iter().position(|entry| entry.id == id);
        position
            .map(|position| self.entries.remove(position))
            .is_some()
    }

    pub(crate) fn start(&mut self, id: &'static str, group: bool, source: Source) {
        match self.entries.iter_mut().find(|entry| entry.id == id) {
            Some(entry) => entry.source = entry.source.max(source),
            None => self.entries.push(Matched {
                id,
                source,
                group,
                values: Vec::new(),
            }),
        }
    }

    pub(crate) fn push_value(&mut self, id: &str, value: Value) {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .expect("values go to a started argument");
        entry.values.push(value);
    }

    pub(crate) fn insert(&mut self, matched: Matched) {
        match self.entries.iter_mut().find(|entry| entry.id == matched.id) {
            Some(entry) => *entry = matched,
            None => self.entries.push(matched),
        }
    }

    #[must_use]
    pub fn subcommand(&self) -> Option<(&'static str, &Self)> {
        self.subcommand
            .as_ref()
            .map(|(name, matches)| (*name, matches.as_ref()))
    }

    fn values(&self, id: &str) -> &[Value] {
        self.entries
            .iter()
            .find(|entry| !entry.group && entry.id == id)
            .map_or(&[], |entry| entry.values.as_slice())
    }

    fn single(&self, id: &str) -> Option<&Value> {
        let values = self.values(id);
        assert!(values.len() <= 1, "argument {id} holds one value");
        values.first()
    }

    #[must_use]
    pub fn flag(&self, id: &str) -> bool {
        match self.single(id) {
            Some(Value::Flag(value)) => *value,
            None => false,
            Some(other) => panic!("argument {id} is a flag, not {other:?}"),
        }
    }

    #[must_use]
    pub fn text(&self, id: &str) -> Option<String> {
        match self.single(id)? {
            Value::Text(value) => Some(value.clone()),
            other => panic!("argument {id} is text, not {other:?}"),
        }
    }

    #[must_use]
    pub fn texts(&self, id: &str) -> Vec<String> {
        self.values(id)
            .iter()
            .map(|value| match value {
                Value::Text(value) => value.clone(),
                other => panic!("argument {id} is text, not {other:?}"),
            })
            .collect()
    }

    #[must_use]
    pub fn path(&self, id: &str) -> Option<PathBuf> {
        match self.single(id)? {
            Value::Path(value) => Some(value.clone()),
            other => panic!("argument {id} is a path, not {other:?}"),
        }
    }

    #[must_use]
    pub fn u64(&self, id: &str) -> Option<u64> {
        match self.single(id)? {
            Value::U64(value) => Some(*value),
            other => panic!("argument {id} is a u64, not {other:?}"),
        }
    }

    #[must_use]
    pub fn u128(&self, id: &str) -> Option<u128> {
        match self.single(id)? {
            Value::U128(value) => Some(*value),
            other => panic!("argument {id} is a u128, not {other:?}"),
        }
    }

    #[must_use]
    pub fn address(&self, id: &str) -> Option<SocketAddr> {
        match self.single(id)? {
            Value::Address(value) => Some(*value),
            other => panic!("argument {id} is a socket address, not {other:?}"),
        }
    }

    #[must_use]
    pub fn choice(&self, id: &str) -> Option<usize> {
        match self.single(id)? {
            Value::Choice(value) => Some(*value),
            other => panic!("argument {id} is a choice, not {other:?}"),
        }
    }
}
