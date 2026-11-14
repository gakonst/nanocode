use std::borrow::Cow;

use nanocode_sys::memchr::{self as scan, Probe};

const SHORT_HAYSTACK: usize = 64;

const RANK: [u8; 256] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 73, 243, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
    24, 25, 26, 27, 28, 29, 255, 177, 225, 195, 164, 170, 186, 221, 233, 234, 176, 174, 242, 224,
    235, 208, 229, 223, 217, 216, 210, 204, 209, 196, 202, 197, 232, 203, 179, 218, 198, 169, 162,
    212, 184, 211, 191, 213, 187, 173, 188, 201, 165, 171, 189, 190, 214, 205, 194, 168, 206, 215,
    207, 180, 175, 172, 166, 167, 163, 193, 185, 192, 160, 236, 226, 251, 230, 244, 245, 254, 238,
    231, 240, 247, 182, 220, 246, 237, 250, 248, 239, 181, 249, 252, 253, 241, 219, 227, 222, 228,
    183, 200, 178, 199, 156, 30, 159, 135, 132, 111, 92, 129, 157, 97, 145, 136, 93, 104, 105, 121,
    79, 86, 130, 110, 158, 143, 150, 102, 94, 149, 118, 148, 90, 107, 144, 147, 83, 127, 101, 100,
    114, 106, 109, 112, 155, 113, 82, 137, 81, 91, 134, 99, 78, 87, 120, 151, 141, 84, 88, 152, 95,
    140, 124, 125, 123, 126, 115, 119, 117, 131, 31, 32, 154, 153, 116, 103, 33, 89, 34, 35, 36,
    70, 37, 69, 146, 142, 96, 108, 38, 39, 40, 41, 42, 43, 44, 76, 45, 46, 47, 48, 49, 50, 71, 138,
    161, 98, 128, 139, 85, 51, 77, 52, 80, 74, 75, 72, 53, 122, 133, 54, 55, 56, 57, 58, 59, 60,
    61, 62, 63, 64, 65, 66, 67, 68,
];

#[inline]
pub fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if haystack.len() < SHORT_HAYSTACK && needle.len() > 1 {
        return rabin_karp::find(haystack, needle);
    }
    Search::new(needle).find(haystack, needle, 0)
}

#[inline]
pub fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if haystack.len() < SHORT_HAYSTACK && needle.len() > 1 {
        return rabin_karp::rfind(haystack, needle);
    }
    Search::new(needle).rfind(haystack, needle, haystack.len())
}

#[inline]
pub fn find_iter<'h, 'n, N: 'n + ?Sized + AsRef<[u8]>>(
    haystack: &'h [u8],
    needle: &'n N,
) -> FindIter<'h, 'n> {
    FindIter::new(haystack, Finder::new(needle))
}

#[inline]
pub fn rfind_iter<'h, 'n, N: 'n + ?Sized + AsRef<[u8]>>(
    haystack: &'h [u8],
    needle: &'n N,
) -> FindRevIter<'h, 'n> {
    FindRevIter::new(haystack, Finder::new(needle))
}

#[derive(Clone, Debug)]
pub struct Finder<'n> {
    needle: Cow<'n, [u8]>,
    search: Search,
}

impl<'n> Finder<'n> {
    #[inline]
    pub fn new<B: ?Sized + AsRef<[u8]>>(needle: &'n B) -> Self {
        let needle = needle.as_ref();
        Self {
            search: Search::new(needle),
            needle: Cow::Borrowed(needle),
        }
    }

    #[inline]
    pub fn find(&self, haystack: &[u8]) -> Option<usize> {
        self.search.find(haystack, &self.needle, 0)
    }

    #[inline]
    pub fn find_iter<'a, 'h>(&'a self, haystack: &'h [u8]) -> FindIter<'h, 'a> {
        FindIter::new(haystack, self.as_ref())
    }

    pub fn needle(&self) -> &[u8] {
        &self.needle
    }

