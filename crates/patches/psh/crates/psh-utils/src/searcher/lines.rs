use crate::matcher::{LineTerminator, Match};

#[derive(Debug)]
pub(crate) struct LineIter<'b> {
    bytes: &'b [u8],
    step: LineStep,
}

impl<'b> LineIter<'b> {
    pub(crate) fn new(line_terminator: u8, bytes: &'b [u8]) -> LineIter<'b> {
        LineIter {
            bytes,
            step: LineStep::new(line_terminator, 0, bytes.len()),
        }
    }
}

impl<'b> Iterator for LineIter<'b> {
    type Item = &'b [u8];

    fn next(&mut self) -> Option<&'b [u8]> {
        self.step
            .next_match(self.bytes)
            .map(|line| &self.bytes[line])
    }
}

#[derive(Debug)]
pub(crate) struct LineStep {
    line_terminator: u8,
    position: usize,
    end: usize,
}

impl LineStep {
    pub(crate) fn new(line_terminator: u8, start: usize, end: usize) -> LineStep {
        assert!(
            start <= end,
            "line step start {start} is past its end {end}"
        );
        LineStep {
            line_terminator,
            position: start,
            end,
        }
    }

    #[inline(always)]
    pub(crate) fn next_match(&mut self, bytes: &[u8]) -> Option<Match> {
        let bytes = &bytes[..self.end];
        let line = match memchr::memchr(self.line_terminator, &bytes[self.position..]) {
            Some(terminator) => Match::new(self.position, self.position + terminator + 1),
            None if self.position < bytes.len() => Match::new(self.position, bytes.len()),
            None => return None,
        };
        self.position = line.end();
        Some(line)
    }
}

pub(crate) fn count(bytes: &[u8], line_terminator: u8) -> u64 {
    let count = memchr::memchr_iter(line_terminator, bytes).count();
    u64::try_from(count).expect("a line count fits in u64")
}

#[inline(always)]
pub(crate) fn without_terminator(bytes: &[u8], line_terminator: LineTerminator) -> &[u8] {
    bytes
        .strip_suffix(line_terminator.as_bytes())
        .unwrap_or(bytes)
}

#[inline(always)]
pub(crate) fn locate(bytes: &[u8], line_terminator: u8, range: Match) -> Match {
    let line_start = memchr::memrchr(line_terminator, &bytes[..range.start()]).map_or(0, |i| i + 1);
    let line_end = if range.end() > line_start && bytes[range.end() - 1] == line_terminator {
        range.end()
    } else {
        memchr::memchr(line_terminator, &bytes[range.end()..])
            .map_or(bytes.len(), |i| range.end() + i + 1)
    };
    Match::new(line_start, line_end)
}

pub(crate) fn preceding(bytes: &[u8], line_terminator: u8, count: usize) -> usize {
    let mut position = bytes.len();
    if position == 0 {
        return 0;
    }
    if bytes[position - 1] == line_terminator {
        position -= 1;
    }
    let mut left = count;
    while let Some(terminator) = memchr::memrchr(line_terminator, &bytes[..position]) {
        if left == 0 {
            return terminator + 1;
        }
        if terminator == 0 {
            return 0;
        }
        left -= 1;
        position = terminator;
    }
    0
}
