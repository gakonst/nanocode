use alloc::vec::Vec;

use crate::unicode;

pub const SCALAR_MAX: u32 = 0x10FFFF;
const SURROGATE_FIRST: u32 = 0xD800;
const SURROGATE_LAST: u32 = 0xDFFF;
const ASCII_MAX: u32 = 0x7F;
const BYTE_MAX: u32 = 0xFF;
const ASCII_CASE_OFFSET: u8 = b'a' - b'A';

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Domain {
    Unicode,
    Bytes,
}

impl Domain {
    fn max(self) -> u32 {
        match self {
            Domain::Unicode => SCALAR_MAX,
            Domain::Bytes => BYTE_MAX,
        }
    }

    fn increment(self, value: u32) -> u32 {
        match self {
            Domain::Unicode if value == SURROGATE_FIRST - 1 => SURROGATE_LAST + 1,
            _ => value + 1,
        }
    }

    fn decrement(self, value: u32) -> u32 {
        match self {
            Domain::Unicode if value == SURROGATE_LAST + 1 => SURROGATE_FIRST - 1,
            _ => value - 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Range {
    pub start: u32,
    pub end: u32,
}

impl Range {
    pub fn new(a: u32, b: u32) -> Range {
        if a <= b {
            Range { start: a, end: b }
        } else {
            Range { start: b, end: a }
        }
    }

    pub fn len(&self) -> usize {
        (self.end - self.start + 1) as usize
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Set {
    ranges: Vec<Range>,
}

impl Set {
    fn new(ranges: impl IntoIterator<Item = Range>) -> Set {
        let mut set = Set {
            ranges: ranges.into_iter().collect(),
        };
        set.canonicalize();
        set
    }

    fn canonicalize(&mut self) {
        if self.is_canonical() {
            return;
        }
        self.ranges.sort();
        let mut out: Vec<Range> = Vec::with_capacity(self.ranges.len());
        for range in self.ranges.drain(..) {
            if let Some(last) = out.last_mut()
                && range.start <= last.end.saturating_add(1)
            {
                if range.end > last.end {
                    last.end = range.end;
                }
                continue;
            }
            out.push(range);
        }
        self.ranges = out;
    }

    fn is_canonical(&self) -> bool {
        self.ranges
            .windows(2)
            .all(|pair| pair[0] < pair[1] && pair[0].end.saturating_add(1) < pair[1].start)
    }

    fn push(&mut self, range: Range) {
        self.ranges.push(range);
        self.canonicalize();
    }

    fn union(&mut self, other: &Set) {
        if other.ranges.is_empty() || self.ranges == other.ranges {
            return;
        }
        self.ranges.extend_from_slice(&other.ranges);
        self.canonicalize();
    }

    fn negate(&mut self, domain: Domain) {
        let mut out = Vec::with_capacity(self.ranges.len() + 1);
        if self.ranges.is_empty() {
            out.push(Range {
                start: 0,
                end: domain.max(),
            });
            self.ranges = out;
            return;
        }
        if self.ranges[0].start > 0 {
            out.push(Range::new(0, domain.decrement(self.ranges[0].start)));
        }
        for pair in self.ranges.windows(2) {
            out.push(Range::new(
                domain.increment(pair[0].end),
                domain.decrement(pair[1].start),
            ));
        }
        let last = self.ranges[self.ranges.len() - 1].end;
        if last < domain.max() {
            out.push(Range::new(domain.increment(last), domain.max()));
        }
        self.ranges = out;
    }

    fn intersect(&mut self, other: &Set) {
        let mut out = Vec::new();
        let (mut a, mut b) = (0, 0);
        while a < self.ranges.len() && b < other.ranges.len() {
            let x = self.ranges[a];
            let y = other.ranges[b];
            let start = x.start.max(y.start);
            let end = x.end.min(y.end);
            if start <= end {
                out.push(Range { start, end });
            }
            if x.end < y.end { a += 1 } else { b += 1 }
        }
        self.ranges = out;
    }

    fn difference(&mut self, domain: Domain, other: &Set) {
        if self.ranges.is_empty() || other.ranges.is_empty() {
            return;
        }
        let mut out = Vec::with_capacity(self.ranges.len());
        let (mut a, mut b) = (0, 0);
        'ranges: while a < self.ranges.len() && b < other.ranges.len() {
            if other.ranges[b].end < self.ranges[a].start {
                b += 1;
                continue;
            }
            if self.ranges[a].end < other.ranges[b].start {
                out.push(self.ranges[a]);
                a += 1;
                continue;
            }
            let mut range = self.ranges[a];
            while b < other.ranges.len()
                && range.start.max(other.ranges[b].start) <= range.end.min(other.ranges[b].end)
            {
                let before = range;
                let removed = other.ranges[b];
                let lower = (removed.start > range.start)
                    .then(|| Range::new(range.start, domain.decrement(removed.start)));
                let upper = (removed.end < range.end)
                    .then(|| Range::new(domain.increment(removed.end), range.end));
                range = match (lower, upper) {
                    (None, None) => {
                        a += 1;
                        continue 'ranges;
                    }
                    (Some(one), None) | (None, Some(one)) => one,
                    (Some(lower), Some(upper)) => {
                        out.push(lower);
                        upper
                    }
                };
                if removed.end > before.end {
                    break;
                }
                b += 1;
            }
            out.push(range);
            a += 1;
        }
        out.extend_from_slice(&self.ranges[a..]);
        self.ranges = out;
    }

    fn symmetric_difference(&mut self, domain: Domain, other: &Set) {
        let mut both = self.clone();
        both.intersect(other);
        self.union(other);
        self.difference(domain, &both);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassUnicode {
    set: Set,
}

impl ClassUnicode {
    pub fn new(ranges: impl IntoIterator<Item = Range>) -> ClassUnicode {
        ClassUnicode {
            set: Set::new(ranges),
        }
    }

    pub fn from_table(table: &[(u32, u32)]) -> ClassUnicode {
        ClassUnicode::new(table.iter().map(|&(start, end)| Range { start, end }))
    }

    pub fn empty() -> ClassUnicode {
        ClassUnicode {
            set: Set { ranges: Vec::new() },
        }
    }

    pub fn ranges(&self) -> &[Range] {
        &self.set.ranges
    }

    pub fn push(&mut self, range: Range) {
        self.set.push(range);
    }

    pub fn union(&mut self, other: &ClassUnicode) {
        self.set.union(&other.set);
    }

    pub fn intersect(&mut self, other: &ClassUnicode) {
        self.set.intersect(&other.set);
    }

    pub fn difference(&mut self, other: &ClassUnicode) {
        self.set.difference(Domain::Unicode, &other.set);
    }

    pub fn symmetric_difference(&mut self, other: &ClassUnicode) {
        self.set.symmetric_difference(Domain::Unicode, &other.set);
    }

    pub fn negate(&mut self) {
        self.set.negate(Domain::Unicode);
    }

    pub fn case_fold_simple(&mut self) {
        let mut added = Vec::new();
        for range in &self.set.ranges {
            unicode::simple_fold_range(range.start, range.end, &mut added);
        }
        if added.is_empty() {
            return;
        }
        self.set
            .ranges
            .extend(added.into_iter().map(|c| Range { start: c, end: c }));
        self.set.canonicalize();
    }

    pub fn is_ascii(&self) -> bool {
        self.set.ranges.last().is_none_or(|r| r.end <= ASCII_MAX)
    }

    pub fn minimum_len(&self) -> Option<usize> {
        self.set.ranges.first().map(|r| utf8_len(r.start))
    }

    pub fn maximum_len(&self) -> Option<usize> {
        self.set.ranges.last().map(|r| utf8_len(r.end))
    }

    pub fn literal(&self) -> Option<Vec<u8>> {
        let ranges = &self.set.ranges;
        if ranges.len() == 1 && ranges[0].start == ranges[0].end {
            let c = char::from_u32(ranges[0].start).expect("class ranges hold scalar values");
            let mut buffer = [0; 4];
            Some(c.encode_utf8(&mut buffer).as_bytes().to_vec())
        } else {
            None
        }
    }

    pub fn to_byte_class(&self) -> Option<ClassBytes> {
        if !self.is_ascii() {
            return None;
        }
        Some(ClassBytes::new(self.set.ranges.iter().copied()))
    }

    pub fn contains(&self, c: u32) -> bool {
        self.set
            .ranges
            .binary_search_by(|r| {
                if r.end < c {
                    core::cmp::Ordering::Less
                } else if r.start > c {
                    core::cmp::Ordering::Greater
                } else {
                    core::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassBytes {
    set: Set,
}

impl ClassBytes {
    pub fn new(ranges: impl IntoIterator<Item = Range>) -> ClassBytes {
        ClassBytes {
            set: Set::new(ranges),
        }
    }

    pub fn empty() -> ClassBytes {
        ClassBytes {
            set: Set { ranges: Vec::new() },
        }
    }

    pub fn ranges(&self) -> &[Range] {
        &self.set.ranges
    }

    pub fn push(&mut self, range: Range) {
        self.set.push(range);
    }

    pub fn union(&mut self, other: &ClassBytes) {
        self.set.union(&other.set);
    }

    pub fn intersect(&mut self, other: &ClassBytes) {
        self.set.intersect(&other.set);
    }

    pub fn difference(&mut self, other: &ClassBytes) {
        self.set.difference(Domain::Bytes, &other.set);
    }

    pub fn symmetric_difference(&mut self, other: &ClassBytes) {
        self.set.symmetric_difference(Domain::Bytes, &other.set);
    }

    pub fn negate(&mut self) {
        self.set.negate(Domain::Bytes);
    }

    pub fn case_fold_simple(&mut self) {
        let mut added = Vec::new();
        let lower = (u32::from(b'a'), u32::from(b'z'));
        let upper = (u32::from(b'A'), u32::from(b'Z'));
        let offset = u32::from(ASCII_CASE_OFFSET);
        for range in &self.set.ranges {
            let (start, end) = (range.start.max(lower.0), range.end.min(lower.1));
            if start <= end {
                added.push(Range {
                    start: start - offset,
                    end: end - offset,
                });
            }
            let (start, end) = (range.start.max(upper.0), range.end.min(upper.1));
            if start <= end {
                added.push(Range {
                    start: start + offset,
                    end: end + offset,
                });
            }
        }
        if added.is_empty() {
            return;
        }
        self.set.ranges.extend(added);
        self.set.canonicalize();
    }

    pub fn is_ascii(&self) -> bool {
        self.set.ranges.last().is_none_or(|r| r.end <= ASCII_MAX)
    }

    pub fn literal(&self) -> Option<Vec<u8>> {
        let ranges = &self.set.ranges;
        if ranges.len() == 1 && ranges[0].start == ranges[0].end {
            Some(alloc::vec![ranges[0].start as u8])
        } else {
            None
        }
    }

    pub fn to_unicode_class(&self) -> Option<ClassUnicode> {
        if !self.is_ascii() {
            return None;
        }
        Some(ClassUnicode::new(self.set.ranges.iter().copied()))
    }

    pub fn contains(&self, byte: u8) -> bool {
        let b = u32::from(byte);
        self.set.ranges.iter().any(|r| r.start <= b && b <= r.end)
    }
}

pub fn utf8_len(c: u32) -> usize {
    match c {
        0..=0x7F => 1,
        0x80..=0x7FF => 2,
        0x800..=0xFFFF => 3,
        _ => 4,
    }
}