    pub fn as_ref(&self) -> Finder<'_> {
        Finder {
            needle: Cow::Borrowed(&self.needle),
            search: self.search,
        }
    }

    pub fn into_owned(self) -> Finder<'static> {
        Finder {
            needle: Cow::Owned(self.needle.into_owned()),
            search: self.search,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FindIter<'h, 'n> {
    haystack: &'h [u8],
    finder: Finder<'n>,
    position: usize,
}

impl<'h, 'n> FindIter<'h, 'n> {
    const fn new(haystack: &'h [u8], finder: Finder<'n>) -> Self {
        Self {
            haystack,
            finder,
            position: 0,
        }
    }
}

impl Iterator for FindIter<'_, '_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        if self.position > self.haystack.len() {
            return None;
        }
        let found = self
            .finder
            .search
            .find(self.haystack, &self.finder.needle, self.position)?;
        self.position = found + self.finder.needle.len().max(1);
        Some(found)
    }
}

#[derive(Clone, Debug)]
pub struct FindRevIter<'h, 'n> {
    haystack: &'h [u8],
    finder: Finder<'n>,
    end: Option<usize>,
}

impl<'h, 'n> FindRevIter<'h, 'n> {
    const fn new(haystack: &'h [u8], finder: Finder<'n>) -> Self {
        Self {
            haystack,
            finder,
            end: Some(haystack.len()),
        }
    }
}

impl Iterator for FindRevIter<'_, '_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        let end = self.end?;
        let found = self
            .finder
            .search
            .rfind(self.haystack, &self.finder.needle, end)?;
        self.end = if found == end {
            end.checked_sub(1)
        } else {
            Some(found)
        };
        Some(found)
    }
}

#[derive(Clone, Copy, Debug)]
enum Search {
    Empty,
    Byte(u8),
    Pair([Probe; 2]),
}

impl Search {
    fn new(needle: &[u8]) -> Self {
        match needle {
            [] => Self::Empty,
            [byte] => Self::Byte(*byte),
            _ => {
                let first =
                    rarest(needle, |_| true).expect("a needle of two bytes has a rarest byte");
                let second = rarest(needle, |byte| byte != needle[first])
                    .unwrap_or(if first == 0 { needle.len() - 1 } else { 0 });
                let probe = |offset: usize| Probe {
                    offset,
                    byte: needle[offset],
                };
                Self::Pair([probe(first), probe(second)])
            }
        }
    }

    fn find(&self, haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
        match self {
            Self::Empty => (start <= haystack.len()).then_some(start),
            Self::Byte(byte) => {
                scan::find([*byte], haystack.get(start..)?).map(|found| start + found)
            }
            Self::Pair(probes) => {
                let len = needle.len();
                let mut work = 0;
                let stopped =
                    scan::find_pair(
                        haystack,
                        start,
                        probes[0],
                        probes[1],
                        |found| match haystack.get(found..found + len) {
                            Some(window) if !same(window, needle) => {
                                work += len;
                                work > found + 1 - start + len
                            }
                            _ => true,
                        },
                    )?;
                match haystack.get(stopped..stopped + len) {
                    None => None,
                    Some(window) if same(window, needle) => Some(stopped),
                    Some(_) => TwoWay::new(needle).find(haystack, needle, probes, stopped + 1),
                }
            }
        }
    }

    fn rfind(&self, haystack: &[u8], needle: &[u8], end: usize) -> Option<usize> {
        match self {
            Self::Empty => Some(end),
            Self::Byte(byte) => scan::rfind([*byte], &haystack[..end]),
            Self::Pair(probes) => {
                let len = needle.len();
                let reach = probes[0].offset.max(probes[1].offset);
                let mut work = 0;
                let limit = (end + 1 + reach).checked_sub(len)?;
                let stopped = scan::rfind_pair(haystack, limit, probes[0], probes[1], |found| {
                    if same(&haystack[found..found + len], needle) {
                        return true;
                    }
                    work += len;
                    work > end - (found + len - 1) + len
                })?;
                if same(&haystack[stopped..stopped + len], needle) {
                    Some(stopped)
                } else {
                    TwoWay::new(needle).rfind(haystack, needle, probes, stopped + len - 1)
                }
            }
        }
    }
}

