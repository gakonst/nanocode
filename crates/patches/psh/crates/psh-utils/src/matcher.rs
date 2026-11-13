use std::fmt;
use std::ops::{Index, IndexMut};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct Match {
    start: usize,
    end: usize,
}

impl Match {
    #[inline]
    pub(crate) fn new(start: usize, end: usize) -> Match {
        assert!(start <= end, "match start {start} is past its end {end}");
        Match { start, end }
    }

    #[inline]
    pub(crate) fn zero(offset: usize) -> Match {
        Match {
            start: offset,
            end: offset,
        }
    }

    #[inline]
    pub(crate) fn start(&self) -> usize {
        self.start
    }

    #[inline]
    pub(crate) fn end(&self) -> usize {
        self.end
    }

    #[inline]
    pub(crate) fn with_start(&self, start: usize) -> Match {
        Match::new(start, self.end)
    }

    #[inline]
    pub(crate) fn with_end(&self, end: usize) -> Match {
        Match::new(self.start, end)
    }

    #[inline]
    pub(crate) fn offset(&self, amount: usize) -> Match {
        Match {
            start: self.start + amount,
            end: self.end + amount,
        }
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

impl Index<Match> for [u8] {
    type Output = [u8];

    #[inline]
    fn index(&self, range: Match) -> &[u8] {
        &self[range.start..range.end]
    }
}

impl IndexMut<Match> for [u8] {
    #[inline]
    fn index_mut(&mut self, range: Match) -> &mut [u8] {
        &mut self[range.start..range.end]
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum LineTerminatorKind {
    Byte(u8),
    CarriageReturnLineFeed,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(crate) struct LineTerminator(LineTerminatorKind);

impl LineTerminator {
    #[inline]
    pub(crate) fn byte(byte: u8) -> LineTerminator {
        LineTerminator(LineTerminatorKind::Byte(byte))
    }

    #[inline]
    pub(crate) fn crlf() -> LineTerminator {
        LineTerminator(LineTerminatorKind::CarriageReturnLineFeed)
    }

    #[inline]
    pub(crate) fn is_crlf(&self) -> bool {
        self.0 == LineTerminatorKind::CarriageReturnLineFeed
    }

    #[inline]
    pub(crate) fn as_byte(&self) -> u8 {
        match self.0 {
            LineTerminatorKind::Byte(byte) => byte,
            LineTerminatorKind::CarriageReturnLineFeed => b'\n',
        }
    }

    #[inline]
    pub(crate) fn as_bytes(&self) -> &[u8] {
        match self.0 {
            LineTerminatorKind::Byte(ref byte) => std::slice::from_ref(byte),
            LineTerminatorKind::CarriageReturnLineFeed => b"\r\n",
        }
    }

    #[inline]
    pub(crate) fn is_suffix(&self, slice: &[u8]) -> bool {
        slice.last() == Some(&self.as_byte())
    }
}

impl Default for LineTerminator {
    fn default() -> LineTerminator {
        LineTerminator::byte(b'\n')
    }
}

impl fmt::Debug for LineTerminator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            LineTerminatorKind::Byte(byte) => write!(f, "LineTerminator(Byte({byte}))"),
            LineTerminatorKind::CarriageReturnLineFeed => f.write_str("LineTerminator(CRLF)"),
        }
    }
}

const BYTE_SET_WORDS: usize = 4;
const BITS_PER_WORD: u8 = 64;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ByteSet([u64; BYTE_SET_WORDS]);

impl ByteSet {
    pub(crate) fn empty() -> ByteSet {
        ByteSet([0; BYTE_SET_WORDS])
    }

    pub(crate) fn add(&mut self, byte: u8) {
        self.0[usize::from(byte / BITS_PER_WORD)] |= 1 << (byte % BITS_PER_WORD);
    }

    #[inline]
    pub(crate) fn contains(&self, byte: u8) -> bool {
        self.0[usize::from(byte / BITS_PER_WORD)] & (1 << (byte % BITS_PER_WORD)) != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LineMatchKind {
    Confirmed(usize),
    Candidate(usize),
}

pub(crate) trait Captures {
    fn get(&self, index: usize) -> Option<Match>;

    fn interpolate<F>(
        &self,
        name_to_index: F,
        haystack: &[u8],
        replacement: &[u8],
        destination: &mut Vec<u8>,
    ) where
        F: FnMut(&str) -> Option<usize>,
    {
        interpolate(
            replacement,
            |index, destination| {
                if let Some(range) = self.get(index) {
                    destination.extend_from_slice(&haystack[range]);
                }
            },
            name_to_index,
            destination,
        );
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NoCaptures;

impl Captures for NoCaptures {
    fn get(&self, _: usize) -> Option<Match> {
        None
    }
}

pub(crate) trait Matcher {
    type Captures: Captures;

    fn find_at(&self, haystack: &[u8], at: usize) -> Option<Match>;

    fn new_captures(&self) -> Self::Captures;

    fn captures_at(&self, _haystack: &[u8], _at: usize, _captures: &mut Self::Captures) -> bool {
        false
    }

    fn capture_index(&self, _name: &str) -> Option<usize> {
        None
    }

    fn shortest_match_at(&self, haystack: &[u8], at: usize) -> Option<usize> {
        self.find_at(haystack, at).map(|m| m.end())
    }

    fn non_matching_bytes(&self) -> Option<ByteSet> {
        None
    }

    fn line_terminator(&self) -> Option<LineTerminator> {
        None
    }

    fn find_candidate_line(&self, haystack: &[u8]) -> Option<LineMatchKind> {
        self.shortest_match_at(haystack, 0)
            .map(LineMatchKind::Confirmed)
    }

    fn find(&self, haystack: &[u8]) -> Option<Match> {
        self.find_at(haystack, 0)
    }

    fn is_match(&self, haystack: &[u8]) -> bool {
        self.shortest_match_at(haystack, 0).is_some()
    }

    fn find_iter_at<F>(&self, haystack: &[u8], at: usize, mut matched: F)
    where
        F: FnMut(Match) -> bool,
    {
        let mut next = NextSearch::new(at);
        while let Some(from) = next.from(haystack) {
            let Some(found) = self.find_at(haystack, from) else {
                return;
            };
            if next.skips(found) {
                continue;
            }
            if !matched(found) {
                return;
            }
        }
    }

    fn captures_iter_at<F>(
        &self,
        haystack: &[u8],
        at: usize,
        captures: &mut Self::Captures,
        mut matched: F,
    ) where
        F: FnMut(&Self::Captures) -> bool,
    {
        let mut next = NextSearch::new(at);
        while let Some(from) = next.from(haystack) {
            if !self.captures_at(haystack, from, captures) {
                return;
            }
            let found = captures.get(0).expect("a capture match has group 0");
            if next.skips(found) {
                continue;
            }
            if !matched(captures) {
                return;
            }
        }
    }
}

impl<M: Matcher> Matcher for &M {
    type Captures = M::Captures;

    fn find_at(&self, haystack: &[u8], at: usize) -> Option<Match> {
        (*self).find_at(haystack, at)
    }

    fn new_captures(&self) -> M::Captures {
        (*self).new_captures()
    }

    fn captures_at(&self, haystack: &[u8], at: usize, captures: &mut M::Captures) -> bool {
        (*self).captures_at(haystack, at, captures)
    }

    fn capture_index(&self, name: &str) -> Option<usize> {
        (*self).capture_index(name)
    }

    fn shortest_match_at(&self, haystack: &[u8], at: usize) -> Option<usize> {
        (*self).shortest_match_at(haystack, at)
    }

    fn non_matching_bytes(&self) -> Option<ByteSet> {
        (*self).non_matching_bytes()
    }

    fn line_terminator(&self) -> Option<LineTerminator> {
        (*self).line_terminator()
    }

    fn find_candidate_line(&self, haystack: &[u8]) -> Option<LineMatchKind> {
        (*self).find_candidate_line(haystack)
    }

    fn find(&self, haystack: &[u8]) -> Option<Match> {
        (*self).find(haystack)
    }

    fn is_match(&self, haystack: &[u8]) -> bool {
        (*self).is_match(haystack)
    }

    fn find_iter_at<F>(&self, haystack: &[u8], at: usize, matched: F)
    where
        F: FnMut(Match) -> bool,
    {
        (*self).find_iter_at(haystack, at, matched)
    }

    fn captures_iter_at<F>(
        &self,
        haystack: &[u8],
        at: usize,
        captures: &mut M::Captures,
        matched: F,
    ) where
        F: FnMut(&M::Captures) -> bool,
    {
        (*self).captures_iter_at(haystack, at, captures, matched)
    }
}

struct NextSearch {
    from: usize,
    last_match_end: Option<usize>,
}

impl NextSearch {
    fn new(at: usize) -> NextSearch {
        NextSearch {
            from: at,
            last_match_end: None,
        }
    }

    fn from(&self, haystack: &[u8]) -> Option<usize> {
        (self.from <= haystack.len()).then_some(self.from)
    }

    fn skips(&mut self, found: Match) -> bool {
        if found.is_empty() {
            self.from = found.end() + 1;
            if self.last_match_end == Some(found.end()) {
                return true;
            }
        } else {
            self.from = found.end();
        }
        self.last_match_end = Some(found.end());
        false
    }
}

enum Reference<'a> {
    Index(usize),
    Name(&'a str),
}

pub(crate) fn interpolate<A, N>(
    mut replacement: &[u8],
    mut append: A,
    mut name_to_index: N,
    destination: &mut Vec<u8>,
) where
    A: FnMut(usize, &mut Vec<u8>),
    N: FnMut(&str) -> Option<usize>,
{
    while !replacement.is_empty() {
        let Some(dollar) = memchr::memchr(b'$', replacement) else {
            break;
        };
        destination.extend_from_slice(&replacement[..dollar]);
        replacement = &replacement[dollar..];
        if replacement.get(1) == Some(&b'$') {
            destination.push(b'$');
            replacement = &replacement[2..];
            continue;
        }
        let Some((reference, consumed)) = find_reference(replacement) else {
            destination.push(b'$');
            replacement = &replacement[1..];
            continue;
        };
        replacement = &replacement[consumed..];
        match reference {
            Reference::Index(index) => append(index, destination),
            Reference::Name(name) => {
                if let Some(index) = name_to_index(name) {
                    append(index, destination);
                }
            }
        }
    }
    destination.extend_from_slice(replacement);
}

fn find_reference(replacement: &[u8]) -> Option<(Reference<'_>, usize)> {
    assert_eq!(replacement.first(), Some(&b'$'));
    let braced = replacement.get(1) == Some(&b'{');
    let name_start = if braced { 2 } else { 1 };
    let name_length = replacement[name_start..]
        .iter()
        .take_while(|&&b| is_capture_name_byte(b))
        .count();
    if name_length == 0 {
        return None;
    }
    let name_end = name_start + name_length;
    let name = std::str::from_utf8(&replacement[name_start..name_end])
        .expect("capture name bytes are ASCII");
    let mut end = name_end;
    if braced {
        if replacement.get(name_end) != Some(&b'}') {
            return None;
        }
        end += 1;
    }
    let reference = match name.parse::<u32>() {
        Ok(index) => Reference::Index(usize::try_from(index).expect("a u32 fits in usize")),
        Err(_) => Reference::Name(name),
    };
    Some((reference, end))
}

fn is_capture_name_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}
