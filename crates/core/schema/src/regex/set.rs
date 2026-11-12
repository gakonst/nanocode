use std::cmp::{max, min};

pub(super) const CODE_POINT_MAX: u32 = 0x10_FFFF;
pub(super) const ASCII_LAST: u32 = 0x7F;

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

    pub(super) fn contains(&self, code_point: u32) -> bool {
        let index = self
            .intervals
            .partition_point(|interval| interval.last < code_point);
        self.intervals
            .get(index)
            .is_some_and(|interval| interval.first <= code_point)
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
        let mut merged: Vec<Interval> =
            Vec::with_capacity(self.intervals.len() + other.intervals.len());
        let mut left = self.intervals.iter().peekable();
        let mut right = other.intervals.iter().peekable();
        loop {
            let next = match (left.peek(), right.peek()) {
                (Some(a), Some(b)) if a.first <= b.first => left.next(),
                (Some(_), Some(_)) => right.next(),
                (Some(_), None) => left.next(),
                (None, Some(_)) => right.next(),
                (None, None) => break,
            };
            let interval = *next.expect("one side has an interval");
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
