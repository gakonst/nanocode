use crate::inner::Bucket;
use core::fmt;
use core::iter::FusedIterator;
use core::slice;
use std::vec;

macro_rules! iterator {
    ($name:ident [$($generics:tt)*], $item:ty, $map:expr) => {
        impl<$($generics)*> Iterator for $name<$($generics)*> {
            type Item = $item;

            #[inline]
            fn next(&mut self) -> Option<Self::Item> {
                self.iter.next().map($map)
            }

            #[inline]
            fn size_hint(&self) -> (usize, Option<usize>) {
                self.iter.size_hint()
            }

            #[inline]
            fn count(self) -> usize {
                self.iter.len()
            }

            #[inline]
            fn nth(&mut self, n: usize) -> Option<Self::Item> {
                self.iter.nth(n).map($map)
            }
        }

        impl<$($generics)*> DoubleEndedIterator for $name<$($generics)*> {
            #[inline]
            fn next_back(&mut self) -> Option<Self::Item> {
                self.iter.next_back().map($map)
            }

            #[inline]
            fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
                self.iter.nth_back(n).map($map)
            }
        }

        impl<$($generics)*> ExactSizeIterator for $name<$($generics)*> {
            #[inline]
            fn len(&self) -> usize {
                self.iter.len()
            }
        }

        impl<$($generics)*> FusedIterator for $name<$($generics)*> {}
    };
}

pub struct Iter<'a, K, V> {
    iter: slice::Iter<'a, Bucket<K, V>>,
}

impl<'a, K, V> Iter<'a, K, V> {
    pub(crate) fn new(entries: &'a [Bucket<K, V>]) -> Self {
        Self {
            iter: entries.iter(),
        }
    }
}

iterator!(Iter['a, K, V], (&'a K, &'a V), Bucket::refs);

impl<K, V> Clone for Iter<'_, K, V> {
    fn clone(&self) -> Self {
        Self {
            iter: self.iter.clone(),
        }
    }
}

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for Iter<'_, K, V> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_list().entries(self.clone()).finish()
    }
}

pub struct IterMut<'a, K, V> {
    iter: slice::IterMut<'a, Bucket<K, V>>,
}

impl<'a, K, V> IterMut<'a, K, V> {
    pub(crate) fn new(entries: &'a mut [Bucket<K, V>]) -> Self {
        Self {
            iter: entries.iter_mut(),
        }
    }
}

iterator!(IterMut['a, K, V], (&'a K, &'a mut V), Bucket::refs_mut);

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for IterMut<'_, K, V> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let iter = self.iter.as_slice().iter().map(Bucket::refs);
        formatter.debug_list().entries(iter).finish()
    }
}

pub struct IntoIter<K, V> {
    iter: vec::IntoIter<Bucket<K, V>>,
}

impl<K, V> IntoIter<K, V> {
    pub(crate) fn new(entries: Vec<Bucket<K, V>>) -> Self {
        Self {
            iter: entries.into_iter(),
        }
    }
}

iterator!(IntoIter[K, V], (K, V), Bucket::into_pair);

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for IntoIter<K, V> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let iter = self.iter.as_slice().iter().map(Bucket::refs);
        formatter.debug_list().entries(iter).finish()
    }
}

pub struct Drain<'a, K, V> {
    iter: vec::Drain<'a, Bucket<K, V>>,
}

impl<'a, K, V> Drain<'a, K, V> {
    pub(crate) const fn new(iter: vec::Drain<'a, Bucket<K, V>>) -> Self {
        Self { iter }
    }
}

iterator!(Drain['a, K, V], (K, V), Bucket::into_pair);

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for Drain<'_, K, V> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let iter = self.iter.as_slice().iter().map(Bucket::refs);
        formatter.debug_list().entries(iter).finish()
    }
}

pub struct Keys<'a, K, V> {
    iter: slice::Iter<'a, Bucket<K, V>>,
}

impl<'a, K, V> Keys<'a, K, V> {
    pub(crate) fn new(entries: &'a [Bucket<K, V>]) -> Self {
        Self {
            iter: entries.iter(),
        }
    }
}

iterator!(Keys['a, K, V], &'a K, Bucket::key_ref);

impl<K, V> Clone for Keys<'_, K, V> {
    fn clone(&self) -> Self {
        Self {
            iter: self.iter.clone(),
        }
    }
}

impl<K: fmt::Debug, V> fmt::Debug for Keys<'_, K, V> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_list().entries(self.clone()).finish()
    }
}

pub struct IntoKeys<K, V> {
    iter: vec::IntoIter<Bucket<K, V>>,
}

impl<K, V> IntoKeys<K, V> {
    pub(crate) fn new(entries: Vec<Bucket<K, V>>) -> Self {
        Self {
            iter: entries.into_iter(),
        }
    }
}

iterator!(IntoKeys[K, V], K, Bucket::into_key);

impl<K: fmt::Debug, V> fmt::Debug for IntoKeys<K, V> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let iter = self.iter.as_slice().iter().map(Bucket::key_ref);
        formatter.debug_list().entries(iter).finish()
    }
}

pub struct Values<'a, K, V> {
    iter: slice::Iter<'a, Bucket<K, V>>,
}

impl<'a, K, V> Values<'a, K, V> {
    pub(crate) fn new(entries: &'a [Bucket<K, V>]) -> Self {
        Self {
            iter: entries.iter(),
        }
    }
}

iterator!(Values['a, K, V], &'a V, Bucket::value_ref);

impl<K, V> Clone for Values<'_, K, V> {
    fn clone(&self) -> Self {
        Self {
            iter: self.iter.clone(),
        }
    }
}

impl<K, V: fmt::Debug> fmt::Debug for Values<'_, K, V> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_list().entries(self.clone()).finish()
    }
}

pub struct ValuesMut<'a, K, V> {
    iter: slice::IterMut<'a, Bucket<K, V>>,
}

impl<'a, K, V> ValuesMut<'a, K, V> {
    pub(crate) fn new(entries: &'a mut [Bucket<K, V>]) -> Self {
        Self {
            iter: entries.iter_mut(),
        }
    }
}

iterator!(ValuesMut['a, K, V], &'a mut V, Bucket::value_mut);

impl<K, V: fmt::Debug> fmt::Debug for ValuesMut<'_, K, V> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let iter = self.iter.as_slice().iter().map(Bucket::value_ref);
        formatter.debug_list().entries(iter).finish()
    }
}

pub struct IntoValues<K, V> {
    iter: vec::IntoIter<Bucket<K, V>>,
}

impl<K, V> IntoValues<K, V> {
    pub(crate) fn new(entries: Vec<Bucket<K, V>>) -> Self {
        Self {
            iter: entries.into_iter(),
        }
    }
}

iterator!(IntoValues[K, V], V, Bucket::into_value);

impl<K, V: fmt::Debug> fmt::Debug for IntoValues<K, V> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let iter = self.iter.as_slice().iter().map(Bucket::value_ref);
        formatter.debug_list().entries(iter).finish()
    }
}
