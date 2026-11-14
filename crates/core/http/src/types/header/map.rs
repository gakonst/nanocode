use std::{fmt, iter, ops::Index, slice, vec};

use super::{name::HeaderName, value::HeaderValue};

#[derive(Clone)]
struct Bucket<T> {
    key: HeaderName,
    value: T,
    extra: Vec<T>,
}

#[derive(Clone)]
pub struct HeaderMap<T = HeaderValue> {
    buckets: Vec<Bucket<T>>,
}

mod sealed {
    pub trait Sealed {}
}

pub trait AsHeaderName: sealed::Sealed {
    #[doc(hidden)]
    fn position<T>(&self, map: &HeaderMap<T>) -> Option<usize>;
}

pub trait IntoHeaderName: sealed::Sealed {
    #[doc(hidden)]
    fn into_name(self) -> HeaderName;
}

impl sealed::Sealed for HeaderName {}
impl sealed::Sealed for &HeaderName {}
impl sealed::Sealed for &str {}
impl sealed::Sealed for String {}
impl sealed::Sealed for &String {}

impl AsHeaderName for HeaderName {
    fn position<T>(&self, map: &HeaderMap<T>) -> Option<usize> {
        map.position_of(self)
    }
}

impl AsHeaderName for &HeaderName {
    fn position<T>(&self, map: &HeaderMap<T>) -> Option<usize> {
        map.position_of(self)
    }
}

impl AsHeaderName for &str {
    fn position<T>(&self, map: &HeaderMap<T>) -> Option<usize> {
        map.position_ignoring_case(self.as_bytes())
    }
}

impl AsHeaderName for String {
    fn position<T>(&self, map: &HeaderMap<T>) -> Option<usize> {
        map.position_ignoring_case(self.as_bytes())
    }
}

impl AsHeaderName for &String {
    fn position<T>(&self, map: &HeaderMap<T>) -> Option<usize> {
        map.position_ignoring_case(self.as_bytes())
    }
}

impl IntoHeaderName for HeaderName {
    fn into_name(self) -> HeaderName {
        self
    }
}

impl IntoHeaderName for &HeaderName {
    fn into_name(self) -> HeaderName {
        self.clone()
    }
}

impl IntoHeaderName for &'static str {
    fn into_name(self) -> HeaderName {
        HeaderName::from_bytes(self.as_bytes()).expect("static str is invalid name")
    }
}

impl<T> Default for HeaderMap<T> {
    fn default() -> Self {
        Self {
            buckets: Vec::new(),
        }
    }
}

