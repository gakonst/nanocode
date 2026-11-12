use std::ops::{Add, AddAssign};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Started(Option<std::time::Instant>);

impl Started {
    pub(crate) fn now() -> Started {
        if cfg!(all(target_arch = "wasm32", target_os = "unknown")) {
            Started(None)
        } else {
            Started(Some(std::time::Instant::now()))
        }
    }

    pub(crate) fn elapsed(&self) -> Duration {
        self.0.map_or(Duration::ZERO, |started| started.elapsed())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Stats {
    elapsed: Duration,
    searches: u64,
    searches_with_match: u64,
    bytes_searched: u64,
    bytes_printed: u64,
    matched_lines: u64,
    matches: u64,
}

impl Stats {
    pub(crate) fn new() -> Stats {
        Stats::default()
    }

    pub(crate) fn elapsed(&self) -> Duration {
        self.elapsed
    }

    pub(crate) fn searches(&self) -> u64 {
        self.searches
    }

    pub(crate) fn searches_with_match(&self) -> u64 {
        self.searches_with_match
    }

    pub(crate) fn bytes_searched(&self) -> u64 {
        self.bytes_searched
    }

    pub(crate) fn bytes_printed(&self) -> u64 {
        self.bytes_printed
    }

    pub(crate) fn matched_lines(&self) -> u64 {
        self.matched_lines
    }

    pub(crate) fn matches(&self) -> u64 {
        self.matches
    }

    pub(crate) fn add_elapsed(&mut self, duration: Duration) {
        self.elapsed += duration;
    }

    pub(crate) fn add_searches(&mut self, count: u64) {
        self.searches += count;
    }

    pub(crate) fn add_searches_with_match(&mut self, count: u64) {
        self.searches_with_match += count;
    }

    pub(crate) fn add_bytes_searched(&mut self, count: u64) {
        self.bytes_searched += count;
    }

    pub(crate) fn add_bytes_printed(&mut self, count: u64) {
        self.bytes_printed += count;
    }

    pub(crate) fn add_matched_lines(&mut self, count: u64) {
        self.matched_lines += count;
    }

    pub(crate) fn add_matches(&mut self, count: u64) {
        self.matches += count;
    }

    pub(crate) fn record_search(
        &mut self,
        started: Started,
        matched: bool,
        bytes_searched: u64,
        bytes_printed: u64,
    ) {
        self.add_elapsed(started.elapsed());
        self.add_searches(1);
        if matched {
            self.add_searches_with_match(1);
        }
        self.add_bytes_searched(bytes_searched);
        self.add_bytes_printed(bytes_printed);
    }
}

impl Add<&Stats> for Stats {
    type Output = Stats;

    fn add(self, other: &Stats) -> Stats {
        Stats {
            elapsed: self.elapsed + other.elapsed,
            searches: self.searches + other.searches,
            searches_with_match: self.searches_with_match + other.searches_with_match,
            bytes_searched: self.bytes_searched + other.bytes_searched,
            bytes_printed: self.bytes_printed + other.bytes_printed,
            matched_lines: self.matched_lines + other.matched_lines,
            matches: self.matches + other.matches,
        }
    }
}

impl AddAssign<&Stats> for Stats {
    fn add_assign(&mut self, other: &Stats) {
        *self = std::mem::take(self) + other;
    }
}
