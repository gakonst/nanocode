//! jq's objects: string keys in insertion order (jq prints keys in the order they were
//! added; `keys` sorts). A vector of entries with a hash index once the object grows.

use std::collections::HashMap;
use std::rc::Rc;

use super::val::Val;

/// Objects with fewer entries are searched linearly.
const INDEX_FROM: usize = 12;

#[derive(Clone, Debug, Default)]
pub struct Map {
    entries: Vec<(Rc<str>, Val)>,
    index: Option<HashMap<Rc<str>, usize>>,
}

impl Map {
    pub fn new() -> Map {
        Map::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn position(&self, k: &str) -> Option<usize> {
        match &self.index {
            Some(ix) => ix.get(k).copied(),
            None => self.entries.iter().position(|(e, _)| &**e == k),
        }
    }

    pub fn get(&self, k: &str) -> Option<&Val> {
        self.position(k).map(|i| &self.entries[i].1)
    }

    pub fn get_mut(&mut self, k: &str) -> Option<&mut Val> {
        self.position(k).map(|i| &mut self.entries[i].1)
    }

    pub fn contains_key(&self, k: &str) -> bool {
        self.position(k).is_some()
    }

    /// Sets a key: an existing one keeps its place, a new one goes last.
    pub fn insert(&mut self, k: Rc<str>, v: Val) {
        if let Some(i) = self.position(&k) {
            self.entries[i].1 = v;
            return;
        }
        if let Some(ix) = &mut self.index {
            ix.insert(k.clone(), self.entries.len());
        }
        self.entries.push((k, v));
        if self.index.is_none() && self.entries.len() >= INDEX_FROM {
            self.reindex();
        }
    }

    pub fn remove(&mut self, k: &str) -> Option<Val> {
        let i = self.position(k)?;
        let (_, v) = self.entries.remove(i);
        if self.index.is_some() {
            self.reindex();
        }
        Some(v)
    }

    fn reindex(&mut self) {
        let ix = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, (k, _))| (k.clone(), i))
            .collect();
        self.index = Some(ix);
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Rc<str>, &Val)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &Rc<str>> {
        self.entries.iter().map(|(k, _)| k)
    }

    pub fn into_entries(self) -> Vec<(Rc<str>, Val)> {
        self.entries
    }

    /// The entries sorted by key (jq compares and sorts objects this way).
    pub fn sorted(&self) -> Vec<(&Rc<str>, &Val)> {
        let mut v: Vec<_> = self.iter().collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        v
    }
}

impl FromIterator<(Rc<str>, Val)> for Map {
    fn from_iter<I: IntoIterator<Item = (Rc<str>, Val)>>(iter: I) -> Map {
        let mut m = Map::new();
        for (k, v) in iter {
            m.insert(k, v);
        }
        m
    }
}

impl PartialEq for Map {
    fn eq(&self, other: &Map) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .all(|(k, v)| other.get(k).is_some_and(|w| w == v))
    }
}
