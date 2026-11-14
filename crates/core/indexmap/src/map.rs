mod entry;
mod iter;

pub use entry::{Entry, OccupiedEntry, VacantEntry};
pub use iter::{Drain, IntoIter, IntoKeys, IntoValues, Iter, IterMut, Keys, Values, ValuesMut};

use crate::inner::{Bucket, Index, Inner};
use crate::table::{LINEAR_ENTRIES_MAX, Table, Vacancy};
use core::borrow::Borrow;
use core::fmt;
use core::hash::{BuildHasher, Hash};
use core::mem;
use core::ops::{self, Bound, RangeBounds};
use std::collections::hash_map::RandomState;

pub struct IndexMap<K, V, S = RandomState> {
    inner: Inner<K, V>,
    hash_builder: S,
}

impl<K, V, S> Clone for IndexMap<K, V, S>
where
    K: Clone,
    V: Clone,
    S: Clone,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            hash_builder: self.hash_builder.clone(),
        }
    }

    fn clone_from(&mut self, other: &Self) {
        self.inner.entries.clone_from(&other.inner.entries);
        self.inner.index.clone_from(&other.inner.index);
        self.hash_builder.clone_from(&other.hash_builder);
    }
}

impl<K, V, S> fmt::Debug for IndexMap<K, V, S>
where
    K: fmt::Debug,
    V: fmt::Debug,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_map().entries(self.iter()).finish()
    }
}

impl<K, V> IndexMap<K, V> {
    #[inline]
    pub fn new() -> Self {
        Self::with_capacity_and_hasher(0, RandomState::new())
    }

    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_capacity_and_hasher(capacity, RandomState::new())
    }
}

impl<K, V, S> IndexMap<K, V, S> {
    #[inline]
    pub fn with_capacity_and_hasher(capacity: usize, hash_builder: S) -> Self {
        Self {
            inner: Inner::with_capacity(capacity),
            hash_builder,
        }
    }

    pub const fn with_hasher(hash_builder: S) -> Self {
        Self {
            inner: Inner::new(),
            hash_builder,
        }
    }

    pub const fn capacity(&self) -> usize {
        self.inner.entries.capacity()
    }

    pub const fn hasher(&self) -> &S {
        &self.hash_builder
    }

    pub(crate) fn as_entries(&self) -> &[Bucket<K, V>] {
        &self.inner.entries
    }

    pub(crate) fn into_entries(self) -> Vec<Bucket<K, V>> {
        self.inner.entries
    }

    #[inline]
    pub const fn len(&self) -> usize {
        self.inner.entries.len()
    }

    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.inner.entries.is_empty()
    }

    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter::new(&self.inner.entries)
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        IterMut::new(&mut self.inner.entries)
    }

    pub fn keys(&self) -> Keys<'_, K, V> {
        Keys::new(&self.inner.entries)
    }

    pub fn into_keys(self) -> IntoKeys<K, V> {
        IntoKeys::new(self.inner.entries)
    }

    pub fn values(&self) -> Values<'_, K, V> {
        Values::new(&self.inner.entries)
    }

    pub fn values_mut(&mut self) -> ValuesMut<'_, K, V> {
        ValuesMut::new(&mut self.inner.entries)
    }

    pub fn into_values(self) -> IntoValues<K, V> {
        IntoValues::new(self.inner.entries)
    }

    pub fn clear(&mut self) {
        self.inner.clear();
    }

    #[track_caller]
    pub fn drain<R>(&mut self, range: R) -> Drain<'_, K, V>
    where
        R: RangeBounds<usize>,
    {
        let len = self.inner.entries.len();
        let start = match range.start_bound() {
            Bound::Included(&start) => start,
            Bound::Excluded(&start) => start
                .checked_add(1)
                .expect("IndexMap: drain range start overflows"),
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&end) => end
                .checked_add(1)
                .expect("IndexMap: drain range end overflows"),
            Bound::Excluded(&end) => end,
            Bound::Unbounded => len,
        };
        assert!(
            start <= end,
            "slice index starts at {start} but ends at {end}"
        );
        assert!(
            end <= len,
            "range end index {end} out of range for slice of length {len}"
        );
        if let Index::Hashed(table) = mem::replace(&mut self.inner.index, Index::Linear) {
            let mut hashes = table.into_hashes();
            hashes.drain(start..end);
            let remaining = hashes.len();
            self.inner.index = Index::Hashed(Table::build(hashes, remaining));
        }
        Drain::new(self.inner.entries.drain(start..end))
    }

    pub fn get_index(&self, index: usize) -> Option<(&K, &V)> {
        self.inner.entries.get(index).map(Bucket::refs)
    }

    pub fn get_index_mut(&mut self, index: usize) -> Option<(&K, &mut V)> {
        self.inner.entries.get_mut(index).map(Bucket::refs_mut)
    }

    pub fn first(&self) -> Option<(&K, &V)> {
        self.inner.entries.first().map(Bucket::refs)
    }

    pub fn last(&self) -> Option<(&K, &V)> {
        self.inner.entries.last().map(Bucket::refs)
    }

    pub fn swap_remove_index(&mut self, index: usize) -> Option<(K, V)> {
        if index < self.inner.entries.len() {
            Some(self.inner.swap_remove_index(index))
        } else {
            None
        }
    }

    pub fn shift_remove_index(&mut self, index: usize) -> Option<(K, V)> {
        if index < self.inner.entries.len() {
            Some(self.inner.shift_remove_index(index))
        } else {
            None
        }
    }

    pub fn pop(&mut self) -> Option<(K, V)> {
        let len = self.inner.entries.len();
        if len == 0 {
            None
        } else {
            Some(self.inner.swap_remove_index(len - 1))
        }
    }

    #[track_caller]
    pub fn move_index(&mut self, from: usize, to: usize) {
        self.inner.move_index(from, to);
    }

    pub fn retain<F>(&mut self, mut keep: F)
    where
        F: FnMut(&K, &mut V) -> bool,
    {
        self.inner
            .retain(move |bucket| keep(&bucket.key, &mut bucket.value));
    }

    pub fn sort_unstable_keys(&mut self)
    where
        K: Ord,
    {
        self.inner
            .sort_unstable_by(|left, right| K::cmp(&left.key, &right.key));
    }
}

