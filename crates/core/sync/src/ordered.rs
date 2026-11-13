use std::{
    borrow::Borrow,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    ops::{Index, RangeBounds},
};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{MapAccess, SeqAccess, Visitor},
    ser::{SerializeMap, SerializeSeq},
};

use crate::tree::{ByKey, ByOrder, Items, Owned, Tree};

const fn entry_key<K, V>(entry: &(K, V)) -> &K {
    &entry.0
}

const fn itself<A>(item: &A) -> &A {
    item
}

pub struct OrdMap<K, V> {
    tree: Tree<(K, V)>,
}

impl<K, V> Clone for OrdMap<K, V> {
    fn clone(&self) -> Self {
        Self {
            tree: self.tree.clone(),
        }
    }
}

impl<K, V> Default for OrdMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> OrdMap<K, V> {
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

    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            items: self.tree.items(0, self.tree.len()),
        }
    }

    pub fn keys(&self) -> Keys<'_, K, V> {
        Keys { iter: self.iter() }
    }

    pub fn values(&self) -> Values<'_, K, V> {
        Values { iter: self.iter() }
    }

    pub fn get_min(&self) -> Option<&(K, V)> {
        self.tree.first()
    }

    pub fn get_max(&self) -> Option<&(K, V)> {
        self.tree.last()
    }
}

impl<K: Ord, V> OrdMap<K, V> {
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.get_key_value(key).map(|(_, value)| value)
    }

    pub fn get_key_value<Q>(&self, key: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.tree
            .find(ByKey {
                key,
                project: entry_key,
            })
            .map(|(key, value)| (key, value))
    }

    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.get_key_value(key).is_some()
    }

    pub fn range<R, Q>(&self, range: R) -> Iter<'_, K, V>
    where
        R: RangeBounds<Q>,
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        Iter {
            items: self.tree.range(&range, entry_key),
        }
    }
}

impl<K: Ord + Clone, V: Clone> OrdMap<K, V> {
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.tree
            .insert((key, value), ByOrder(entry_key))
            .map(|(_, old)| old)
    }

    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.remove_with_key(key).map(|(_, value)| value)
    }

    pub fn remove_with_key<Q>(&mut self, key: &Q) -> Option<(K, V)>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let path = self.tree.path(ByKey {
            key,
            project: entry_key,
        })?;
        Some(self.tree.remove(path))
    }

    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let path = self.tree.path(ByKey {
            key,
            project: entry_key,
        })?;
        self.tree.find_mut(path).map(|entry| &mut entry.1)
    }

    pub const fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        Entry { map: self, key }
    }

    #[must_use]
    pub fn split<Q>(&self, split: &Q) -> (Self, Self)
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let below = self.tree.rank(split, false, &entry_key);
        let above = self.tree.rank(split, true, &entry_key);
        let left = Self {
            tree: self.tree.slice(0, below),
        };
        let right = Self {
            tree: self.tree.slice(above, self.len()),
        };
        (left, right)
    }
}

pub struct Entry<'a, K, V> {
    map: &'a mut OrdMap<K, V>,
    key: K,
}

impl<'a, K: Ord + Clone, V: Clone> Entry<'a, K, V> {
    pub fn or_insert_with(self, value: impl FnOnce() -> V) -> &'a mut V {
        let locate = || ByKey {
            key: &self.key,
            project: entry_key,
        };
        let path = match self.map.tree.path(locate()) {
            Some(path) => path,
            None => {
                self.map.insert(self.key.clone(), value());
                let Some(path) = self.map.tree.path(locate()) else {
                    panic!("an entry holds its key after insertion")
                };
                path
            }
        };
        let Some(entry) = self.map.tree.find_mut(path) else {
            panic!("a found path leads to an entry")
        };
        &mut entry.1
    }

    pub fn or_insert(self, value: V) -> &'a mut V {
        self.or_insert_with(|| value)
    }

    pub fn or_default(self) -> &'a mut V
    where
        V: Default,
    {
        self.or_insert_with(V::default)
    }
}

impl<K: Ord, V: PartialEq> PartialEq for OrdMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl<K: Ord, V: Eq> Eq for OrdMap<K, V> {}

