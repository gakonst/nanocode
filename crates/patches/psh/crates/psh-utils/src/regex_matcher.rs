use grep_matcher::NoError;
use grep_regex::{RegexCaptures, RegexMatcher};

use crate::matcher::{ByteSet, Captures, LineMatchKind, LineTerminator, Match, Matcher};

const BYTE_VALUES: std::ops::RangeInclusive<u8> = 0..=u8::MAX;

fn infallible<T>(result: Result<T, NoError>) -> T {
    match result {
        Ok(value) => value,
        Err(_) => unreachable!("grep-regex's matcher has no errors"),
    }
}

fn own_match(found: grep_matcher::Match) -> Match {
    Match::new(found.start(), found.end())
}

impl Captures for RegexCaptures {
    fn get(&self, index: usize) -> Option<Match> {
        grep_matcher::Captures::get(self, index).map(own_match)
    }
}

impl Matcher for RegexMatcher {
    type Captures = RegexCaptures;

    fn find_at(&self, haystack: &[u8], at: usize) -> Option<Match> {
        infallible(grep_matcher::Matcher::find_at(self, haystack, at)).map(own_match)
    }

    fn new_captures(&self) -> RegexCaptures {
        infallible(grep_matcher::Matcher::new_captures(self))
    }

    fn captures_at(&self, haystack: &[u8], at: usize, captures: &mut RegexCaptures) -> bool {
        infallible(grep_matcher::Matcher::captures_at(
            self, haystack, at, captures,
        ))
    }

    fn capture_index(&self, name: &str) -> Option<usize> {
        grep_matcher::Matcher::capture_index(self, name)
    }

    fn shortest_match_at(&self, haystack: &[u8], at: usize) -> Option<usize> {
        infallible(grep_matcher::Matcher::shortest_match_at(self, haystack, at))
    }

    fn non_matching_bytes(&self) -> Option<ByteSet> {
        let theirs = grep_matcher::Matcher::non_matching_bytes(self)?;
        let mut ours = ByteSet::empty();
        for byte in BYTE_VALUES.filter(|&byte| theirs.contains(byte)) {
            ours.add(byte);
        }
        Some(ours)
    }

    fn line_terminator(&self) -> Option<LineTerminator> {
        grep_matcher::Matcher::line_terminator(self).map(|line_terminator| {
            if line_terminator.is_crlf() {
                LineTerminator::crlf()
            } else {
                LineTerminator::byte(line_terminator.as_byte())
            }
        })
    }

    fn find_candidate_line(&self, haystack: &[u8]) -> Option<LineMatchKind> {
        infallible(grep_matcher::Matcher::find_candidate_line(self, haystack)).map(
            |kind| match kind {
                grep_matcher::LineMatchKind::Confirmed(offset) => LineMatchKind::Confirmed(offset),
                grep_matcher::LineMatchKind::Candidate(offset) => LineMatchKind::Candidate(offset),
            },
        )
    }
}
