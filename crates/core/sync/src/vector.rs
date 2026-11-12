use std::{
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    ops::{Index, IndexMut},
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeSeq};

use crate::{
    ordered::SequenceVisitor,
    tree::{At, Items, Owned, Tree},
};

pub struct Vector<A> {
    tree: Tree<A>,
}

impl<A> Clone for Vector<A> {
    fn clone(&self) -> Self {
        Self {
            tree: self.tree.clone(),
        }
    }
}

impl<A> Default for Vector<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A> Vector<A> {
    #[must_use]
    pub const fn new() -> Self {
        Self { tree: Tree::new() }
    }

    pub const fn len(&self) -> usize {
        self.tree.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.tree.len() == 0
    }

    pub fn iter(&self) -> Items<'_, A> {
        self.tree.items(0, self.tree.len())
    }

    pub fn get(&self, index: usize) -> Option<&A> {
        if index >= self.len() {
            return None;
        }
        self.tree.find(At(index))
    }

    pub fn front(&self) -> Option<&A> {
        self.iter().next()
    }

    pub fn back(&self) -> Option<&A> {
        self.iter().next_back()
    }

    pub fn last(&self) -> Option<&A> {
        self.back()
    }

    pub fn clear(&mut self) {
        self.tree = Tree::new();
    }
}

impl<A: Clone> Vector<A> {
    #[must_use]
    pub fn skip(&self, count: usize) -> Self {
        if count >= self.len() {
            return Self::new();
        }
        Self {
            tree: self.tree.slice(count, self.len()),
        }
    }

    #[must_use]
    pub fn take(&self, count: usize) -> Self {
        assert!(count <= self.len(), "Vector::take: count beyond the length");
        Self {
            tree: self.tree.slice(0, count),
        }
    }

    pub fn push_back(&mut self, value: A) {
        self.tree.push(value);
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&A) -> bool) {
        let mut kept = Tree::new();
        for item in self.tree.items(0, self.tree.len()) {
            if keep(item) {
                kept.push(item.clone());
            }
        }
        if kept.len() < self.tree.len() {
            self.tree = kept;
        }
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut A> {
        if index >= self.len() {
            return None;
        }
        self.tree.find_mut(At(index))
    }

    pub fn pop_front(&mut self) -> Option<A> {
        self.tree.pop_first()
    }

    pub fn pop_back(&mut self) -> Option<A> {
        self.tree.pop_last()
    }
}

impl<A: PartialEq> PartialEq for Vector<A> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl<A: Eq> Eq for Vector<A> {}

impl<A: Hash> Hash for Vector<A> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for item in self.iter() {
            item.hash(state);
        }
    }
}

impl<A: fmt::Debug> fmt::Debug for Vector<A> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_list().entries(self.iter()).finish()
    }
}

impl<A: Clone> Extend<A> for Vector<A> {
    fn extend<I: IntoIterator<Item = A>>(&mut self, values: I) {
        for value in values {
            self.push_back(value);
        }
    }
}

impl<A: Clone> FromIterator<A> for Vector<A> {
    fn from_iter<I: IntoIterator<Item = A>>(values: I) -> Self {
        let mut vector = Self::new();
        vector.extend(values);
        vector
    }
}

impl<A> Index<usize> for Vector<A> {
    type Output = A;

    fn index(&self, index: usize) -> &A {
        let Some(item) = self.get(index) else {
            panic!(
                "Vector::index: index out of bounds: {index} >= {}",
                self.len()
            )
        };
        item
    }
}

impl<A: Clone> IndexMut<usize> for Vector<A> {
    fn index_mut(&mut self, index: usize) -> &mut A {
        let len = self.len();
        let Some(item) = self.get_mut(index) else {
            panic!("Vector::index_mut: index out of bounds: {index} >= {len}")
        };
        item
    }
}

impl<'a, A> IntoIterator for &'a Vector<A> {
    type Item = &'a A;
    type IntoIter = Items<'a, A>;

    fn into_iter(self) -> Items<'a, A> {
        self.iter()
    }
}

impl<A: Clone> IntoIterator for Vector<A> {
    type Item = A;
    type IntoIter = Owned<A>;

    fn into_iter(self) -> Owned<A> {
        Owned::new(self.tree)
    }
}

impl<A: Serialize> Serialize for Vector<A> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.len()))?;
        for item in self.iter() {
            sequence.serialize_element(item)?;
        }
        sequence.end()
    }
}

impl<'de, A: Deserialize<'de> + Clone> Deserialize<'de> for Vector<A> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let items = deserializer.deserialize_seq(SequenceVisitor(PhantomData))?;
        Ok(items.into_iter().collect())
    }
}
