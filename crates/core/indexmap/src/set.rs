use crate::inner::Bucket;
use crate::map::IndexMap;
use core::borrow::Borrow;
use core::fmt;
use core::hash::{BuildHasher, Hash};
use core::iter::FusedIterator;
use core::ops;
use core::slice;
use std::collections::hash_map::RandomState;
use std::vec;

pub struct IndexSet<T, S = RandomState> {
    map: IndexMap<T, (), S>,
}

impl<T, S> Clone for IndexSet<T, S>
where
    T: Clone,
    S: Clone,
{
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
        }
    }

    fn clone_from(&mut self, other: &Self) {
        self.map.clone_from(&other.map);
    }
}

impl<T: fmt::Debug, S> fmt::Debug for IndexSet<T, S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_set().entries(self.iter()).finish()
    }
}

impl<T> IndexSet<T> {
    pub fn new() -> Self {
        Self {
            map: IndexMap::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            map: IndexMap::with_capacity(capacity),
        }
    }
}

impl<T, S> IndexSet<T, S> {
    pub fn with_capacity_and_hasher(capacity: usize, hash_builder: S) -> Self {
        Self {
            map: IndexMap::with_capacity_and_hasher(capacity, hash_builder),
        }
    }

    pub const fn with_hasher(hash_builder: S) -> Self {
        Self {
            map: IndexMap::with_hasher(hash_builder),
        }
    }

    pub const fn capacity(&self) -> usize {
        self.map.capacity()
    }

    pub const fn hasher(&self) -> &S {
        self.map.hasher()
    }

    pub const fn len(&self) -> usize {
        self.map.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            iter: self.map.as_entries().iter(),
        }
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    pub fn get_index(&self, index: usize) -> Option<&T> {
        self.map.get_index(index).map(|(key, _)| key)
    }

    pub fn first(&self) -> Option<&T> {
        self.map.first().map(|(key, _)| key)
    }

    pub fn last(&self) -> Option<&T> {
        self.map.last().map(|(key, _)| key)
    }

    pub fn pop(&mut self) -> Option<T> {
        self.map.pop().map(|(key, ())| key)
    }

    pub fn swap_remove_index(&mut self, index: usize) -> Option<T> {
        self.map.swap_remove_index(index).map(|(key, ())| key)
    }

    pub fn shift_remove_index(&mut self, index: usize) -> Option<T> {
        self.map.shift_remove_index(index).map(|(key, ())| key)
    }

    #[track_caller]
    pub fn move_index(&mut self, from: usize, to: usize) {
        self.map.move_index(from, to);
    }

    pub fn retain<F>(&mut self, mut keep: F)
    where
        F: FnMut(&T) -> bool,
    {
        self.map.retain(move |key, ()| keep(key));
    }

    pub fn sort_unstable(&mut self)
    where
        T: Ord,
    {
        self.map.sort_unstable_keys();
    }
}

impl<T, S> IndexSet<T, S>
where
    T: Hash + Eq,
    S: BuildHasher,
{
    pub fn reserve(&mut self, additional: usize) {
        self.map.reserve(additional);
    }

    pub fn insert(&mut self, value: T) -> bool {
        self.insert_full(value).1
    }

    pub fn insert_full(&mut self, value: T) -> (usize, bool) {
        let (index, existing) = self.map.insert_full(value, ());
        (index, existing.is_none())
    }

    pub fn contains<Q>(&self, value: &Q) -> bool
    where
        T: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.map.contains_key(value)
    }

    pub fn get<Q>(&self, value: &Q) -> Option<&T>
    where
        T: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.map.get_key_value(value).map(|(key, _)| key)
    }

    pub fn get_full<Q>(&self, value: &Q) -> Option<(usize, &T)>
    where
        T: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.map.get_full(value).map(|(index, key, _)| (index, key))
    }

    pub fn get_index_of<Q>(&self, value: &Q) -> Option<usize>
    where
        T: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.map.get_index_of(value)
    }

    pub fn swap_remove<Q>(&mut self, value: &Q) -> bool
    where
        T: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.map.swap_remove(value).is_some()
    }

    pub fn shift_remove<Q>(&mut self, value: &Q) -> bool
    where
        T: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.map.shift_remove(value).is_some()
    }

    pub fn swap_take<Q>(&mut self, value: &Q) -> Option<T>
    where
        T: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.map.swap_remove_entry(value).map(|(key, ())| key)
    }

    pub fn shift_take<Q>(&mut self, value: &Q) -> Option<T>
    where
        T: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.map.shift_remove_entry(value).map(|(key, ())| key)
    }

    pub fn swap_remove_full<Q>(&mut self, value: &Q) -> Option<(usize, T)>
    where
        T: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.map
            .swap_remove_full(value)
            .map(|(index, key, ())| (index, key))
    }

    pub fn shift_remove_full<Q>(&mut self, value: &Q) -> Option<(usize, T)>
    where
        T: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.map
            .shift_remove_full(value)
            .map(|(index, key, ())| (index, key))
    }

    pub fn is_subset<S2>(&self, other: &IndexSet<T, S2>) -> bool
    where
        S2: BuildHasher,
    {
        self.len() <= other.len() && self.iter().all(|value| other.contains(value))
    }
}

