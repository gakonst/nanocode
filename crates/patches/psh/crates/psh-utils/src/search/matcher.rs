use psh_pattern::matcher as pattern;

use crate::matcher::{ByteSet, Captures, LineMatchKind, LineTerminator, Match, Matcher};

#[derive(Clone, Debug)]
pub struct RegexMatcher {
    inner: pattern::RegexMatcher,
    non_matching: ByteSet,
}

impl RegexMatcher {
    pub fn new(inner: pattern::RegexMatcher) -> RegexMatcher {
        let mut non_matching = ByteSet::empty();
        for byte in 0..=u8::MAX {
            if inner.non_matching_bytes().contains(byte) {
                non_matching.add(byte);
            }
        }
        RegexMatcher {
            inner,
            non_matching,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RegexCaptures(pattern::RegexCaptures);

impl Captures for RegexCaptures {
    fn get(&self, index: usize) -> Option<Match> {
        self.0.get(index).map(|(start, end)| Match::new(start, end))
    }
}

impl Matcher for RegexMatcher {
    type Captures = RegexCaptures;

    fn find_at(&self, haystack: &[u8], at: usize) -> Option<Match> {
        self.inner
            .find_at(haystack, at)
            .map(|(start, end)| Match::new(start, end))
    }

    fn new_captures(&self) -> RegexCaptures {
        RegexCaptures(self.inner.new_captures())
    }

    fn captures_at(&self, haystack: &[u8], at: usize, captures: &mut RegexCaptures) -> bool {
        self.inner.captures_at(haystack, at, &mut captures.0)
    }

    fn capture_index(&self, name: &str) -> Option<usize> {
        self.inner.capture_index(name)
    }

    fn shortest_match_at(&self, haystack: &[u8], at: usize) -> Option<usize> {
        self.inner.shortest_match_at(haystack, at)
    }

    fn non_matching_bytes(&self) -> Option<ByteSet> {
        Some(self.non_matching)
    }

    fn line_terminator(&self) -> Option<LineTerminator> {
        self.inner
            .line_terminator()
            .map(|terminator| match terminator {
                pattern::LineTerminator::Crlf => LineTerminator::crlf(),
                pattern::LineTerminator::Byte(byte) => LineTerminator::byte(byte),
            })
    }

    fn find_candidate_line(&self, haystack: &[u8]) -> Option<LineMatchKind> {
        self.inner
            .find_candidate_line(haystack)
            .map(|kind| match kind {
                pattern::LineMatchKind::Confirmed(at) => LineMatchKind::Confirmed(at),
                pattern::LineMatchKind::Candidate(at) => LineMatchKind::Candidate(at),
            })
    }
}
