// Copied from fd (https://github.com/sharkdp/fd, commit 193d0df), src/filter/time.rs,
// with the psh patches marked `psh:` (D1138):
// Copyright (c) 2017-present The fd developers. Licensed under the MIT License (see NOTICE).

// psh: no jiff. Only the `@timestamp` form is parsed here; durations (`2weeks`) and
// dates (`2018-10-27 10:00:00`) need the shared date parser (0020) and run the real fd
// until then (D1082). Times are the Host's: nanoseconds since the epoch.

/// Filter based on time ranges.
#[derive(Debug, PartialEq, Eq)]
pub enum TimeFilter {
    Before(i64),
    After(i64),
}

impl TimeFilter {
    fn from_str(s: &str) -> Option<i64> {
        let timestamp_secs: u64 = s.strip_prefix('@')?.parse().ok()?;
        i64::try_from(timestamp_secs)
            .ok()?
            .checked_mul(1_000_000_000)
    }

    pub fn before(s: &str) -> Option<Self> {
        Self::from_str(s).map(TimeFilter::Before)
    }

    pub fn after(s: &str) -> Option<Self> {
        Self::from_str(s).map(TimeFilter::After)
    }

    pub fn applies_to(&self, t: i64) -> bool {
        match self {
            Self::Before(limit) => t < *limit,
            Self::After(limit) => t > *limit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_time_filter_applicable() {
        // psh: the `@timestamp` part of upstream's test.
        let s = 1_000_000_000i64;
        let ref_timestamp = 1707723412u64; // Mon Feb 12 07:36:52 UTC 2024
        let ref_time = ref_timestamp as i64 * s;
        let t1m_ago = ref_time - 60 * s;
        let t1s_later = ref_time + s;
        // Timestamp only supported via '@' prefix
        assert!(TimeFilter::before(&ref_timestamp.to_string()).is_none());
        assert!(
            TimeFilter::before(&format!("@{ref_timestamp}"))
                .unwrap()
                .applies_to(t1m_ago)
        );
        assert!(
            !TimeFilter::before(&format!("@{ref_timestamp}"))
                .unwrap()
                .applies_to(t1s_later)
        );
        assert!(
            !TimeFilter::after(&format!("@{ref_timestamp}"))
                .unwrap()
                .applies_to(t1m_ago)
        );
        assert!(
            TimeFilter::after(&format!("@{ref_timestamp}"))
                .unwrap()
                .applies_to(t1s_later)
        );
        // psh: what the real fd parses.
        assert!(TimeFilter::after("1day").is_none());
        assert!(TimeFilter::after("2018-10-27").is_none());
    }

    #[test]
    fn out_of_range_unix_timestamp_is_rejected() {
        // A '@' timestamp large enough to overflow SystemTime must return
        // None rather than panicking.
        assert!(TimeFilter::before(&format!("@{}", u64::MAX)).is_none());
        assert!(TimeFilter::after(&format!("@{}", u64::MAX)).is_none());
    }
}
