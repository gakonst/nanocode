use std::{
    borrow::Borrow,
    hash::{BuildHasher, Hash},
    sync::Arc,
};

use foldhash::quality::RandomState;

const BITS: u32 = 5;
const MASK: u64 = (1 << BITS) - 1;

#[derive(Clone)]
struct Pair<K, V> {
    hash: u64,
    key: K,
    value: V,
}

enum Slot<K, V> {
    Pair(Arc<Pair<K, V>>),
    Bucket(Arc<Vec<Arc<Pair<K, V>>>>),
    Node(Arc<Node<K, V>>),
}

impl<K, V> Clone for Slot<K, V> {
    fn clone(&self) -> Self {
        match self {
            Self::Pair(pair) => Self::Pair(Arc::clone(pair)),
            Self::Bucket(bucket) => Self::Bucket(Arc::clone(bucket)),
            Self::Node(node) => Self::Node(Arc::clone(node)),
        }
    }
}

struct Node<K, V> {
    occupied: u32,
    slots: Vec<Slot<K, V>>,
}

impl<K, V> Clone for Node<K, V> {
    fn clone(&self) -> Self {
        Self {
            occupied: self.occupied,
            slots: self.slots.clone(),
        }
    }
}

const fn bit(hash: u64, shift: u32) -> u32 {
    1 << ((hash >> shift) & MASK)
}

impl<K, V> Node<K, V> {
    const fn empty() -> Self {
        Self {
            occupied: 0,
            slots: Vec::new(),
        }
    }

    const fn position(&self, bit: u32) -> usize {
        (self.occupied & (bit - 1)).count_ones() as usize
    }

    fn joined(held: Slot<K, V>, held_hash: u64, pair: Arc<Pair<K, V>>, shift: u32) -> Self {
        let (held_bit, pair_bit) = (bit(held_hash, shift), bit(pair.hash, shift));
        if held_bit == pair_bit {
            let below = Self::joined(held, held_hash, pair, shift + BITS);
            return Self {
                occupied: held_bit,
                slots: vec![Slot::Node(Arc::new(below))],
            };
        }
        let slots = if held_bit < pair_bit {
            vec![held, Slot::Pair(pair)]
        } else {
            vec![Slot::Pair(pair), held]
        };
        Self {
            occupied: held_bit | pair_bit,
            slots,
        }
    }
}

impl<K: Eq, V> Node<K, V> {
    fn find<Q>(&self, hash: u64, key: &Q) -> Option<&Arc<Pair<K, V>>>
    where
        K: Borrow<Q>,
        Q: Eq + ?Sized,
    {
        let mut node = self;
        let mut shift = 0;
        loop {
            let bit = bit(hash, shift);
            if node.occupied & bit == 0 {
                return None;
            }
            match &node.slots[node.position(bit)] {
                Slot::Pair(pair) => {
                    return (pair.hash == hash && pair.key.borrow() == key).then_some(pair);
                }
                Slot::Bucket(bucket) => {
                    return bucket
                        .iter()
                        .find(|pair| pair.hash == hash && pair.key.borrow() == key);
                }
                Slot::Node(below) => {
                    node = below;
                    shift += BITS;
                }
            }
        }
    }

    fn insert(&mut self, pair: Arc<Pair<K, V>>, shift: u32) -> Option<Arc<Pair<K, V>>> {
        let bit = bit(pair.hash, shift);
        let position = self.position(bit);
        if self.occupied & bit == 0 {
            self.slots.insert(position, Slot::Pair(pair));
            self.occupied |= bit;
            return None;
        }
        let slot = &mut self.slots[position];
        match slot {
            Slot::Node(below) => Arc::make_mut(below).insert(pair, shift + BITS),
            Slot::Pair(held) => {
                if held.hash != pair.hash {
                    let (kept, hash) = (Slot::Pair(Arc::clone(held)), held.hash);
                    *slot = Slot::Node(Arc::new(Self::joined(kept, hash, pair, shift + BITS)));
                    None
                } else if held.key == pair.key {
                    Some(std::mem::replace(held, pair))
                } else {
                    *slot = Slot::Bucket(Arc::new(vec![Arc::clone(held), pair]));
                    None
                }
            }
            Slot::Bucket(bucket) => {
                let hash = bucket[0].hash;
                if hash != pair.hash {
                    let kept = Slot::Bucket(Arc::clone(bucket));
                    *slot = Slot::Node(Arc::new(Self::joined(kept, hash, pair, shift + BITS)));
                    return None;
                }
                let bucket = Arc::make_mut(bucket);
                match bucket.iter_mut().find(|held| held.key == pair.key) {
                    Some(held) => Some(std::mem::replace(held, pair)),
                    None => {
                        bucket.push(pair);
                        None
                    }
                }
            }
        }
    }