impl<K: Ord + Hash, V: Hash> Hash for OrdMap<K, V> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for entry in self.iter() {
            entry.hash(state);
        }
    }
}

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for OrdMap<K, V> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_map().entries(self.iter()).finish()
    }
}

impl<K: Ord + Clone, V: Clone> Extend<(K, V)> for OrdMap<K, V> {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, entries: I) {
        for (key, value) in entries {
            self.insert(key, value);
        }
    }
}

impl<K: Ord + Clone, V: Clone> FromIterator<(K, V)> for OrdMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(entries: I) -> Self {
        let mut map = Self::new();
        map.extend(entries);
        map
    }
}

impl<K: Ord, V, Q> Index<&Q> for OrdMap<K, V>
where
    K: Borrow<Q>,
    Q: Ord + ?Sized,
{
    type Output = V;

    fn index(&self, key: &Q) -> &V {
        let Some(value) = self.get(key) else {
            panic!("OrdMap::index: invalid key")
        };
        value
    }
}

impl<'a, K, V> IntoIterator for &'a OrdMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Iter<'a, K, V> {
        self.iter()
    }
}

impl<K: Clone, V: Clone> IntoIterator for OrdMap<K, V> {
    type Item = (K, V);
    type IntoIter = Owned<(K, V)>;

    fn into_iter(self) -> Owned<(K, V)> {
        Owned::new(self.tree)
    }
}

impl<K: Serialize, V: Serialize> Serialize for OrdMap<K, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.len()))?;
        for (key, value) in self.iter() {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

struct MapVisitor<K, V>(PhantomData<(K, V)>);

impl<'de, K, V> Visitor<'de> for MapVisitor<K, V>
where
    K: Deserialize<'de> + Ord + Clone,
    V: Deserialize<'de> + Clone,
{
    type Value = OrdMap<K, V>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a sequence")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut access: M) -> Result<OrdMap<K, V>, M::Error> {
        let mut entries = Vec::with_capacity(access.size_hint().unwrap_or(0));
        while let Some(entry) = access.next_entry()? {
            entries.push(entry);
        }
        Ok(entries.into_iter().collect())
    }
}

impl<'de, K, V> Deserialize<'de> for OrdMap<K, V>
where
    K: Deserialize<'de> + Ord + Clone,
    V: Deserialize<'de> + Clone,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(MapVisitor(PhantomData))
    }
}

pub struct Iter<'a, K, V> {
    items: Items<'a, (K, V)>,
}

impl<K, V> Clone for Iter<'_, K, V> {
    fn clone(&self) -> Self {
        Self {
            items: self.items.clone(),
        }
    }
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<(&'a K, &'a V)> {
        self.items.next().map(|entry| (&entry.0, &entry.1))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.items.size_hint()
    }
}

impl<'a, K, V> DoubleEndedIterator for Iter<'a, K, V> {
    fn next_back(&mut self) -> Option<(&'a K, &'a V)> {
        self.items.next_back().map(|entry| (&entry.0, &entry.1))
    }
}

impl<K, V> ExactSizeIterator for Iter<'_, K, V> {}

impl<K, V> std::iter::FusedIterator for Iter<'_, K, V> {}

pub struct Keys<'a, K, V> {
    iter: Iter<'a, K, V>,
}

impl<'a, K, V> Iterator for Keys<'a, K, V> {
    type Item = &'a K;

    fn next(&mut self) -> Option<&'a K> {
        self.iter.next().map(|(key, _)| key)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a, K, V> DoubleEndedIterator for Keys<'a, K, V> {
    fn next_back(&mut self) -> Option<&'a K> {
        self.iter.next_back().map(|(key, _)| key)
    }
}

impl<K, V> ExactSizeIterator for Keys<'_, K, V> {}

pub struct Values<'a, K, V> {
    iter: Iter<'a, K, V>,
}

impl<'a, K, V> Iterator for Values<'a, K, V> {
    type Item = &'a V;