impl<K, V, S> IndexMap<K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    pub fn reserve(&mut self, additional: usize) {
        self.inner.reserve(additional);
    }

    #[inline]
    fn find<Q>(&self, key: &Q) -> Option<usize>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        let entries = &self.inner.entries;
        match &self.inner.index {
            Index::Linear => entries.iter().position(|bucket| bucket.key.borrow() == key),
            Index::Hashed(table) => {
                let hash = self.hash_builder.hash_one(key);
                table
                    .find(hash, |index| entries[index].key.borrow() == key)
                    .ok()
            }
        }
    }

    fn index_all(&mut self) {
        let mut hashes = Vec::with_capacity(self.inner.entries.capacity());
        for bucket in &self.inner.entries {
            hashes.push(self.hash_builder.hash_one(&bucket.key));
        }
        self.inner.index_hashes(hashes);
    }

    #[inline]
    fn find_or_vacancy(&mut self, key: &K) -> Result<usize, Vacancy> {
        if let Index::Linear = self.inner.index {
            let entries = &self.inner.entries;
            if let Some(index) = entries.iter().position(|bucket| bucket.key == *key) {
                return Ok(index);
            }
            if entries.len() < LINEAR_ENTRIES_MAX {
                return Err(Vacancy::Linear);
            }
            self.index_all();
        }
        let hash = self.hash_builder.hash_one(key);
        let entries = &self.inner.entries;
        match &self.inner.index {
            Index::Hashed(table) => table
                .find(hash, |index| entries[index].key == *key)
                .map_err(|position| Vacancy::Hashed { hash, position }),
            Index::Linear => panic!("IndexMap: index_all left a linear index"),
        }
    }

    #[inline]
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.insert_full(key, value).1
    }

    #[inline]
    pub fn insert_full(&mut self, key: K, value: V) -> (usize, Option<V>) {
        match self.find_or_vacancy(&key) {
            Ok(index) => {
                let old = mem::replace(&mut self.inner.entries[index].value, value);
                (index, Some(old))
            }
            Err(vacancy) => (self.inner.push(key, value, vacancy), None),
        }
    }

    #[track_caller]
    pub fn shift_insert(&mut self, index: usize, key: K, value: V) -> Option<V> {
        let len = self.len();
        match self.entry(key) {
            Entry::Occupied(mut entry) => {
                assert!(
                    index < len,
                    "index out of bounds: the len is {len} but the index is {index}"
                );
                let old = mem::replace(entry.get_mut(), value);
                entry.move_index(index);
                Some(old)
            }
            Entry::Vacant(entry) => {
                assert!(
                    index <= len,
                    "index out of bounds: the len is {len} but the index is {index}. Expected index <= len"
                );
                entry.shift_insert(index, value);
                None
            }
        }
    }

    #[inline]
    pub fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        match self.find_or_vacancy(&key) {
            Ok(index) => Entry::Occupied(OccupiedEntry::new(&mut self.inner, index)),
            Err(vacancy) => Entry::Vacant(VacantEntry::new(&mut self.inner, key, vacancy)),
        }
    }

    #[inline]
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.find(key).is_some()
    }

    #[inline]
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.find(key).map(|index| &self.inner.entries[index].value)
    }

    #[inline]
    pub fn get_key_value<Q>(&self, key: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.find(key).map(|index| self.inner.entries[index].refs())
    }

    pub fn get_full<Q>(&self, key: &Q) -> Option<(usize, &K, &V)>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.find(key).map(|index| {
            let (key, value) = self.inner.entries[index].refs();
            (index, key, value)
        })
    }

    #[inline]
    pub fn get_index_of<Q>(&self, key: &Q) -> Option<usize>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.find(key)
    }

    #[inline]
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.find(key)
            .map(|index| &mut self.inner.entries[index].value)
    }

    pub fn swap_remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.swap_remove_full(key).map(|(_, _, value)| value)
    }

    pub fn swap_remove_entry<Q>(&mut self, key: &Q) -> Option<(K, V)>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.swap_remove_full(key)
            .map(|(_, key, value)| (key, value))
    }

    pub fn swap_remove_full<Q>(&mut self, key: &Q) -> Option<(usize, K, V)>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        let index = self.find(key)?;
        let (key, value) = self.inner.swap_remove_index(index);
        Some((index, key, value))
    }

    pub fn shift_remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.shift_remove_full(key).map(|(_, _, value)| value)
    }

    pub fn shift_remove_entry<Q>(&mut self, key: &Q) -> Option<(K, V)>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.shift_remove_full(key)
            .map(|(_, key, value)| (key, value))
    }

    pub fn shift_remove_full<Q>(&mut self, key: &Q) -> Option<(usize, K, V)>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        let index = self.find(key)?;
        let (key, value) = self.inner.shift_remove_index(index);
        Some((index, key, value))
    }
}