fn same(window: &[u8], needle: &[u8]) -> bool {
    let (window_words, window_rest) = window.as_chunks::<{ size_of::<u32>() }>();
    let (needle_words, needle_rest) = needle.as_chunks::<{ size_of::<u32>() }>();
    window_words
        .iter()
        .zip(needle_words)
        .all(|(left, right)| u32::from_ne_bytes(*left) == u32::from_ne_bytes(*right))
        && window_rest.iter().eq(needle_rest)
}

struct TwoWay {
    critical: usize,
    critical_back: usize,
    period: usize,
    byteset: u64,
    long: bool,
}

impl TwoWay {
    fn new(needle: &[u8]) -> Self {
        let (low, low_period) = maximal_suffix(needle, false);
        let (high, high_period) = maximal_suffix(needle, true);
        let (critical, period) = if low > high {
            (low, low_period)
        } else {
            (high, high_period)
        };
        if needle[..critical] == needle[period..period + critical] {
            let back = reverse_maximal_suffix(needle, period, false)
                .max(reverse_maximal_suffix(needle, period, true));
            Self {
                critical,
                critical_back: needle.len() - back,
                period,
                byteset: byteset(&needle[..period]),
                long: false,
            }
        } else {
            Self {
                critical,
                critical_back: critical,
                period: critical.max(needle.len() - critical) + 1,
                byteset: byteset(needle),
                long: true,
            }
        }
    }

    const fn holds(&self, byte: u8) -> bool {
        self.byteset >> (byte as u32 % u64::BITS) & 1 != 0
    }

    fn find(
        &self,
        haystack: &[u8],
        needle: &[u8],
        probes: &[Probe; 2],
        mut position: usize,
    ) -> Option<usize> {
        let len = needle.len();
        let mut memory = 0;
        loop {
            if self.long || memory == 0 {
                position = scan::find_pair(haystack, position, probes[0], probes[1], |_| true)?;
            }
            let tail = *haystack.get(position + len - 1)?;
            if !self.holds(tail) {
                position += len;
                memory = 0;
                continue;
            }
            let start = if self.long {
                self.critical
            } else {
                self.critical.max(memory)
            };
            if let Some(index) =
                (start..len).find(|&index| needle[index] != haystack[position + index])
            {
                position += index - self.critical + 1;
                memory = 0;
                continue;
            }
            let start = if self.long { 0 } else { memory };
            if (start..self.critical)
                .rev()
                .any(|index| needle[index] != haystack[position + index])
            {
                position += self.period;
                if !self.long {
                    memory = len - self.period;
                }
                continue;
            }
            return Some(position);
        }
    }

    fn rfind(
        &self,
        haystack: &[u8],
        needle: &[u8],
        probes: &[Probe; 2],
        mut end: usize,
    ) -> Option<usize> {
        let len = needle.len();
        let reach = probes[0].offset.max(probes[1].offset);
        let mut memory = len;
        loop {
            if self.long || memory == len {
                end = scan::rfind_pair(
                    haystack,
                    (end + 1 + reach).checked_sub(len)?,
                    probes[0],
                    probes[1],
                    |_| true,
                )? + len;
            }
            let start = end.checked_sub(len)?;
            if !self.holds(haystack[start]) {
                end -= len;
                memory = len;
                continue;
            }
            let critical = if self.long {
                self.critical_back
            } else {
                self.critical_back.min(memory)
            };
            if let Some(index) = (0..critical)
                .rev()
                .find(|&index| needle[index] != haystack[start + index])
            {
                end -= self.critical_back - index;
                memory = len;
                continue;
            }
            let stop = if self.long { len } else { memory };
            if (self.critical_back..stop).any(|index| needle[index] != haystack[start + index]) {
                end -= self.period;
                if !self.long {
                    memory = self.period;
                }
                continue;
            }
            return Some(start);
        }
    }
}

