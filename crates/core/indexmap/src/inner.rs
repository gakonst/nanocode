use crate::table::{LINEAR_ENTRIES_MAX, Table, Vacancy};
use core::mem;

#[derive(Clone)]
pub(crate) struct Bucket<K, V> {
    pub(crate) key: K,
    pub(crate) value: V,
}

impl<K, V> Bucket<K, V> {
    pub(crate) const fn refs(&self) -> (&K, &V) {
        (&self.key, &self.value)
    }

    pub(crate) const fn refs_mut(&mut self) -> (&K, &mut V) {
        (&self.key, &mut self.value)
    }

    pub(crate) fn into_pair(self) -> (K, V) {
        (self.key, self.value)
    }

    pub(crate) const fn key_ref(&self) -> &K {
        &self.key
    }

    pub(crate) const fn value_ref(&self) -> &V {
        &self.value
    }

    pub(crate) const fn value_mut(&mut self) -> &mut V {
        &mut self.value
    }

    pub(crate) fn into_key(self) -> K {
        self.key
    }

    pub(crate) fn into_value(self) -> V {
        self.value
    }
}

#[derive(Clone)]
pub(crate) enum Index {
    Linear,
    Hashed(Table),
}

#[derive(Clone)]
pub(crate) struct Inner<K, V> {
    pub(crate) entries: Vec<Bucket<K, V>>,
    pub(crate) index: Index,
}

impl<K, V> Inner<K, V> {
    pub(crate) const fn new() -> Self {
        Self {
            entries: Vec::new(),
            index: Index::Linear,
        }
    }

    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            index: Index::Linear,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.index = Index::Linear;
        self.entries.clear();
    }

    pub(crate) fn reserve(&mut self, additional: usize) {
        self.entries.reserve(additional);
        if let Index::Hashed(table) = &mut self.index {
            let total = self
                .entries
                .len()
                .checked_add(additional)
                .expect("IndexMap: capacity overflow");
            table.reserve(total);
        }
    }

    pub(crate) fn index_hashes(&mut self, hashes: Vec<u64>) {
        assert!(hashes.len() == self.entries.len());
        let planned = self.entries.capacity().max(self.entries.len() * 2);
        self.index = Index::Hashed(Table::build(hashes, planned));
    }

    pub(crate) fn push(&mut self, key: K, value: V, vacancy: Vacancy) -> usize {
        let index = self.entries.len();
        match (&mut self.index, vacancy) {
            (Index::Linear, Vacancy::Linear) => {
                assert!(index < LINEAR_ENTRIES_MAX);
            }
            (Index::Hashed(table), Vacancy::Hashed { hash, position }) => {
                assert!(table.len() == index);
                table.push(hash, position);
            }
            (Index::Linear, Vacancy::Hashed { .. }) | (Index::Hashed(_), Vacancy::Linear) => {
                panic!("IndexMap: vacancy does not match the index");
            }
        }
        self.entries.push(Bucket { key, value });
        index
    }

    pub(crate) fn swap_remove_index(&mut self, index: usize) -> (K, V) {
        assert!(index < self.entries.len());
        if let Index::Hashed(table) = &mut self.index {
            table.swap_remove(index);
        }
        self.entries.swap_remove(index).into_pair()
    }

    pub(crate) fn shift_remove_index(&mut self, index: usize) -> (K, V) {
        assert!(index < self.entries.len());
        if let Index::Hashed(table) = &mut self.index {
            table.shift_remove(index);
        }
        self.entries.remove(index).into_pair()
    }

    #[track_caller]
    pub(crate) fn move_index(&mut self, from: usize, to: usize) {
        let len = self.entries.len();
        assert!(from < len);
        assert!(
            to < len,
            "index out of bounds: the len is {len} but the index is {to}"
        );
        if from == to {
            return;
        }
        if let Index::Hashed(table) = &mut self.index {
            table.move_index(from, to);
        }
        if from < to {
            self.entries[from..=to].rotate_left(1);
        } else {
            self.entries[to..=from].rotate_right(1);
        }
    }

    pub(crate) fn retain<F>(&mut self, mut keep: F)
    where
        F: FnMut(&mut Bucket<K, V>) -> bool,
    {
        match mem::replace(&mut self.index, Index::Linear) {
            Index::Linear => self.entries.retain_mut(keep),
            Index::Hashed(mut table) => {
                let len = self.entries.len();
                let hashes = table.hashes_mut();
                let mut position = 0;
                let mut kept = 0;
                self.entries.retain_mut(|bucket| {
                    let keeps = keep(bucket);
                    if keeps {
                        hashes[kept] = hashes[position];
                        kept += 1;
                    }
                    position += 1;
                    keeps
                });
                assert!(position == len);
                assert!(kept == self.entries.len());
                if kept == len {
                    self.index = Index::Hashed(table);
                } else {
                    let mut hashes = table.into_hashes();
                    hashes.truncate(kept);
                    self.index = Index::Hashed(Table::build(hashes, kept));
                }
            }
        }
    }

    pub(crate) fn sort_unstable_by<F>(&mut self, mut compare: F)
    where
        F: FnMut(&Bucket<K, V>, &Bucket<K, V>) -> core::cmp::Ordering,
    {
        match mem::replace(&mut self.index, Index::Linear) {
            Index::Linear => self.entries.sort_unstable_by(compare),
            Index::Hashed(table) => {
                let mut order: Vec<usize> = (0..self.entries.len()).collect();
                let entries = &self.entries;
                order.sort_unstable_by(|&left, &right| compare(&entries[left], &entries[right]));
                let mut hashes = table.into_hashes();
                assert!(hashes.len() == self.entries.len());
                for start in 0..order.len() {
                    let mut current = start;
                    for _ in 0..order.len() {
                        let next = order[current];
                        order[current] = current;
                        if next == start {
                            break;
                        }
                        self.entries.swap(current, next);
                        hashes.swap(current, next);
                        current = next;
                    }
                }
                let len = hashes.len();
                self.index = Index::Hashed(Table::build(hashes, len));
            }
        }
    }
}