    fn remove<Q>(&mut self, hash: u64, key: &Q, shift: u32) -> Option<Arc<Pair<K, V>>>
    where
        K: Borrow<Q>,
        Q: Eq + ?Sized,
    {
        let bit = bit(hash, shift);
        if self.occupied & bit == 0 {
            return None;
        }
        let position = self.position(bit);
        let slot = &mut self.slots[position];
        let (removed, left) = match slot {
            Slot::Pair(pair) => {
                if pair.hash != hash || pair.key.borrow() != key {
                    return None;
                }
                (Arc::clone(pair), None)
            }
            Slot::Bucket(bucket) => {
                let index = bucket
                    .iter()
                    .position(|pair| pair.hash == hash && pair.key.borrow() == key)?;
                let bucket = Arc::make_mut(bucket);
                let removed = bucket.remove(index);
                let left = match bucket.len() {
                    1 => Slot::Pair(Arc::clone(&bucket[0])),
                    _ => slot.clone(),
                };
                (removed, Some(left))
            }
            Slot::Node(below) => {
                let below = Arc::make_mut(below);
                let removed = below.remove(hash, key, shift + BITS)?;
                let left = match below.slots.as_slice() {
                    [] => None,
                    [only @ (Slot::Pair(_) | Slot::Bucket(_))] => Some(only.clone()),
                    _ => Some(slot.clone()),
                };
                (removed, left)
            }
        };
        match left {
            Some(left) => self.slots[position] = left,
            None => {
                self.slots.remove(position);
                self.occupied &= !bit;
            }
        }
        Some(removed)
    }
}

pub struct Map<K, V> {
    root: Arc<Node<K, V>>,
    len: usize,
    hasher: RandomState,
}

impl<K, V> Clone for Map<K, V> {
    fn clone(&self) -> Self {
        Self {
            root: Arc::clone(&self.root),
            len: self.len,
            hasher: self.hasher.clone(),
        }
    }
}

impl<K, V> Default for Map<K, V> {
    fn default() -> Self {
        Self {
            root: Arc::new(Node::empty()),
            len: 0,
            hasher: RandomState::default(),
        }
    }
}

impl<K, V> Map<K, V> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.root = Arc::new(Node::empty());
        self.len = 0;
    }

    #[must_use]
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            nodes: vec![self.root.slots.iter()],
            bucket: [].iter(),
            left: self.len,
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(key, _)| key)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, value)| value)
    }
}

impl<K: Hash + Eq, V> Map<K, V> {
    fn hash<Q: Hash + ?Sized>(&self, key: &Q) -> u64 {
        self.hasher.hash_one(key)
    }

