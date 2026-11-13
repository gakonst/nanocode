use std::cmp::{max, min};

pub(super) const CODE_POINT_MAX: u32 = 0x10_FFFF;
pub(super) const ASCII_LAST: u32 = 0x7F;
const TWO_BYTE_FIRST: u32 = 0x80;
const TWO_BYTE_LAST: u32 = 0x7FF;
const BITMAP_WORD_BITS: u32 = 64;
const TWO_BYTE_WORDS: usize = ((TWO_BYTE_LAST + 1 - TWO_BYTE_FIRST) / BITMAP_WORD_BITS) as usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Interval {
    pub(super) first: u32,
    pub(super) last: u32,
}

impl Interval {
    pub(super) const fn new(first: u32, last: u32) -> Self {
        Self { first, last }
    }
}

#[derive(Debug)]
pub(super) struct Class {
    invert: bool,
    ascii: u128,
    two_byte: [u64; TWO_BYTE_WORDS],
    wide: Box<[Interval]>,
}

impl Class {
    pub(super) fn new(invert: bool, set: &CodePointSet) -> Self {
        let wide_start = set
            .intervals
            .partition_point(|interval| interval.last <= TWO_BYTE_LAST);
        let wide = set.intervals[wide_start..]
            .iter()
            .map(|interval| Interval::new(interval.first.max(TWO_BYTE_LAST + 1), interval.last))
            .collect();
        Self {
            invert,
            ascii: set.ascii_mask(),
            two_byte: set.two_byte_bitmap(),
            wide,
        }
    }

    pub(super) fn contains(&self, code_point: u32) -> bool {
        let contained = if code_point <= ASCII_LAST {
            self.ascii & (1 << code_point) != 0
        } else if code_point <= TWO_BYTE_LAST {
            let offset = code_point - TWO_BYTE_FIRST;
            self.two_byte[(offset / BITMAP_WORD_BITS) as usize] & (1 << (offset % BITMAP_WORD_BITS))
                != 0
        } else {
            let index = self
                .wide
                .partition_point(|interval| interval.last < code_point);
            self.wide
                .get(index)
                .is_some_and(|interval| interval.first <= code_point)
        };
        contained != self.invert
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct CodePointSet {
    intervals: Vec<Interval>,
}

impl CodePointSet {
    pub(super) const fn new() -> Self {
        Self {
            intervals: Vec::new(),
        }
    }

    pub(super) fn from_intervals(intervals: &[Interval]) -> Self {
        let mut set = Self::new();
        set.add_set(&Self {
            intervals: intervals.to_vec(),
        });
        set
    }

    pub(super) fn from_sorted(intervals: Vec<Interval>) -> Self {
        for pair in intervals.windows(2) {
            assert!(pair[0].last + 1 < pair[1].first);
        }
        Self { intervals }
    }

    pub(super) fn intervals(&self) -> &[Interval] {
        &self.intervals
    }

    pub(super) const fn is_empty(&self) -> bool {
        self.intervals.is_empty()
    }

    pub(super) fn contains_all(&self) -> bool {
        self.intervals == [Interval::new(0, CODE_POINT_MAX)]
    }

    pub(super) fn add_one(&mut self, code_point: u32) {
        self.add(Interval::new(code_point, code_point));
    }

    pub(super) fn add(&mut self, interval: Interval) {
        assert!(interval.first <= interval.last);
        assert!(interval.last <= CODE_POINT_MAX);
        let start = self
            .intervals
            .partition_point(|existing| existing.last + 1 < interval.first);
        let end = self
            .intervals
            .partition_point(|existing| existing.first <= interval.last + 1);
        if start == end {
            self.intervals.insert(start, interval);
            return;
        }
        let merged = Interval::new(
            min(interval.first, self.intervals[start].first),
            max(interval.last, self.intervals[end - 1].last),
        );
        self.intervals.splice(start..end, [merged]);
    }

    pub(super) fn add_set(&mut self, other: &Self) {
        let left = &self.intervals;
        let right = &other.intervals;
        let mut merged: Vec<Interval> = Vec::with_capacity(left.len() + right.len());
        let (mut left_index, mut right_index) = (0, 0);
        while left_index < left.len() || right_index < right.len() {
            let take_left = right_index == right.len()
                || (left_index < left.len() && left[left_index].first <= right[right_index].first);
            let interval = if take_left {
                left_index += 1;
                left[left_index - 1]
            } else {
                right_index += 1;
                right[right_index - 1]
            };
            match merged.last_mut() {
                Some(last) if interval.first <= last.last + 1 => {
                    last.last = last.last.max(interval.last)
                }
                _ => merged.push(interval),
            }
        }
        self.intervals = merged;
    }

    pub(super) fn ascii_mask(&self) -> u128 {
        let mut mask = 0;
        for interval in &self.intervals {
            for code_point in interval.first..=interval.last.min(ASCII_LAST) {
                mask |= 1 << code_point;
            }
        }
        mask
    }

    pub(super) fn two_byte_bitmap(&self) -> [u64; TWO_BYTE_WORDS] {
        let mut bitmap = [0; TWO_BYTE_WORDS];
        for interval in &self.intervals {
            let first = interval.first.max(TWO_BYTE_FIRST);
            let last = interval.last.min(TWO_BYTE_LAST);
            if first > last {
                continue;
            }
            let low = first - TWO_BYTE_FIRST;
            let high = last - TWO_BYTE_FIRST;
            for word in low / BITMAP_WORD_BITS..=high / BITMAP_WORD_BITS {
                let word_start = word * BITMAP_WORD_BITS;
                let from = low.max(word_start) - word_start;
                let to = high.min(word_start + BITMAP_WORD_BITS - 1) - word_start;
                let width = to - from + 1;
                let mask = if width == BITMAP_WORD_BITS {
                    u64::MAX
                } else {
                    ((1 << width) - 1) << from
                };
                bitmap[word as usize] |= mask;
            }
        }
        bitmap
    }

    pub(super) fn inverted(&self) -> Self {
        let mut intervals = Vec::with_capacity(self.intervals.len() + 1);
        let mut next = 0;
        for interval in &self.intervals {
            if interval.first > next {
                intervals.push(Interval::new(next, interval.first - 1));
            }
            next = interval.last + 1;
        }
        if next <= CODE_POINT_MAX {
            intervals.push(Interval::new(next, CODE_POINT_MAX));
        }
        Self { intervals }
    }
}