impl<T, S> ops::Index<usize> for IndexSet<T, S> {
    type Output = T;

    fn index(&self, index: usize) -> &T {
        if let Some(value) = self.get_index(index) {
            value
        } else {
            panic!(
                "index out of bounds: the len is {len} but the index is {index}",
                len = self.len()
            );
        }
    }
}

impl<T, S> FromIterator<T> for IndexSet<T, S>
where
    T: Hash + Eq,
    S: BuildHasher + Default,
{
    fn from_iter<I: IntoIterator<Item = T>>(iterable: I) -> Self {
        Self {
            map: iterable.into_iter().map(|value| (value, ())).collect(),
        }
    }
}

impl<T, S> Extend<T> for IndexSet<T, S>
where
    T: Hash + Eq,
    S: BuildHasher,
{
    fn extend<I: IntoIterator<Item = T>>(&mut self, iterable: I) {
        self.map
            .extend(iterable.into_iter().map(|value| (value, ())));
    }
}

impl<'a, T, S> Extend<&'a T> for IndexSet<T, S>
where
    T: Hash + Eq + Copy + 'a,
    S: BuildHasher,
{
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, iterable: I) {
        self.extend(iterable.into_iter().copied());
    }
}

impl<T, S> Default for IndexSet<T, S>
where
    S: Default,
{
    fn default() -> Self {
        Self {
            map: IndexMap::default(),
        }
    }
}

impl<T, S1, S2> PartialEq<IndexSet<T, S2>> for IndexSet<T, S1>
where
    T: Hash + Eq,
    S1: BuildHasher,
    S2: BuildHasher,
{
    fn eq(&self, other: &IndexSet<T, S2>) -> bool {
        self.len() == other.len() && self.is_subset(other)
    }
}

impl<T, S> Eq for IndexSet<T, S>
where
    T: Eq + Hash,
    S: BuildHasher,
{
}

impl<'a, T, S> IntoIterator for &'a IndexSet<T, S> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T, S> IntoIterator for IndexSet<T, S> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            iter: self.map.into_entries().into_iter(),
        }
    }
}

pub struct Iter<'a, T> {
    iter: slice::Iter<'a, Bucket<T, ()>>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(Bucket::key_ref)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<T> DoubleEndedIterator for Iter<'_, T> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        self.iter.next_back().map(Bucket::key_ref)
    }
}

impl<T> ExactSizeIterator for Iter<'_, T> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<T> FusedIterator for Iter<'_, T> {}

impl<T> Clone for Iter<'_, T> {
    fn clone(&self) -> Self {
        Self {
            iter: self.iter.clone(),
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Iter<'_, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_list().entries(self.clone()).finish()
    }
}

pub struct IntoIter<T> {
    iter: vec::IntoIter<Bucket<T, ()>>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(Bucket::into_key)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<T> DoubleEndedIterator for IntoIter<T> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        self.iter.next_back().map(Bucket::into_key)
    }
}

impl<T> ExactSizeIterator for IntoIter<T> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<T> FusedIterator for IntoIter<T> {}

impl<T: fmt::Debug> fmt::Debug for IntoIter<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let iter = self.iter.as_slice().iter().map(Bucket::key_ref);
        formatter.debug_list().entries(iter).finish()
    }
}