fn rarest(needle: &[u8], allowed: impl Fn(u8) -> bool) -> Option<usize> {
    (0..needle.len())
        .filter(|&offset| allowed(needle[offset]))
        .min_by_key(|&offset| RANK[usize::from(needle[offset])])
}

mod rabin_karp {
    use super::same;

    fn hash<'a>(bytes: impl Iterator<Item = &'a u8>) -> u32 {
        bytes.fold(0, |hash, &byte| {
            hash.wrapping_shl(1).wrapping_add(u32::from(byte))
        })
    }

    fn top(len: usize) -> u32 {
        (1..len).fold(1, |power: u32, _| power.wrapping_shl(1))
    }

    const fn roll(window: u32, power: u32, out: u8, inside: u8) -> u32 {
        window
            .wrapping_sub(power.wrapping_mul(out as u32))
            .wrapping_shl(1)
            .wrapping_add(inside as u32)
    }

    pub(super) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        let len = needle.len();
        let last = haystack.len().checked_sub(len)?;
        let target = hash(needle.iter());
        let power = top(len);
        let mut window = hash(haystack[..len].iter());
        for (at, (&out, &inside)) in haystack.iter().zip(&haystack[len..]).enumerate() {
            if window == target && same(&haystack[at..at + len], needle) {
                return Some(at);
            }
            window = roll(window, power, out, inside);
        }
        (window == target && same(&haystack[last..], needle)).then_some(last)
    }

    pub(super) fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        let len = needle.len();
        let last = haystack.len().checked_sub(len)?;
        let target = hash(needle.iter().rev());
        let power = top(len);
        let mut window = hash(haystack[last..].iter().rev());
        for (at, (&inside, &out)) in haystack.iter().zip(&haystack[len..]).enumerate().rev() {
            if window == target && same(&haystack[at + 1..at + 1 + len], needle) {
                return Some(at + 1);
            }
            window = roll(window, power, out, inside);
        }
        (window == target && same(&haystack[..len], needle)).then_some(0)
    }
}

fn byteset(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0, |set, &byte| set | 1 << (u32::from(byte) % u64::BITS))
}

fn maximal_suffix(needle: &[u8], greater: bool) -> (usize, usize) {
    let mut left = 0;
    let mut right = 1;
    let mut offset = 0;
    let mut period = 1;
    while let Some(&candidate) = needle.get(right + offset) {
        let known = needle[left + offset];
        if (candidate < known && !greater) || (candidate > known && greater) {
            right += offset + 1;
            offset = 0;
            period = right - left;
        } else if candidate == known {
            if offset + 1 == period {
                right += offset + 1;
                offset = 0;
            } else {
                offset += 1;
            }
        } else {
            left = right;
            right += 1;
            offset = 0;
            period = 1;
        }
    }
    (left, period)
}

fn reverse_maximal_suffix(needle: &[u8], known_period: usize, greater: bool) -> usize {
    let len = needle.len();
    let mut left = 0;
    let mut right = 1;
    let mut offset = 0;
    let mut period = 1;
    while right + offset < len {
        let candidate = needle[len - (1 + right + offset)];
        let known = needle[len - (1 + left + offset)];
        if (candidate < known && !greater) || (candidate > known && greater) {
            right += offset + 1;
            offset = 0;
            period = right - left;
        } else if candidate == known {
            if offset + 1 == period {
                right += offset + 1;
                offset = 0;
            } else {
                offset += 1;
            }
        } else {
            left = right;
            right += 1;
            offset = 0;
            period = 1;
        }
        if period == known_period {
            break;
        }
    }
    assert!(
        period <= known_period,
        "the reverse suffix period stays within the needle's period"
    );
    left
}