impl<'a, K, V, S> IntoIterator for &'a IndexMap<K, V, S> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, K, V, S> IntoIterator for &'a mut IndexMap<K, V, S> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = IterMut<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<K, V, S> IntoIterator for IndexMap<K, V, S> {
    type Item = (K, V);
    type IntoIter = IntoIter<K, V>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter::new(self.inner.entries)
    }
}

impl<K, V, Q, S> ops::Index<&Q> for IndexMap<K, V, S>
where
    K: Hash + Eq + Borrow<Q>,
    Q: ?Sized + Hash + Eq,
    S: BuildHasher,
{
    type Output = V;

    fn index(&self, key: &Q) -> &V {
        self.get(key).expect("no entry found for key")
    }
}

impl<K, V, Q, S> ops::IndexMut<&Q> for IndexMap<K, V, S>
where
    K: Hash + Eq + Borrow<Q>,
    Q: ?Sized + Hash + Eq,
    S: BuildHasher,
{
    fn index_mut(&mut self, key: &Q) -> &mut V {
        self.get_mut(key).expect("no entry found for key")
    }
}

impl<K, V, S> ops::Index<usize> for IndexMap<K, V, S> {
    type Output = V;

    fn index(&self, index: usize) -> &V {
        if let Some((_, value)) = self.get_index(index) {
            value
        } else {
            panic!(
                "index out of bounds: the len is {len} but the index is {index}",
                len = self.len()
            );
        }
    }
}

impl<K, V, S> ops::IndexMut<usize> for IndexMap<K, V, S> {
    fn index_mut(&mut self, index: usize) -> &mut V {
        let len = self.len();
        if let Some((_, value)) = self.get_index_mut(index) {
            value
        } else {
            panic!("index out of bounds: the len is {len} but the index is {index}");
        }
    }
}

impl<K, V, S> FromIterator<(K, V)> for IndexMap<K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher + Default,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iterable: I) -> Self {
        let iter = iterable.into_iter();
        let (lower, _) = iter.size_hint();
        let mut map = Self::with_capacity_and_hasher(lower, S::default());
        map.extend(iter);
        map
    }
}

impl<K, V, S> Extend<(K, V)> for IndexMap<K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iterable: I) {
        let iter = iterable.into_iter();
        let (lower, _) = iter.size_hint();
        let reserve = if self.is_empty() {
            lower
        } else {
            lower.div_ceil(2)
        };
        self.reserve(reserve);
        iter.for_each(move |(key, value)| {
            self.insert(key, value);
        });
    }
}

impl<'a, K, V, S> Extend<(&'a K, &'a V)> for IndexMap<K, V, S>
where
    K: Hash + Eq + Copy,
    V: Copy,
    S: BuildHasher,
{
    fn extend<I: IntoIterator<Item = (&'a K, &'a V)>>(&mut self, iterable: I) {
        self.extend(iterable.into_iter().map(|(&key, &value)| (key, value)));
    }
}

impl<K, V, S> Default for IndexMap<K, V, S>
where
    S: Default,
{
    fn default() -> Self {
        Self::with_capacity_and_hasher(0, S::default())
    }
}

impl<K, V1, S1, V2, S2> PartialEq<IndexMap<K, V2, S2>> for IndexMap<K, V1, S1>
where
    K: Hash + Eq,
    V1: PartialEq<V2>,
    S1: BuildHasher,
    S2: BuildHasher,
{
    fn eq(&self, other: &IndexMap<K, V2, S2>) -> bool {
        self.len() == other.len()
            && self.iter().all(|(key, value)| {
                other
                    .get(key)
                    .is_some_and(|other_value| *value == *other_value)
            })
    }
}

impl<K, V, S> Eq for IndexMap<K, V, S>
where
    K: Eq + Hash,
    V: Eq,
    S: BuildHasher,
{
}
