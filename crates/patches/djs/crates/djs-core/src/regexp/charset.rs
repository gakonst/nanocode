//! Code point sets as sorted ranges, and the spec's Canonicalize for case-insensitive
//! matching.
use crate::PushOut;

use alloc::vec::Vec;

use crate::unicode::case;

pub(crate) const MAX_CP: u32 = 0x10FFFF;

/// A set of code points: sorted, disjoint, non-adjacent inclusive ranges.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CharSet {
    ranges: Vec<(u32, u32)>,
}

impl CharSet {
    pub(crate) fn new() -> CharSet {
        CharSet { ranges: Vec::new() }
    }

    /// A set from ranges that are already sorted, disjoint and non-adjacent.
    pub(crate) fn from_sorted(ranges: Vec<(u32, u32)>) -> CharSet {
        debug_assert!(ranges.windows(2).all(|w| w[0].1 + 1 < w[1].0));
        CharSet { ranges }
    }

    pub(crate) fn ranges(&self) -> &[(u32, u32)] {
        &self.ranges
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub(crate) fn contains(&self, c: u32) -> bool {
        let i = self.ranges.partition_point(|&(lo, _)| lo <= c);
        i > 0 && c <= self.ranges[i - 1].1
    }

    pub(crate) fn add(&mut self, c: u32) {
        self.add_range(c, c);
    }

    pub(crate) fn add_range(&mut self, lo: u32, hi: u32) {
        debug_assert!(lo <= hi);
        // The ranges that touch or overlap [lo, hi] are merged into one.
        let first = self
            .ranges
            .partition_point(|&(_, h)| h.saturating_add(1) < lo);
        let mut last = first;
        let (mut nlo, mut nhi) = (lo, hi);
        while last < self.ranges.len() && self.ranges[last].0 <= hi.saturating_add(1) {
            nlo = nlo.min(self.ranges[last].0);
            nhi = nhi.max(self.ranges[last].1);
            last += 1;
        }
        // Sets are mostly built in order: appending or widening one range are the
        // common cases, and much cheaper than a splice (regexp compile time).
        match last - first {
            0 => self.ranges.insert(first, (nlo, nhi)),
            1 => self.ranges[first] = (nlo, nhi),
            _ => {
                self.ranges
                    .splice(first..last, core::iter::once((nlo, nhi)));
            }
        }
    }

    pub(crate) fn union(&self, other: &CharSet) -> CharSet {
        let mut all: Vec<(u32, u32)> =
            crate::buf::vec_with_capacity(self.ranges.len() + other.ranges.len());
        let (mut i, mut j) = (0, 0);
        while i < self.ranges.len() || j < other.ranges.len() {
            let take_a = j >= other.ranges.len()
                || (i < self.ranges.len() && self.ranges[i].0 <= other.ranges[j].0);
            let r = if take_a {
                i += 1;
                self.ranges[i - 1]
            } else {
                j += 1;
                other.ranges[j - 1]
            };
            match all.last_mut() {
                Some(last) if r.0 <= last.1.saturating_add(1) => last.1 = last.1.max(r.1),
                _ => all.push_out(r),
            }
        }
        CharSet { ranges: all }
    }

    pub(crate) fn intersect(&self, other: &CharSet) -> CharSet {
        let mut out = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < self.ranges.len() && j < other.ranges.len() {
            let (a0, a1) = self.ranges[i];
            let (b0, b1) = other.ranges[j];
            let lo = a0.max(b0);
            let hi = a1.min(b1);
            if lo <= hi {
                out.push_out((lo, hi));
            }
            if a1 < b1 {
                i += 1;
            } else {
                j += 1;
            }
        }
        CharSet { ranges: out }
    }

    /// Whether no code point is in both sets (`intersect(..).is_empty()` without
    /// building the intersection).
    pub(crate) fn is_disjoint(&self, other: &CharSet) -> bool {
        let (mut i, mut j) = (0, 0);
        while i < self.ranges.len() && j < other.ranges.len() {
            let (a0, a1) = self.ranges[i];
            let (b0, b1) = other.ranges[j];
            if a0.max(b0) <= a1.min(b1) {
                return false;
            }
            if a1 < b1 {
                i += 1;
            } else {
                j += 1;
            }
        }
        true
    }

    /// The complement within 0..=0x10FFFF.
    pub(crate) fn invert(&self) -> CharSet {
        let mut out = crate::buf::vec_with_capacity(self.ranges.len() + 1);
        let mut next = 0u32;
        for &(lo, hi) in &self.ranges {
            if lo > next {
                out.push_out((next, lo - 1));
            }
            next = hi + 1;
        }
        if next <= MAX_CP {
            out.push_out((next, MAX_CP));
        }
        CharSet { ranges: out }
    }

    pub(crate) fn subtract(&self, other: &CharSet) -> CharSet {
        self.intersect(&other.invert())
    }

    /// `{ canonicalize(c) | c in self }`.
    pub(crate) fn canonicalize(&self, unicode: bool) -> CharSet {
        if self.ranges.last().is_none_or(|&(_, hi)| hi < 0x80) {
            // ASCII only: canonical forms stay ASCII.
            let mut out = CharSet::new();
            for &(lo, hi) in &self.ranges {
                for c in lo..=hi {
                    out.add(canonicalize(c, unicode));
                }
            }
            return out;
        }
        // Only code points whose canonical form differs from themselves move; walk those
        // (about 1500, in order) next to the set's ranges instead of the set, which can be
        // huge.
        let mut moved = CharSet::new();
        let mut img = CharSet::new();
        let mut i = 0;
        let ranges = &self.ranges;
        for_each_moved(unicode, |c, m| {
            while i < ranges.len() && ranges[i].1 < c {
                i += 1;
            }
            if i < ranges.len() && ranges[i].0 <= c {
                moved.add(c);
                img.add(m);
            }
        });
        if moved.is_empty() {
            return self.clone();
        }
        self.subtract(&moved).union(&img)
    }

    /// `{ c | canonicalize(c) in self }`: the input characters that match `self` in a
    /// case-insensitive test.
    pub(crate) fn case_closure(&self, unicode: bool) -> CharSet {
        let mut out = self.clone();
        if self.ranges.last().is_none_or(|&(_, hi)| hi < 0x80) {
            // ASCII only: the other case, and in unicode mode U+017F and U+212A (the only
            // non-ASCII code points that fold to ASCII).
            for &(lo, hi) in &self.ranges {
                for c in lo..=hi {
                    if (c as u8).is_ascii_alphabetic() {
                        out.add(c ^ 0x20);
                    }
                }
            }
            if unicode {
                if self.contains('s' as u32) {
                    out.add(0x17F);
                }
                if self.contains('k' as u32) {
                    out.add(0x212A);
                }
            }
            return out;
        }
        for_each_moved(unicode, |c, m| {
            if self.contains(m) {
                out.add(c);
            }
        });
        out
    }
}

/// Calls `f(c, canonicalize(c, unicode))` for every `c` that canonicalizes to something
/// else, in increasing order of `c`.
pub(crate) fn for_each_moved(unicode: bool, mut f: impl FnMut(u32, u32)) {
    if unicode {
        case::for_each_fold(f);
    } else {
        case::for_each_upper(|c, m| {
            // Non-unicode Canonicalize: keep characters whose uppercase is several
            // characters or maps non-ASCII to ASCII.
            if c < 0x80 || (m >= 0x80 && case::to_upper_full(c).single().is_some()) {
                f(c, m);
            }
        });
    }
}

/// The spec's Canonicalize(rer, ch): simple case folding in unicode modes; otherwise
/// toUppercase, unless that gives several characters or maps a non-ASCII character to
/// ASCII.
pub(crate) fn canonicalize(c: u32, unicode: bool) -> u32 {
    if c < 0x80 {
        if unicode {
            if (0x41..=0x5A).contains(&c) {
                return c + 0x20;
            }
        } else if (0x61..=0x7A).contains(&c) {
            return c - 0x20;
        }
        return c;
    }
    if unicode {
        return case::simple_fold(c);
    }
    match case::to_upper_full(c).single() {
        Some(u) if u >= 0x80 => u,
        _ => c,
    }
}

/// `\d`
pub(crate) fn digits() -> CharSet {
    CharSet::from_sorted(alloc::vec![(0x30, 0x39)])
}

/// `\s`: WhiteSpace and LineTerminator.
pub(crate) fn spaces() -> CharSet {
    CharSet::from_sorted(alloc::vec![
        (0x09, 0x0D),
        (0x20, 0x20),
        (0xA0, 0xA0),
        (0x1680, 0x1680),
        (0x2000, 0x200A),
        (0x2028, 0x2029),
        (0x202F, 0x202F),
        (0x205F, 0x205F),
        (0x3000, 0x3000),
        (0xFEFF, 0xFEFF),
    ])
}

/// `\w` (the basic word characters; see `is_word_char` for the `ui` extras).
pub(crate) fn word() -> CharSet {
    CharSet::from_sorted(alloc::vec![
        (0x30, 0x39),
        (0x41, 0x5A),
        (0x5F, 0x5F),
        (0x61, 0x7A)
    ])
}

/// IsWordChar for `\b`: ASCII word characters, plus U+017F and U+212A when case-insensitive
/// in a unicode mode (they canonicalize to `s` and `k`).
pub(crate) fn is_word_char(c: u32, unicode_icase: bool) -> bool {
    match c {
        0x30..=0x39 | 0x41..=0x5A | 0x5F | 0x61..=0x7A => true,
        0x17F | 0x212A => unicode_icase,
        _ => false,
    }
}

pub(crate) fn is_line_terminator(c: u32) -> bool {
    matches!(c, 0x0A | 0x0D | 0x2028 | 0x2029)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(r: &[(u32, u32)]) -> CharSet {
        let mut s = CharSet::new();
        for &(lo, hi) in r {
            s.add_range(lo, hi);
        }
        s
    }

    #[test]
    fn set_ops() {
        let a = set(&[(1, 5), (10, 20)]);
        let b = set(&[(4, 12), (30, 30)]);
        assert_eq!(a.union(&b).ranges(), &[(1, 20), (30, 30)]);
        assert_eq!(a.intersect(&b).ranges(), &[(4, 5), (10, 12)]);
        assert_eq!(a.subtract(&b).ranges(), &[(1, 3), (13, 20)]);
        assert!(!a.is_disjoint(&b));
        assert!(a.is_disjoint(&set(&[(6, 9), (21, 40)])));
        assert!(!a.is_disjoint(&set(&[(6, 10)])));
        assert!(set(&[]).is_disjoint(&a));
        assert_eq!(set(&[]).invert().ranges(), &[(0, MAX_CP)]);
        assert_eq!(set(&[(0, 3)]).invert().ranges(), &[(4, MAX_CP)]);
        assert_eq!(a.invert().invert(), a);
        let mut c = CharSet::new();
        for (lo, hi) in [(10, 12), (1, 2), (3, 4), (13, 13), (20, 30), (5, 25)] {
            c.add_range(lo, hi);
        }
        assert_eq!(c.ranges(), &[(1, 30)]);
        assert!(c.contains(1) && c.contains(30) && !c.contains(0) && !c.contains(31));
    }

    #[test]
    fn canonicalize_chars() {
        assert_eq!(canonicalize('a' as u32, false), 'A' as u32);
        assert_eq!(canonicalize('A' as u32, true), 'a' as u32);
        assert_eq!(canonicalize(0x17F, false), 0x17F); // upper is 'S': ASCII, so unchanged
        assert_eq!(canonicalize(0x17F, true), 's' as u32);
        assert_eq!(canonicalize(0xDF, false), 0xDF); // upper is "SS"
        assert_eq!(canonicalize(0xE9, false), 0xC9);
        assert_eq!(canonicalize(0x1F80, false), 0x1F80); // multi-char upper
        assert_eq!(canonicalize(0x3C2, true), 0x3C3);
    }

    #[test]
    fn moved_matches_canonicalize() {
        for unicode in [false, true] {
            let mut moved = Vec::new();
            for_each_moved(unicode, |c, m| {
                assert_eq!(canonicalize(c, unicode), m, "{c:#x}");
                assert_ne!(c, m);
                moved.push_out(c);
            });
            for c in 0..0x11000 {
                let changes = canonicalize(c, unicode) != c;
                assert_eq!(changes, moved.binary_search(&c).is_ok(), "{c:#x} {unicode}");
            }
        }
        // The ASCII fast path of case_closure relies on this.
        let mut to_ascii = Vec::new();
        for_each_moved(true, |c, m| {
            if c >= 0x80 && m < 0x80 {
                to_ascii.push_out(c);
            }
        });
        assert_eq!(to_ascii, [0x17F, 0x212A]);
        let s = set(&[('k' as u32, 'k' as u32)]).case_closure(true);
        assert_eq!(s.ranges(), &[(0x4B, 0x4B), (0x6B, 0x6B), (0x212A, 0x212A)]);
    }

    #[test]
    fn canonicalize_sets() {
        let s = set(&[('a' as u32, 'c' as u32)]).canonicalize(false);
        assert_eq!(s.ranges(), &[('A' as u32, 'C' as u32)]);
        let s = set(&[('A' as u32, 'Z' as u32)]).canonicalize(true);
        assert_eq!(s.ranges(), &[('a' as u32, 'z' as u32)]);
        // Inverting first, then canonicalizing keeps everything except the moved ones.
        let s = set(&[('a' as u32, 'a' as u32)]).invert().canonicalize(true);
        assert!(s.contains('a' as u32)); // 'A' canonicalizes to 'a'
        assert!(!s.contains('A' as u32));
    }
}