impl HeaderMap {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<T> HeaderMap<T> {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buckets: Vec::with_capacity(capacity),
        }
    }

    fn position_of(&self, key: &HeaderName) -> Option<usize> {
        self.buckets.iter().position(|bucket| bucket.key == *key)
    }

    fn position_ignoring_case(&self, key: &[u8]) -> Option<usize> {
        self.buckets
            .iter()
            .position(|bucket| bucket.key.matches_ignoring_case(key))
    }

    pub fn len(&self) -> usize {
        self.buckets
            .iter()
            .map(|bucket| 1 + bucket.extra.len())
            .sum()
    }

    pub const fn keys_len(&self) -> usize {
        self.buckets.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }

    pub fn clear(&mut self) {
        self.buckets.clear();
    }

    pub const fn capacity(&self) -> usize {
        self.buckets.capacity()
    }

    pub fn reserve(&mut self, additional: usize) {
        self.buckets.reserve(additional);
    }

    pub fn get<K: AsHeaderName>(&self, key: K) -> Option<&T> {
        key.position(self).map(|at| &self.buckets[at].value)
    }

    pub fn get_mut<K: AsHeaderName>(&mut self, key: K) -> Option<&mut T> {
        key.position(self).map(|at| &mut self.buckets[at].value)
    }

    pub fn get_all<K: AsHeaderName>(&self, key: K) -> GetAll<'_, T> {
        GetAll {
            bucket: key.position(self).map(|at| &self.buckets[at]),
        }
    }

    pub fn contains_key<K: AsHeaderName>(&self, key: K) -> bool {
        key.position(self).is_some()
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            buckets: self.buckets.iter(),
            current: None,
        }
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        IterMut {
            buckets: self.buckets.iter_mut(),
            current: None,
        }
    }

    pub fn keys(&self) -> Keys<'_, T> {
        Keys {
            buckets: self.buckets.iter(),
        }
    }

    pub fn values(&self) -> Values<'_, T> {
        Values { inner: self.iter() }
    }

    pub fn values_mut(&mut self) -> ValuesMut<'_, T> {
        ValuesMut {
            inner: self.iter_mut(),
        }
    }

    pub fn drain(&mut self) -> Drain<'_, T> {
        Drain {
            buckets: self.buckets.drain(..),
            extra: None,
        }
    }

    pub fn entry<K: IntoHeaderName>(&mut self, key: K) -> Entry<'_, T> {
        let key = key.into_name();
        match self.position_of(&key) {
            Some(at) => Entry::Occupied(OccupiedEntry { map: self, at }),
            None => Entry::Vacant(VacantEntry { map: self, key }),
        }
    }

    pub fn insert<K: IntoHeaderName>(&mut self, key: K, value: T) -> Option<T> {
        let key = key.into_name();
        match self.position_of(&key) {
            Some(at) => {
                let bucket = &mut self.buckets[at];
                bucket.extra.clear();
                Some(std::mem::replace(&mut bucket.value, value))
            }
            None => {
                self.push(key, value);
                None
            }
        }
    }

    pub fn append<K: IntoHeaderName>(&mut self, key: K, value: T) -> bool {
        let key = key.into_name();
        match self.position_of(&key) {
            Some(at) => {
                self.buckets[at].extra.push(value);
                true
            }
            None => {
                self.push(key, value);
                false
            }
        }
    }

    fn push(&mut self, key: HeaderName, value: T) -> usize {
        self.buckets.push(Bucket {
            key,
            value,
            extra: Vec::new(),
        });
        self.buckets.len() - 1
    }

    pub fn remove<K: AsHeaderName>(&mut self, key: K) -> Option<T> {
        key.position(self)
            .map(|at| self.buckets.swap_remove(at).value)
    }
}

impl<T> IntoIterator for HeaderMap<T> {
    type Item = (Option<HeaderName>, T);
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> IntoIter<T> {
        IntoIter {
            buckets: self.buckets.into_iter(),
            extra: None,
        }
    }
}

impl<'a, T> IntoIterator for &'a HeaderMap<T> {
    type Item = (&'a HeaderName, &'a T);
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut HeaderMap<T> {
    type Item = (&'a HeaderName, &'a mut T);
    type IntoIter = IterMut<'a, T>;

    fn into_iter(self) -> IterMut<'a, T> {
        self.iter_mut()
    }
}

impl<T> FromIterator<(HeaderName, T)> for HeaderMap<T> {
    fn from_iter<I: IntoIterator<Item = (HeaderName, T)>>(items: I) -> Self {
        let mut map = Self::default();
        map.extend(items);
        map
    }
}

impl<T> Extend<(HeaderName, T)> for HeaderMap<T> {
    fn extend<I: IntoIterator<Item = (HeaderName, T)>>(&mut self, items: I) {
        for (key, value) in items {
            self.append(key, value);
        }
    }
}

impl<T> Extend<(Option<HeaderName>, T)> for HeaderMap<T> {
    fn extend<I: IntoIterator<Item = (Option<HeaderName>, T)>>(&mut self, items: I) {
        let mut items = items.into_iter();
        let (mut key, mut value) = match items.next() {
            Some((Some(key), value)) => (key, value),
            Some((None, _)) => panic!("expected a header name, but got None"),
            None => return,
        };
        loop {
            let at = match self.position_of(&key) {
                Some(at) => {
                    let bucket = &mut self.buckets[at];
                    bucket.extra.clear();
                    bucket.value = value;
                    at
                }
                None => self.push(key, value),
            };
            loop {
                match items.next() {
                    Some((Some(next_key), next_value)) => {
                        key = next_key;
                        value = next_value;
                        break;
                    }
                    Some((None, extra)) => self.buckets[at].extra.push(extra),
                    None => return,
                }
            }
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for HeaderMap<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_map().entries(self.iter()).finish()
    }
}

impl<T: PartialEq> PartialEq for HeaderMap<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .keys()
                .all(|key| self.get_all(key) == other.get_all(key))
    }
}

impl<T: Eq> Eq for HeaderMap<T> {}

