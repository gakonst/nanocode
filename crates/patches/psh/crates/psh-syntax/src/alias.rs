//! Alias tables. Bash expands aliases while it reads commands, so the chunked
//! [`crate::Parser`] takes the table the shell has when it reads each chunk.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::span::Span;

/// The aliases defined in a shell: name to replacement text. Cloning is cheap (the
/// table is shared until one copy changes).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Aliases {
    map: Arc<BTreeMap<Vec<u8>, Vec<u8>>>,
}

impl Aliases {
    pub fn new() -> Aliases {
        Aliases::default()
    }

    pub fn get(&self, name: &[u8]) -> Option<&[u8]> {
        self.map.get(name).map(Vec::as_slice)
    }

    pub fn set(&mut self, name: &[u8], value: &[u8]) {
        Arc::make_mut(&mut self.map).insert(name.to_vec(), value.to_vec());
    }

    /// Removes an alias; false if there was none.
    pub fn remove(&mut self, name: &[u8]) -> bool {
        if !self.map.contains_key(name) {
            return false;
        }
        Arc::make_mut(&mut self.map).remove(name).is_some()
    }

    pub fn clear(&mut self) {
        if !self.map.is_empty() {
            self.map = Arc::default();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// The aliases sorted by name (as `alias` lists them).
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        self.map.iter().map(|(k, v)| (k.as_slice(), v.as_slice()))
    }

    /// Whether two tables are the same shared table (cheap; equal tables that were
    /// built separately compare false).
    pub(crate) fn same(&self, other: &Aliases) -> bool {
        Arc::ptr_eq(&self.map, &other.map)
    }
}

/// One expansion of an alias in a chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasUse {
    pub name: Vec<u8>,
    /// The word that was replaced. It lies in the script, or in the text of an outer
    /// alias when an alias text starts with another alias.
    pub word: Span,
    /// The replacement text, in the chunk's source after the script
    /// (`span.start >= Ast::script_len()`).
    pub text: Span,
}
