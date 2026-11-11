use crate::Atom;
use std::{borrow::Borrow, hash::Hash};

pub struct AtomMap<K, V>(Atom<crate::Map<K, V>>);

impl<K: Hash + Eq + Clone, V: Clone> Default for AtomMap<K, V> {
    fn default() -> Self {
        Self(Atom::new(crate::Map::new()))
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

    pub fn insert(&self, key: K, value: V) -> Option<V> {
        self.0.update(|map| map.insert(key.clone(), value.clone()))
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

    pub fn update<R>(&self, change: impl FnMut(&mut crate::Map<K, V>) -> R) -> R {
        self.0.update(change)
    }
}

#[cfg(not(target_family = "wasm"))]
pub(crate) use aged::Aged;
#[cfg(not(target_family = "wasm"))]
pub use aged::{Kept, Throttle};

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
        entries: crate::Map<K, (Instant, V)>,
        ages: imbl::OrdSet<(Instant, K)>,
    }

    impl<K: Hash + Eq + Ord + Clone, V: Clone> Default for Aged<K, V> {
        fn default() -> Self {
            Self {
                entries: crate::Map::new(),
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

    pub struct Kept<K, V> {
        held: Atom<Aged<K, V>>,
        fresh: Duration,
        kept: usize,
    }

    impl<K: Hash + Eq + Ord + Clone, V: Clone> Kept<K, V> {
        #[must_use]
        pub fn new(fresh: Duration, kept: usize) -> Self {
            Self {
                held: Atom::new(Aged::default()),
                fresh,
                kept,
            }
        }

        pub fn get_or_try<E>(&self, key: &K, make: impl FnOnce() -> Result<V, E>) -> Result<V, E> {
            let now = Instant::now();
            if let Some(found) = self
                .held
                .read(|held| held.fresh(key, now, self.fresh).cloned())
            {
                return Ok(found);
            }
            let made = make()?;
            self.held.update(|held| {
                held.put(key.clone(), now, made.clone(), self.fresh, self.kept);
            });
            Ok(made)
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