impl<K: AsHeaderName, T> Index<K> for HeaderMap<T> {
    type Output = T;

    fn index(&self, key: K) -> &T {
        self.get(key).expect("no entry found for key")
    }
}

pub struct GetAll<'a, T> {
    bucket: Option<&'a Bucket<T>>,
}

impl<'a, T> GetAll<'a, T> {
    pub fn iter(&self) -> ValueIter<'a, T> {
        ValueIter::new(self.bucket)
    }
}

impl<'a, T> IntoIterator for GetAll<'a, T> {
    type Item = &'a T;
    type IntoIter = ValueIter<'a, T>;

    fn into_iter(self) -> ValueIter<'a, T> {
        ValueIter::new(self.bucket)
    }
}

impl<'a, T> IntoIterator for &GetAll<'a, T> {
    type Item = &'a T;
    type IntoIter = ValueIter<'a, T>;

    fn into_iter(self) -> ValueIter<'a, T> {
        ValueIter::new(self.bucket)
    }
}

impl<T: PartialEq> PartialEq for GetAll<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}

impl<T: fmt::Debug> fmt::Debug for GetAll<'_, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_list().entries(self.iter()).finish()
    }
}

pub struct ValueIter<'a, T> {
    first: Option<&'a T>,
    extra: slice::Iter<'a, T>,
}

impl<'a, T> ValueIter<'a, T> {
    fn new(bucket: Option<&'a Bucket<T>>) -> Self {
        match bucket {
            Some(bucket) => Self {
                first: Some(&bucket.value),
                extra: bucket.extra.iter(),
            },
            None => Self {
                first: None,
                extra: [].iter(),
            },
        }
    }
}

impl<'a, T> Iterator for ValueIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        self.first.take().or_else(|| self.extra.next())
    }
}

impl<T> DoubleEndedIterator for ValueIter<'_, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.extra.next_back().or_else(|| self.first.take())
    }
}

pub struct ValueIterMut<'a, T> {
    first: Option<&'a mut T>,
    extra: slice::IterMut<'a, T>,
}

impl<'a, T> Iterator for ValueIterMut<'a, T> {
    type Item = &'a mut T;

    fn next(&mut self) -> Option<&'a mut T> {
        self.first.take().or_else(|| self.extra.next())
    }
}

pub struct Iter<'a, T> {
    buckets: slice::Iter<'a, Bucket<T>>,
    current: Option<(&'a HeaderName, slice::Iter<'a, T>)>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = (&'a HeaderName, &'a T);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some((key, extra)) = &mut self.current
            && let Some(value) = extra.next()
        {
            return Some((*key, value));
        }
        let bucket = self.buckets.next()?;
        self.current = Some((&bucket.key, bucket.extra.iter()));
        Some((&bucket.key, &bucket.value))
    }
}

pub struct IterMut<'a, T> {
    buckets: slice::IterMut<'a, Bucket<T>>,
    current: Option<(&'a HeaderName, slice::IterMut<'a, T>)>,
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = (&'a HeaderName, &'a mut T);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some((key, extra)) = &mut self.current
            && let Some(value) = extra.next()
        {
            return Some((*key, value));
        }
        let Bucket { key, value, extra } = self.buckets.next()?;
        let key: &'a HeaderName = key;
        self.current = Some((key, extra.iter_mut()));
        Some((key, value))
    }
}

pub struct Keys<'a, T> {
    buckets: slice::Iter<'a, Bucket<T>>,
}

impl<'a, T> Iterator for Keys<'a, T> {
    type Item = &'a HeaderName;

    fn next(&mut self) -> Option<&'a HeaderName> {
        self.buckets.next().map(|bucket| &bucket.key)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.buckets.size_hint()
    }
}

impl<T> ExactSizeIterator for Keys<'_, T> {}

pub struct Values<'a, T> {
    inner: Iter<'a, T>,
}

impl<'a, T> Iterator for Values<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        self.inner.next().map(|(_, value)| value)
    }
}

pub struct ValuesMut<'a, T> {
    inner: IterMut<'a, T>,
}

impl<'a, T> Iterator for ValuesMut<'a, T> {
    type Item = &'a mut T;

    fn next(&mut self) -> Option<&'a mut T> {
        self.inner.next().map(|(_, value)| value)
    }
}