    #[must_use]
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.root.find(self.hash(key), key).map(|pair| &pair.value)
    }

    #[must_use]
    pub fn get_key_value<Q>(&self, key: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.root
            .find(self.hash(key), key)
            .map(|pair| (&pair.key, &pair.value))
    }

    #[must_use]
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.root.find(self.hash(key), key).is_some()
    }

    fn place(&mut self, key: K, value: V) -> Option<Arc<Pair<K, V>>> {
        let hash = self.hash(&key);
        let pair = Arc::new(Pair { hash, key, value });
        let replaced = Arc::make_mut(&mut self.root).insert(pair, 0);
        if replaced.is_none() {
            self.len += 1;
        }
        replaced
    }

    fn take<Q>(&mut self, key: &Q) -> Option<Arc<Pair<K, V>>>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = self.hash(key);
        self.root.find(hash, key)?;
        let removed = Arc::make_mut(&mut self.root).remove(hash, key, 0)?;
        self.len -= 1;
        Some(removed)
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool) {
        let dropped = self
            .root
            .pairs()
            .filter(|pair| !keep(&pair.key, &pair.value))
            .cloned()
            .collect::<Vec<_>>();
        if dropped.is_empty() {
            return;
        }
        let root = Arc::make_mut(&mut self.root);
        for pair in dropped {
            root.remove(pair.hash, &pair.key, 0);
            self.len -= 1;
        }
    }
}

impl<K: Hash + Eq, V: Clone> Map<K, V> {
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.place(key, value).map(|pair| taken(pair))
    }

    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.take(key).map(|pair| taken(pair))
    }
}

fn taken<K, V: Clone>(pair: Arc<Pair<K, V>>) -> V {
    Arc::try_unwrap(pair).map_or_else(|pair| pair.value.clone(), |pair| pair.value)
}

fn owned<K: Clone, V: Clone>(pair: Arc<Pair<K, V>>) -> (K, V) {
    let pair = Arc::unwrap_or_clone(pair);
    (pair.key, pair.value)
}

impl<K: Hash + Eq + Clone, V: Clone> Map<K, V> {
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = self.hash(key);
        self.root.find(hash, key)?;
        let mut node = Arc::make_mut(&mut self.root);
        let mut shift = 0;
        loop {
            let position = node.position(bit(hash, shift));
            match &mut node.slots[position] {
                Slot::Pair(pair) => return Some(&mut Arc::make_mut(pair).value),
                Slot::Bucket(bucket) => {
                    return Arc::make_mut(bucket)
                        .iter_mut()
                        .find(|pair| pair.key.borrow() == key)
                        .map(|pair| &mut Arc::make_mut(pair).value);
                }
                Slot::Node(below) => {
                    node = Arc::make_mut(below);
                    shift += BITS;
                }
            }
        }
    }

    pub fn remove_with_key<Q>(&mut self, key: &Q) -> Option<(K, V)>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.take(key).map(owned)
    }

    pub const fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        Entry { map: self, key }
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        IterMut {
            nodes: vec![Arc::make_mut(&mut self.root).slots.iter_mut()],
            bucket: [].iter_mut(),
        }
    }
}

pub struct Entry<'a, K, V> {
    map: &'a mut Map<K, V>,
    key: K,
}

impl<'a, K: Hash + Eq + Clone, V: Clone> Entry<'a, K, V> {
    pub fn or_insert_with(self, value: impl FnOnce() -> V) -> &'a mut V {
        if !self.map.contains_key(&self.key) {
            self.map.place(self.key.clone(), value());
        }
        self.map
            .get_mut(&self.key)
            .expect("the entry was just placed")
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

impl<K, V> Node<K, V> {
    fn pairs(&self) -> Pairs<'_, K, V> {
        Pairs {
            nodes: vec![self.slots.iter()],
            bucket: [].iter(),
        }
    }
}

struct Pairs<'a, K, V> {
    nodes: Vec<std::slice::Iter<'a, Slot<K, V>>>,
    bucket: std::slice::Iter<'a, Arc<Pair<K, V>>>,
}

impl<'a, K, V> Iterator for Pairs<'a, K, V> {
    type Item = &'a Arc<Pair<K, V>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(pair) = self.bucket.next() {
                return Some(pair);
            }
            let slots = self.nodes.last_mut()?;
            match slots.next() {
                Some(Slot::Pair(pair)) => return Some(pair),
                Some(Slot::Bucket(bucket)) => self.bucket = bucket.iter(),
                Some(Slot::Node(below)) => self.nodes.push(below.slots.iter()),
                None => {
                    self.nodes.pop();
                }
            }
        }
    }
}

