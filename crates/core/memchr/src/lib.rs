use core::iter::{FusedIterator, Rev};

use nanocode_sys::memchr as scan;

pub mod memmem;

#[inline]
pub fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    scan::find([needle], haystack)
}

#[inline]
pub fn memchr2(needle1: u8, needle2: u8, haystack: &[u8]) -> Option<usize> {
    scan::find([needle1, needle2], haystack)
}

#[inline]
pub fn memchr3(needle1: u8, needle2: u8, needle3: u8, haystack: &[u8]) -> Option<usize> {
    scan::find([needle1, needle2, needle3], haystack)
}

#[inline]
pub fn memrchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    scan::rfind([needle], haystack)
}

#[inline]
pub fn memrchr2(needle1: u8, needle2: u8, haystack: &[u8]) -> Option<usize> {
    scan::rfind([needle1, needle2], haystack)
}

#[inline]
pub fn memrchr3(needle1: u8, needle2: u8, needle3: u8, haystack: &[u8]) -> Option<usize> {
    scan::rfind([needle1, needle2, needle3], haystack)
}

#[inline]
pub const fn memchr_iter(needle: u8, haystack: &[u8]) -> Memchr<'_> {
    Memchr::new([needle], haystack)
}

#[inline]
pub const fn memchr2_iter(needle1: u8, needle2: u8, haystack: &[u8]) -> Memchr2<'_> {
    Memchr2::new([needle1, needle2], haystack)
}

#[inline]
pub const fn memchr3_iter(needle1: u8, needle2: u8, needle3: u8, haystack: &[u8]) -> Memchr3<'_> {
    Memchr3::new([needle1, needle2, needle3], haystack)
}

#[inline]
pub fn memrchr_iter(needle: u8, haystack: &[u8]) -> Rev<Memchr<'_>> {
    memchr_iter(needle, haystack).rev()
}

#[inline]
pub fn memrchr2_iter(needle1: u8, needle2: u8, haystack: &[u8]) -> Rev<Memchr2<'_>> {
    memchr2_iter(needle1, needle2, haystack).rev()
}

#[inline]
pub fn memrchr3_iter(needle1: u8, needle2: u8, needle3: u8, haystack: &[u8]) -> Rev<Memchr3<'_>> {
    memchr3_iter(needle1, needle2, needle3, haystack).rev()
}

pub type Memchr<'h> = Bytes<'h, 1>;
pub type Memchr2<'h> = Bytes<'h, 2>;
pub type Memchr3<'h> = Bytes<'h, 3>;

#[derive(Clone, Debug)]
pub struct Bytes<'h, const N: usize> {
    needles: [u8; N],
    haystack: &'h [u8],
    front: usize,
    back: usize,
}

impl<'h, const N: usize> Bytes<'h, N> {
    pub const fn new(needles: [u8; N], haystack: &'h [u8]) -> Self {
        Self {
            needles,
            haystack,
            front: 0,
            back: haystack.len(),
        }
    }
}

impl<const N: usize> Iterator for Bytes<'_, N> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        let found = self.front + scan::find(self.needles, &self.haystack[self.front..self.back])?;
        self.front = found + 1;
        Some(found)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.back - self.front))
    }

    fn count(self) -> usize {
        if N == 1 {
            scan::count(self.needles[0], &self.haystack[self.front..self.back])
        } else {
            self.fold(0, |total, _| total + 1)
        }
    }
}

impl<const N: usize> DoubleEndedIterator for Bytes<'_, N> {
    #[inline]
    fn next_back(&mut self) -> Option<usize> {
        let found = self.front + scan::rfind(self.needles, &self.haystack[self.front..self.back])?;
        self.back = found;
        Some(found)
    }
}

impl<const N: usize> FusedIterator for Bytes<'_, N> {}
