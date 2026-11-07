use crate::Atom;
use std::{borrow::Borrow, hash::Hash};

pub struct AtomMap<K, V>(Atom<imbl::HashMap<K, V>>);

impl<K: Hash + Eq + Clone, V: Clone> Default for AtomMap<K, V> {
    fn default() -> Self {
        Self(Atom::new(imbl::HashMap::new()))
    }
}

impl<K: Hash + Eq + Clone, V: Clone> AtomMap<K, V> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn get<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.0.read(|map| map.get(key).cloned())
    }

    #[must_use]
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.0.read(|map| map.contains_key(key))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.read(imbl::HashMap::len)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.read(imbl::HashMap::is_empty)
    }

    #[must_use]
    pub fn snapshot(&self) -> imbl::HashMap<K, V> {
        self.0.read(Clone::clone)
    }

    pub fn read<R>(&self, read: impl FnOnce(&imbl::HashMap<K, V>) -> R) -> R {
        self.0.read(read)
    }

    pub fn insert(&self, key: K, value: V) -> Option<V> {
        self.0.update(|map| map.insert(key.clone(), value.clone()))
    }

    pub fn remove<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.remove_if(key, |_| true)
    }

    pub fn remove_if<Q>(&self, key: &Q, remove: impl Fn(&V) -> bool) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        if !self.0.read(|map| map.get(key).is_some_and(&remove)) {
            return None;
        }
        self.0.update(|map| {
            if map.get(key).is_some_and(&remove) {
                map.remove(key)
            } else {
                None
            }
        })
    }

    pub fn get_or_insert_with(&self, key: K, make: impl FnOnce() -> V) -> V {
        if let Some(value) = self.get(&key) {
            return value;
        }
        let made = make();
        self.0.update(|map| {
            if let Some(value) = map.get(&key) {
                return value.clone();
            }
            map.insert(key.clone(), made.clone());
            made.clone()
        })
    }

    pub fn update<R>(&self, change: impl FnMut(&mut imbl::HashMap<K, V>) -> R) -> R {
        self.0.update(change)
    }

    pub fn retain(&self, keep: impl Fn(&K, &V) -> bool) {
        if self
            .0
            .read(|map| map.iter().all(|(key, value)| keep(key, value)))
        {
            return;
        }
        self.0
            .update(|map| map.retain(|key, value| keep(key, value)));
    }
}

#[cfg(not(target_family = "wasm"))]
pub(crate) use aged::Aged;
#[cfg(not(target_family = "wasm"))]
pub use aged::Throttle;

#[cfg(not(target_family = "wasm"))]
mod aged {
    use crate::Atom;
    use std::{
        borrow::Borrow,
        hash::Hash,
        time::{Duration, Instant},
    };

    #[derive(Clone)]
    pub(crate) struct Aged<K, V> {
        entries: imbl::HashMap<K, (Instant, V)>,
        ages: imbl::OrdSet<(Instant, K)>,
    }

    impl<K: Hash + Eq + Ord + Clone, V: Clone> Default for Aged<K, V> {
        fn default() -> Self {
            Self {
                entries: imbl::HashMap::new(),
                ages: imbl::OrdSet::new(),
            }
        }
    }

    impl<K: Hash + Eq + Ord + Clone, V: Clone> Aged<K, V> {
        pub(crate) fn fresh<Q>(&self, key: &Q, now: Instant, ttl: Duration) -> Option<&V>
        where
            K: Borrow<Q>,
            Q: Hash + Eq + ?Sized,
        {
            let (at, value) = self.entries.get(key)?;
            (now.saturating_duration_since(*at) < ttl).then_some(value)
        }

        pub(crate) fn held<Q>(&self, key: &Q) -> Option<&V>
        where
            K: Borrow<Q>,
            Q: Hash + Eq + ?Sized,
        {
            self.entries.get(key).map(|(_, value)| value)
        }

        pub(crate) fn put(&mut self, key: K, at: Instant, value: V, ttl: Duration, kept: usize) {
            if let Some((before, _)) = self.entries.insert(key.clone(), (at, value)) {
                self.ages.remove(&(before, key.clone()));
            }
            self.ages.insert((at, key));
            while let Some((oldest, key)) = self.ages.get_min().cloned() {
                if self.entries.len() <= kept && at.saturating_duration_since(oldest) < ttl {
                    break;
                }
                self.ages.remove_min();
                self.entries.remove(&key);
            }
        }

        pub(crate) fn remove<Q>(&mut self, key: &Q) -> Option<V>
        where
            K: Borrow<Q>,
            Q: Hash + Eq + ?Sized,
        {
            let (owned, (at, value)) = self.entries.remove_with_key(key)?;
            self.ages.remove(&(at, owned));
            Some(value)
        }
    }

    pub struct Throttle<K> {
        seen: Atom<Aged<K, ()>>,
        window: Duration,
        kept: usize,
    }

    impl<K: Hash + Eq + Ord + Clone> Throttle<K> {
        #[must_use]
        pub fn new(window: Duration, kept: usize) -> Self {
            Self {
                seen: Atom::new(Aged::default()),
                window,
                kept,
            }
        }

        pub fn allow<Q>(&self, key: &Q) -> bool
        where
            K: Borrow<Q> + for<'q> From<&'q Q>,
            Q: Hash + Eq + ?Sized,
        {
            let now = Instant::now();
            if self
                .seen
                .read(|seen| seen.fresh(key, now, self.window).is_some())
            {
                return false;
            }
            self.seen.update(|seen| {
                if seen.fresh(key, now, self.window).is_some() {
                    return false;
                }
                seen.put(K::from(key), now, (), self.window, self.kept);
                true
            })
        }

        pub fn forget<Q>(&self, key: &Q)
        where
            K: Borrow<Q>,
            Q: Hash + Eq + ?Sized,
        {
            if self.seen.read(|seen| seen.held(key).is_none()) {
                return;
            }
            self.seen.update(|seen| {
                seen.remove(key);
            });
        }
    }
}
