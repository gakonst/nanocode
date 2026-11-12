//! awk's associative arrays: string keys to values, iterated in insertion order (awk
//! leaves the order unspecified; this one is stable and what people expect).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::value::{S, Value};

pub type ArrRc = Rc<RefCell<Array>>;

#[derive(Default, Debug)]
pub struct Array {
    index: HashMap<S, usize, crate::hasher::SeededState>,
    entries: Vec<Option<(S, Value)>>,
    live: usize,
}

impl Array {
    pub fn len(&self) -> usize {
        self.live
    }

    pub fn contains(&self, k: &[u8]) -> bool {
        self.index.contains_key(k)
    }

    pub fn get(&self, k: &[u8]) -> Option<&Value> {
        let &i = self.index.get(k)?;
        self.entries[i].as_ref().map(|(_, v)| v)
    }

    /// The element, created (uninitialized) if missing, as a reference does in awk.
    pub fn get_or_create(&mut self, k: S) -> &mut Value {
        let i = match self.index.get(&k) {
            Some(&i) => i,
            None => {
                let i = self.entries.len();
                self.entries.push(Some((k.clone(), Value::Uninit)));
                self.index.insert(k, i);
                self.live += 1;
                i
            }
        };
        &mut self.entries[i].as_mut().unwrap().1
    }

    pub fn set(&mut self, k: S, v: Value) {
        *self.get_or_create(k) = v;
    }

    pub fn delete(&mut self, k: &[u8]) {
        if let Some(i) = self.index.remove(k) {
            self.entries[i] = None;
            self.live -= 1;
            if self.entries.len() > 32 && self.live * 2 < self.entries.len() {
                self.compact();
            }
        }
    }

    pub fn clear(&mut self) {
        self.index.clear();
        self.entries.clear();
        self.live = 0;
    }

    fn compact(&mut self) {
        let entries: Vec<_> = self.entries.drain(..).flatten().collect();
        self.index.clear();
        for (i, (k, _)) in entries.iter().enumerate() {
            self.index.insert(k.clone(), i);
        }
        self.entries = entries.into_iter().map(Some).collect();
    }

    /// The keys now, for a `for (k in a)` loop.
    pub fn keys(&self) -> Vec<S> {
        self.entries
            .iter()
            .flatten()
            .map(|(k, _)| k.clone())
            .collect()
    }
}