pub struct Drain<'a, T> {
    buckets: vec::Drain<'a, Bucket<T>>,
    extra: Option<vec::IntoIter<T>>,
}

impl<T> Iterator for Drain<'_, T> {
    type Item = (Option<HeaderName>, T);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(value) = self.extra.as_mut().and_then(Iterator::next) {
            return Some((None, value));
        }
        let bucket = self.buckets.next()?;
        self.extra = Some(bucket.extra.into_iter());
        Some((Some(bucket.key), bucket.value))
    }
}

pub struct IntoIter<T> {
    buckets: vec::IntoIter<Bucket<T>>,
    extra: Option<vec::IntoIter<T>>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = (Option<HeaderName>, T);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(value) = self.extra.as_mut().and_then(Iterator::next) {
            return Some((None, value));
        }
        let bucket = self.buckets.next()?;
        self.extra = Some(bucket.extra.into_iter());
        Some((Some(bucket.key), bucket.value))
    }
}

pub enum Entry<'a, T: 'a> {
    Occupied(OccupiedEntry<'a, T>),
    Vacant(VacantEntry<'a, T>),
}

pub struct OccupiedEntry<'a, T> {
    map: &'a mut HeaderMap<T>,
    at: usize,
}

pub struct VacantEntry<'a, T> {
    map: &'a mut HeaderMap<T>,
    key: HeaderName,
}

impl<'a, T> Entry<'a, T> {
    pub fn or_insert(self, default: T) -> &'a mut T {
        match self {
            Self::Occupied(entry) => entry.into_mut(),
            Self::Vacant(entry) => entry.insert(default),
        }
    }

    pub fn or_insert_with<F: FnOnce() -> T>(self, default: F) -> &'a mut T {
        match self {
            Self::Occupied(entry) => entry.into_mut(),
            Self::Vacant(entry) => entry.insert(default()),
        }
    }

    pub fn or_insert_with_key<F: FnOnce(&HeaderName) -> T>(self, default: F) -> &'a mut T {
        match self {
            Self::Occupied(entry) => entry.into_mut(),
            Self::Vacant(entry) => {
                let value = default(&entry.key);
                entry.insert(value)
            }
        }
    }

    pub fn key(&self) -> &HeaderName {
        match self {
            Self::Occupied(entry) => entry.key(),
            Self::Vacant(entry) => entry.key(),
        }
    }
}

impl<'a, T> VacantEntry<'a, T> {
    pub const fn key(&self) -> &HeaderName {
        &self.key
    }

    pub fn into_key(self) -> HeaderName {
        self.key
    }

    pub fn insert(self, value: T) -> &'a mut T {
        let at = self.map.push(self.key, value);
        &mut self.map.buckets[at].value
    }

    pub fn insert_entry(self, value: T) -> OccupiedEntry<'a, T> {
        let at = self.map.push(self.key, value);
        OccupiedEntry { map: self.map, at }
    }
}

impl<'a, T> OccupiedEntry<'a, T> {
    pub fn key(&self) -> &HeaderName {
        &self.map.buckets[self.at].key
    }

    pub fn get(&self) -> &T {
        &self.map.buckets[self.at].value
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.map.buckets[self.at].value
    }

    pub fn into_mut(self) -> &'a mut T {
        &mut self.map.buckets[self.at].value
    }

    pub fn insert(&mut self, value: T) -> T {
        let bucket = &mut self.map.buckets[self.at];
        bucket.extra.clear();
        std::mem::replace(&mut bucket.value, value)
    }

    pub fn append(&mut self, value: T) {
        self.map.buckets[self.at].extra.push(value);
    }

    pub fn remove(self) -> T {
        self.remove_entry().1
    }

    pub fn remove_entry(self) -> (HeaderName, T) {
        let bucket = self.map.buckets.swap_remove(self.at);
        (bucket.key, bucket.value)
    }

    pub fn iter(&self) -> ValueIter<'_, T> {
        ValueIter::new(Some(&self.map.buckets[self.at]))
    }

    pub fn iter_mut(&mut self) -> ValueIterMut<'_, T> {
        let bucket = &mut self.map.buckets[self.at];
        ValueIterMut {
            first: Some(&mut bucket.value),
            extra: bucket.extra.iter_mut(),
        }
    }
}

impl<T> iter::FusedIterator for Iter<'_, T> {}
