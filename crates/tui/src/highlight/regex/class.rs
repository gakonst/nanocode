pub(super) const CODE_POINT_MAX: u32 = 0x10_FFFF;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct CodePointSet {
    ranges: Vec<(u32, u32)>,
}

impl CodePointSet {
    pub(super) const fn new() -> Self {
        Self { ranges: Vec::new() }
    }

    pub(super) fn from_ranges(ranges: &[(u32, u32)]) -> Self {
        let mut set = Self {
            ranges: ranges.to_vec(),
        };
        set.normalize();
        set
    }

    fn normalize(&mut self) {
        for &(first, last) in &self.ranges {
            assert!(first <= last, "a range starts before it ends");
        }
        self.ranges.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(self.ranges.len());
        for &(first, last) in &self.ranges {
            match merged.last_mut() {
                Some(previous) if first <= previous.1.saturating_add(1) => {
                    previous.1 = previous.1.max(last);
                }
                _ => merged.push((first, last)),
            }
        }
        self.ranges = merged;
    }

    pub(super) fn ranges(&self) -> &[(u32, u32)] {
        &self.ranges
    }

    pub(super) fn contains(&self, code: u32) -> bool {
        self.ranges
            .binary_search_by(|&(first, last)| {
                if last < code {
                    std::cmp::Ordering::Less
                } else if first > code {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }

    pub(super) fn add_range(&mut self, first: u32, last: u32) {
        assert!(first <= last, "a range starts before it ends");
        let start = self
            .ranges
            .partition_point(|&(_, end)| end.saturating_add(1) < first);
        let mut merged = (first, last);
        let mut end = start;
        while end < self.ranges.len() && self.ranges[end].0 <= last.saturating_add(1) {
            merged.0 = merged.0.min(self.ranges[end].0);
            merged.1 = merged.1.max(self.ranges[end].1);
            end += 1;
        }
        self.ranges.splice(start..end, [merged]);
    }

    pub(super) fn add(&mut self, code: u32) {
        self.add_range(code, code);
    }

    pub(super) fn union_with(&mut self, other: &Self) {
        self.ranges.extend_from_slice(&other.ranges);
        self.normalize();
    }

    pub(super) fn intersection(&self, other: &Self) -> Self {
        let mut ranges = Vec::new();
        let (mut left, mut right) = (0, 0);
        while left < self.ranges.len() && right < other.ranges.len() {
            let (a_first, a_last) = self.ranges[left];
            let (b_first, b_last) = other.ranges[right];
            let first = a_first.max(b_first);
            let last = a_last.min(b_last);
            if first <= last {
                ranges.push((first, last));
            }
            if a_last < b_last {
                left += 1;
            } else {
                right += 1;
            }
        }
        Self { ranges }
    }

    pub(super) fn complement(&self) -> Self {
        let mut ranges = Vec::new();
        let mut next = 0_u32;
        for &(first, last) in &self.ranges {
            if first > next {
                ranges.push((next, first - 1));
            }
            next = last.saturating_add(1);
        }
        if next <= CODE_POINT_MAX
            && self
                .ranges
                .last()
                .is_none_or(|&(_, last)| last < CODE_POINT_MAX)
        {
            ranges.push((next, CODE_POINT_MAX));
        }
        Self { ranges }
    }
}
