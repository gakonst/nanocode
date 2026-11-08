use crate::Atom;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

struct Kept<V> {
    value: V,
    weight: usize,
    used: AtomicU64,
}

type Batch<V> = Vec<(Arc<str>, Arc<Kept<V>>)>;

struct Shard<V> {
    map: HashMap<Arc<str>, Arc<Kept<V>>>,
    weight: usize,
}

impl<V> Clone for Shard<V> {
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
            weight: self.weight,
        }
    }
}

pub struct Weighed<V> {
    shards: Vec<Atom<Shard<V>>>,
    limit: usize,
    clock: AtomicU64,
    held: AtomicU64,
}

impl<V: Clone> Weighed<V> {
    #[must_use]
    pub fn new(budget: usize, shards: NonZeroUsize) -> Self {
        Self {
            shards: (0..shards.get())
                .map(|_| {
                    Atom::new(Shard {
                        map: HashMap::new(),
                        weight: 0,
                    })
                })
                .collect(),
            limit: budget / shards.get(),
            clock: AtomicU64::new(0),
            held: AtomicU64::new(0),
        }
    }

    fn at(&self, key: &str) -> usize {
        let hash = key.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
        });
        usize::try_from(hash).unwrap_or_default() % self.shards.len()
    }

    pub fn get(&self, key: &str) -> Option<V> {
        let tick = self.clock.fetch_add(1, Ordering::Relaxed);
        self.shards[self.at(key)].read(|shard| {
            shard.map.get(key).map(|kept| {
                kept.used.store(tick, Ordering::Relaxed);
                kept.value.clone()
            })
        })
    }

    pub fn remove(&self, key: &str) -> u64 {
        let gone = self.shards[self.at(key)].update(|shard| {
            let gone = shard.map.remove(key).map_or(0, |gone| gone.weight);
            shard.weight -= gone;
            gone
        });
        let gone = u64::try_from(gone).unwrap_or(0);
        self.held
            .fetch_sub(gone, Ordering::Relaxed)
            .saturating_sub(gone)
    }

    pub fn put(&self, entries: impl IntoIterator<Item = (String, V, usize)>) -> u64 {
        let mut by_shard: Vec<Batch<V>> = self.shards.iter().map(|_| Vec::new()).collect();
        for (key, value, weight) in entries {
            if weight > self.limit {
                continue;
            }
            let used = AtomicU64::new(self.clock.fetch_add(1, Ordering::Relaxed));
            by_shard[self.at(&key)].push((
                key.into(),
                Arc::new(Kept {
                    value,
                    weight,
                    used,
                }),
            ));
        }
        for (at, batch) in by_shard.into_iter().enumerate() {
            if batch.is_empty() {
                continue;
            }
            let (added, removed) = self.shards[at].update(|shard| {
                let (mut added, mut removed) = (0, 0);
                for (key, kept) in &batch {
                    if let Some(old) = shard.map.insert(Arc::clone(key), Arc::clone(kept)) {
                        shard.weight -= old.weight;
                        removed += old.weight;
                    }
                    shard.weight += kept.weight;
                    added += kept.weight;
                }
                if shard.weight > self.limit {
                    let mut order: Vec<(u64, Arc<str>)> = shard
                        .map
                        .iter()
                        .map(|(key, kept)| (kept.used.load(Ordering::Relaxed), Arc::clone(key)))
                        .collect();
                    order.sort_unstable();
                    for (_, key) in order {
                        if shard.weight <= self.limit - self.limit / 10 {
                            break;
                        }
                        if let Some(gone) = shard.map.remove(&key) {
                            shard.weight -= gone.weight;
                            removed += gone.weight;
                        }
                    }
                }
                (added, removed)
            });
            self.held
                .fetch_add(u64::try_from(added).unwrap_or(0), Ordering::Relaxed);
            self.held
                .fetch_sub(u64::try_from(removed).unwrap_or(0), Ordering::Relaxed);
        }
        self.held.load(Ordering::Relaxed)
    }
}