pub struct Iter<'a, K, V> {
    nodes: Vec<std::slice::Iter<'a, Slot<K, V>>>,
    bucket: std::slice::Iter<'a, Arc<Pair<K, V>>>,
    left: usize,
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(pair) = self.bucket.next() {
                self.left -= 1;
                return Some((&pair.key, &pair.value));
            }
            let slots = self.nodes.last_mut()?;
            match slots.next() {
                Some(Slot::Pair(pair)) => {
                    self.left -= 1;
                    return Some((&pair.key, &pair.value));
                }
                Some(Slot::Bucket(bucket)) => self.bucket = bucket.iter(),
                Some(Slot::Node(below)) => self.nodes.push(below.slots.iter()),
                None => {
                    self.nodes.pop();
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.left, Some(self.left))
    }
}

impl<K, V> ExactSizeIterator for Iter<'_, K, V> {}

pub struct IterMut<'a, K, V> {
    nodes: Vec<std::slice::IterMut<'a, Slot<K, V>>>,
    bucket: std::slice::IterMut<'a, Arc<Pair<K, V>>>,
}

impl<'a, K: Clone, V: Clone> Iterator for IterMut<'a, K, V> {
    type Item = (&'a K, &'a mut V);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(pair) = self.bucket.next() {
                let pair = Arc::make_mut(pair);
                return Some((&pair.key, &mut pair.value));
            }
            let slots = self.nodes.last_mut()?;
            match slots.next() {
                Some(Slot::Pair(pair)) => {
                    let pair = Arc::make_mut(pair);
                    return Some((&pair.key, &mut pair.value));
                }
                Some(Slot::Bucket(bucket)) => self.bucket = Arc::make_mut(bucket).iter_mut(),
                Some(Slot::Node(below)) => self.nodes.push(Arc::make_mut(below).slots.iter_mut()),
                None => {
                    self.nodes.pop();
                }
            }
        }
    }
}

impl<'a, K, V> IntoIterator for &'a Map<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<K: Clone, V: Clone> IntoIterator for Map<K, V> {
    type Item = (K, V);
    type IntoIter = std::vec::IntoIter<(K, V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.root
            .pairs()
            .map(|pair| (pair.key.clone(), pair.value.clone()))
            .collect::<Vec<_>>()
            .into_iter()
    }
}

impl<K: Hash + Eq, V> Extend<(K, V)> for Map<K, V> {
    fn extend<T: IntoIterator<Item = (K, V)>>(&mut self, entries: T) {
        for (key, value) in entries {
            self.place(key, value);
        }
    }
}

impl<K: Hash + Eq, V> FromIterator<(K, V)> for Map<K, V> {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(entries: T) -> Self {
        let mut map = Self::default();
        map.extend(entries);
        map
    }
}

impl<K: std::fmt::Debug, V: std::fmt::Debug> std::fmt::Debug for Map<K, V> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_map().entries(self.iter()).finish()
    }
}

pub struct Set<T>(Map<T, ()>);

impl<T> Clone for Set<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Default for Set<T> {
    fn default() -> Self {
        Self(Map::default())
    }
}

impl<T> Set<T> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.keys()
    }
}

impl<T: Hash + Eq> Set<T> {
    #[must_use]
    pub fn contains<Q>(&self, value: &Q) -> bool
    where
        T: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.0.contains_key(value)
    }

    pub fn insert(&mut self, value: T) -> bool {
        if self.0.contains_key(&value) {
            return false;
        }
        self.0.place(value, ());
        true
    }

    pub fn remove<Q>(&mut self, value: &Q) -> bool
    where
        T: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.0.take(value).is_some()
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        self.0.retain(|value, ()| keep(value));
    }
}

impl<T: Hash + Eq> Extend<T> for Set<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, values: I) {
        for value in values {
            self.insert(value);
        }
    }
}

impl<T: Hash + Eq> FromIterator<T> for Set<T> {
    fn from_iter<I: IntoIterator<Item = T>>(values: I) -> Self {
        let mut set = Self::default();
        set.extend(values);
        set
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Set<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_set().entries(self.iter()).finish()
    }
}