    fn next(&mut self) -> Option<&'a V> {
        self.iter.next().map(|(_, value)| value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a, K, V> DoubleEndedIterator for Values<'a, K, V> {
    fn next_back(&mut self) -> Option<&'a V> {
        self.iter.next_back().map(|(_, value)| value)
    }
}

impl<K, V> ExactSizeIterator for Values<'_, K, V> {}

pub struct OrdSet<A> {
    tree: Tree<A>,
}

impl<A> Clone for OrdSet<A> {
    fn clone(&self) -> Self {
        Self {
            tree: self.tree.clone(),
        }
    }
}

impl<A> Default for OrdSet<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A> OrdSet<A> {
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

    pub fn get_min(&self) -> Option<&A> {
        self.tree.first()
    }

    pub fn get_max(&self) -> Option<&A> {
        self.tree.last()
    }
}

impl<A: Ord> OrdSet<A> {
    pub fn get<Q>(&self, value: &Q) -> Option<&A>
    where
        A: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.tree.find(ByKey {
            key: value,
            project: itself,
        })
    }

    pub fn contains<Q>(&self, value: &Q) -> bool
    where
        A: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.get(value).is_some()
    }

    pub fn range<R, Q>(&self, range: R) -> Items<'_, A>
    where
        R: RangeBounds<Q>,
        A: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.tree.range(&range, itself)
    }
}

impl<A: Ord + Clone> OrdSet<A> {
    pub fn insert(&mut self, value: A) -> Option<A> {
        self.tree.insert(value, ByOrder(itself))
    }

    pub fn remove<Q>(&mut self, value: &Q) -> Option<A>
    where
        A: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let path = self.tree.path(ByKey {
            key: value,
            project: itself,
        })?;
        Some(self.tree.remove(path))
    }

    pub fn remove_min(&mut self) -> Option<A> {
        self.tree.pop_first()
    }

    pub fn remove_max(&mut self) -> Option<A> {
        self.tree.pop_last()
    }
}

impl<A: Ord> PartialEq for OrdSet<A> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl<A: Ord> Eq for OrdSet<A> {}

impl<A: Ord + Hash> Hash for OrdSet<A> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for item in self.iter() {
            item.hash(state);
        }
    }
}

impl<A: fmt::Debug> fmt::Debug for OrdSet<A> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_set().entries(self.iter()).finish()
    }
}

impl<A: Ord + Clone> Extend<A> for OrdSet<A> {
    fn extend<I: IntoIterator<Item = A>>(&mut self, values: I) {
        for value in values {
            self.insert(value);
        }
    }
}

impl<A: Ord + Clone> FromIterator<A> for OrdSet<A> {
    fn from_iter<I: IntoIterator<Item = A>>(values: I) -> Self {
        let mut set = Self::new();
        set.extend(values);
        set
    }
}

impl<'a, A> IntoIterator for &'a OrdSet<A> {
    type Item = &'a A;
    type IntoIter = Items<'a, A>;

    fn into_iter(self) -> Items<'a, A> {
        self.iter()
    }
}

impl<A: Clone> IntoIterator for OrdSet<A> {
    type Item = A;
    type IntoIter = Owned<A>;

    fn into_iter(self) -> Owned<A> {
        Owned::new(self.tree)
    }
}

impl<A: Serialize> Serialize for OrdSet<A> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.len()))?;
        for item in self.iter() {
            sequence.serialize_element(item)?;
        }
        sequence.end()
    }
}

pub(crate) struct SequenceVisitor<A>(pub(crate) PhantomData<A>);

impl<'de, A: Deserialize<'de>> Visitor<'de> for SequenceVisitor<A> {
    type Value = Vec<A>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a sequence")
    }

    fn visit_seq<S: SeqAccess<'de>>(self, mut access: S) -> Result<Vec<A>, S::Error> {
        let mut items = Vec::with_capacity(access.size_hint().unwrap_or(0));
        while let Some(item) = access.next_element()? {
            items.push(item);
        }
        Ok(items)
    }
}

impl<'de, A: Deserialize<'de> + Ord + Clone> Deserialize<'de> for OrdSet<A> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let items = deserializer.deserialize_seq(SequenceVisitor(PhantomData))?;
        Ok(items.into_iter().collect())
    }
}
