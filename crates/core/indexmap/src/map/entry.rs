use crate::inner::Inner;
use crate::table::Vacancy;
use core::fmt;

pub enum Entry<'a, K, V> {
    Occupied(OccupiedEntry<'a, K, V>),
    Vacant(VacantEntry<'a, K, V>),
}

impl<'a, K, V> Entry<'a, K, V> {
    pub const fn index(&self) -> usize {
        match self {
            Self::Occupied(entry) => entry.index(),
            Self::Vacant(entry) => entry.index(),
        }
    }

    pub fn key(&self) -> &K {
        match self {
            Self::Occupied(entry) => entry.key(),
            Self::Vacant(entry) => entry.key(),
        }
    }

    pub fn or_insert(self, default: V) -> &'a mut V {
        match self {
            Self::Occupied(entry) => entry.into_mut(),
            Self::Vacant(entry) => entry.insert(default),
        }
    }

    pub fn or_insert_with<F>(self, call: F) -> &'a mut V
    where
        F: FnOnce() -> V,
    {
        match self {
            Self::Occupied(entry) => entry.into_mut(),
            Self::Vacant(entry) => entry.insert(call()),
        }
    }

    pub fn or_insert_with_key<F>(self, call: F) -> &'a mut V
    where
        F: FnOnce(&K) -> V,
    {
        match self {
            Self::Occupied(entry) => entry.into_mut(),
            Self::Vacant(entry) => {
                let value = call(entry.key());
                entry.insert(value)
            }
        }
    }

    pub fn or_default(self) -> &'a mut V
    where
        V: Default,
    {
        match self {
            Self::Occupied(entry) => entry.into_mut(),
            Self::Vacant(entry) => entry.insert(V::default()),
        }
    }

    #[must_use]
    pub fn and_modify<F>(mut self, modify: F) -> Self
    where
        F: FnOnce(&mut V),
    {
        if let Self::Occupied(entry) = &mut self {
            modify(entry.get_mut());
        }
        self
    }
}

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for Entry<'_, K, V> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut tuple = formatter.debug_tuple("Entry");
        match self {
            Self::Vacant(entry) => tuple.field(entry),
            Self::Occupied(entry) => tuple.field(entry),
        };
        tuple.finish()
    }
}

pub struct OccupiedEntry<'a, K, V> {
    inner: &'a mut Inner<K, V>,
    index: usize,
}

impl<'a, K, V> OccupiedEntry<'a, K, V> {
    pub(crate) fn new(inner: &'a mut Inner<K, V>, index: usize) -> Self {
        assert!(index < inner.entries.len());
        Self { inner, index }
    }

    pub const fn index(&self) -> usize {
        self.index
    }

    pub fn key(&self) -> &K {
        &self.inner.entries[self.index].key
    }

    pub fn get(&self) -> &V {
        &self.inner.entries[self.index].value
    }

    pub fn get_mut(&mut self) -> &mut V {
        &mut self.inner.entries[self.index].value
    }

    pub fn into_mut(self) -> &'a mut V {
        &mut self.inner.entries[self.index].value
    }

    pub fn insert(&mut self, value: V) -> V {
        core::mem::replace(self.get_mut(), value)
    }

    pub fn swap_remove(self) -> V {
        self.swap_remove_entry().1
    }

    pub fn shift_remove(self) -> V {
        self.shift_remove_entry().1
    }

    pub fn swap_remove_entry(self) -> (K, V) {
        self.inner.swap_remove_index(self.index)
    }

    pub fn shift_remove_entry(self) -> (K, V) {
        self.inner.shift_remove_index(self.index)
    }

    #[track_caller]
    pub fn move_index(self, to: usize) {
        self.inner.move_index(self.index, to);
    }
}

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for OccupiedEntry<'_, K, V> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OccupiedEntry")
            .field("key", self.key())
            .field("value", self.get())
            .finish()
    }
}

pub struct VacantEntry<'a, K, V> {
    inner: &'a mut Inner<K, V>,
    key: K,
    vacancy: Vacancy,
}

impl<'a, K, V> VacantEntry<'a, K, V> {
    pub(crate) const fn new(inner: &'a mut Inner<K, V>, key: K, vacancy: Vacancy) -> Self {
        Self {
            inner,
            key,
            vacancy,
        }
    }

    pub const fn index(&self) -> usize {
        self.inner.entries.len()
    }

    pub const fn key(&self) -> &K {
        &self.key
    }

    pub fn into_key(self) -> K {
        self.key
    }

    pub fn insert(self, value: V) -> &'a mut V {
        let index = self.inner.push(self.key, value, self.vacancy);
        &mut self.inner.entries[index].value
    }

    #[track_caller]
    pub fn shift_insert(self, index: usize, value: V) -> &'a mut V {
        let len = self.inner.entries.len();
        assert!(
            index <= len,
            "index out of bounds: the len is {len} but the index is {index}. Expected index <= len"
        );
        let last = self.inner.push(self.key, value, self.vacancy);
        self.inner.move_index(last, index);
        &mut self.inner.entries[index].value
    }
}

impl<K: fmt::Debug, V> fmt::Debug for VacantEntry<'_, K, V> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("VacantEntry")
            .field(self.key())
            .finish()
    }
}
