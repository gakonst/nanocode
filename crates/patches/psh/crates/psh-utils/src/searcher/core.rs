use std::io;

use super::line_buffer::BinaryDetectionMode;
use super::lines::{self, LineStep};
use super::{Searcher, Sink, SinkContext, SinkContextKind, SinkFinish, SinkMatch};
use crate::matcher::{LineMatchKind, Match, Matcher};

enum FastMatch {
    Continue,
    Stop,
    SwitchToSlow,
}

pub(crate) struct Core<'s, M, S> {
    searcher: &'s Searcher,
    matcher: M,
    sink: S,
    detects_binary_per_line: bool,
    position: usize,
    absolute_byte_offset: u64,
    binary_byte_offset: Option<usize>,
    line_number: Option<u64>,
    last_line_counted: usize,
    last_line_visited: usize,
    after_context_left: usize,
    has_sunk: bool,
    has_matched: bool,
    count: u64,
}

impl<'s, M: Matcher, S: Sink> Core<'s, M, S> {
    pub(crate) fn new(
        searcher: &'s Searcher,
        matcher: M,
        sink: S,
        detects_binary_per_line: bool,
    ) -> Core<'s, M, S> {
        let line_number = if searcher.config.line_number {
            Some(1)
        } else {
            None
        };
        Core {
            searcher,
            matcher,
            sink,
            detects_binary_per_line,
            position: 0,
            absolute_byte_offset: 0,
            binary_byte_offset: None,
            line_number,
            last_line_counted: 0,
            last_line_visited: 0,
            after_context_left: 0,
            has_sunk: false,
            has_matched: false,
            count: 0,
        }
    }

    pub(crate) fn position(&self) -> usize {
        self.position
    }

    pub(crate) fn set_position(&mut self, position: usize) {
        self.position = position;
    }

    pub(crate) fn binary_byte_offset(&self) -> Option<u64> {
        self.binary_byte_offset.map(|offset| offset as u64)
    }

    pub(crate) fn binary_data(&mut self, binary_byte_offset: u64) -> io::Result<bool> {
        self.sink.binary_data(self.searcher, binary_byte_offset)
    }

    pub(crate) fn begin(&mut self) -> io::Result<bool> {
        self.sink.begin(self.searcher)
    }

    pub(crate) fn finish(
        &mut self,
        byte_count: u64,
        binary_byte_offset: Option<u64>,
    ) -> io::Result<()> {
        self.sink.finish(
            self.searcher,
            &SinkFinish {
                byte_count,
                binary_byte_offset,
            },
        )
    }

    fn line_terminator_byte(&self) -> u8 {
        self.searcher.config.line_terminator.as_byte()
    }

    fn is_match(&self, line: &[u8]) -> bool {
        let line = lines::without_terminator(line, self.searcher.config.line_terminator);
        self.matcher.is_match(line)
    }

    pub(crate) fn find(&mut self, slice: &[u8]) -> Option<Match> {
        if self.has_exceeded_match_limit() {
            return None;
        }
        let found = self.matcher.find(slice)?;
        self.count += 1;
        Some(found)
    }

    fn shortest_match(&self, slice: &[u8]) -> Option<usize> {
        if self.has_exceeded_match_limit() {
            return None;
        }
        self.matcher.shortest_match(slice)
    }

    pub(crate) fn match_by_line(&mut self, buffer: &[u8]) -> io::Result<bool> {
        if !self.is_line_by_line_fast() {
            return self.match_by_line_slow(buffer);
        }
        match self.match_by_line_fast(buffer)? {
            FastMatch::SwitchToSlow => self.match_by_line_slow(buffer),
            FastMatch::Continue => Ok(true),
            FastMatch::Stop => Ok(false),
        }
    }

    pub(crate) fn roll(&mut self, buffer: &[u8]) -> usize {
        let config = &self.searcher.config;
        let consumed = if config.max_context() == 0 {
            buffer.len()
        } else {
            let context_start =
                lines::preceding(buffer, self.line_terminator_byte(), config.before_context);
            context_start.max(self.last_line_visited)
        };
        self.count_lines(buffer, consumed);
        self.absolute_byte_offset += consumed as u64;
        self.last_line_counted = 0;
        self.last_line_visited = 0;
        self.set_position(buffer.len() - consumed);
        consumed
    }

    pub(crate) fn detect_binary(&mut self, buffer: &[u8], range: Match) -> io::Result<bool> {
        let binary = self.searcher.config.binary.0;
        if self.binary_byte_offset.is_some() {
            return Ok(matches!(binary, BinaryDetectionMode::Quit(_)));
        }
        let binary_byte = match binary {
            BinaryDetectionMode::Quit(byte) | BinaryDetectionMode::Convert(byte) => byte,
            BinaryDetectionMode::None => return Ok(false),
        };
        let Some(found) = memchr::memchr(binary_byte, &buffer[range]) else {
            return Ok(false);
        };
        let offset = range.start() + found;
        self.binary_byte_offset = Some(offset);
        if !self.binary_data(offset as u64)? {
            return Ok(true);
        }
        Ok(matches!(binary, BinaryDetectionMode::Quit(_)))
    }

    pub(crate) fn before_context_by_line(
        &mut self,
        buffer: &[u8],
        upto: usize,
    ) -> io::Result<bool> {
        let before_context = self.searcher.config.before_context;
        if before_context == 0 {
            return Ok(true);
        }
        let range = Match::new(self.last_line_visited, upto);
        if range.is_empty() {
            return Ok(true);
        }
        let start = range.start()
            + lines::preceding(
                &buffer[range],
                self.line_terminator_byte(),
                before_context - 1,
            );
        let mut step = LineStep::new(self.line_terminator_byte(), start, range.end());
        while let Some(line) = step.next_match(buffer) {
            if !self.sink_break_context(line.start())? {
                return Ok(false);
            }
            if !self.sink_context(buffer, line, SinkContextKind::Before)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn after_context_by_line(&mut self, buffer: &[u8], upto: usize) -> io::Result<bool> {
        if self.after_context_left == 0 {
            return Ok(true);
        }
        let exceeded_match_limit = self.has_exceeded_match_limit();
        let invert_match = self.searcher.config.invert_match;
        let mut step = LineStep::new(self.line_terminator_byte(), self.last_line_visited, upto);
        while let Some(line) = step.next_match(buffer) {
            if exceeded_match_limit && self.is_match(&buffer[line]) != invert_match {
                let after_context_left = self.after_context_left;
                self.set_position(line.end());
                if !self.sink_matched(buffer, line)? {
                    return Ok(false);
                }
                self.after_context_left = after_context_left - 1;
            } else if !self.sink_context(buffer, line, SinkContextKind::After)? {
                return Ok(false);
            }
            if self.after_context_left == 0 {
                break;
            }
        }
        Ok(true)
    }

    pub(crate) fn other_context_by_line(&mut self, buffer: &[u8], upto: usize) -> io::Result<bool> {
        let mut step = LineStep::new(self.line_terminator_byte(), self.last_line_visited, upto);
        while let Some(line) = step.next_match(buffer) {
            if !self.sink_context(buffer, line, SinkContextKind::Other)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn match_by_line_slow(&mut self, buffer: &[u8]) -> io::Result<bool> {
        let config = &self.searcher.config;
        let mut step = LineStep::new(self.line_terminator_byte(), self.position(), buffer.len());
        while let Some(line) = step.next_match(buffer) {
            if self.has_exceeded_match_limit() && !config.passthru && self.after_context_left == 0 {
                return Ok(false);
            }
            let slice = lines::without_terminator(&buffer[line], config.line_terminator);
            let matched = self.shortest_match(slice).is_some();
            self.set_position(line.end());
            let success = matched != config.invert_match;
            if success {
                self.has_matched = true;
                self.count += 1;
                if !self.before_context_by_line(buffer, line.start())? {
                    return Ok(false);
                }
                if !self.sink_matched(buffer, line)? {
                    return Ok(false);
                }
            } else if self.after_context_left >= 1 {
                if !self.sink_context(buffer, line, SinkContextKind::After)? {
                    return Ok(false);
                }
            } else if config.passthru && !self.sink_context(buffer, line, SinkContextKind::Other)? {
                return Ok(false);
            }
            if config.stop_on_nonmatch && !success && self.has_matched {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn match_by_line_fast(&mut self, buffer: &[u8]) -> io::Result<FastMatch> {
        let config = &self.searcher.config;
        assert!(!config.passthru);
        while self.position() < buffer.len() {
            if config.stop_on_nonmatch && self.has_matched {
                return Ok(FastMatch::SwitchToSlow);
            }
            if config.invert_match {
                if !self.match_by_line_fast_invert(buffer)? {
                    break;
                }
                continue;
            }
            let Some(line) = self.find_by_line_fast(buffer) else {
                break;
            };
            self.has_matched = true;
            self.count += 1;
            if config.max_context() > 0 {
                if !self.after_context_by_line(buffer, line.start())? {
                    return Ok(FastMatch::Stop);
                }
                if !self.before_context_by_line(buffer, line.start())? {
                    return Ok(FastMatch::Stop);
                }
            }
            self.set_position(line.end());
            if !self.sink_matched(buffer, line)? {
                return Ok(FastMatch::Stop);
            }
        }
        if !self.after_context_by_line(buffer, buffer.len())? {
            return Ok(FastMatch::Stop);
        }
        if self.has_exceeded_match_limit() && self.after_context_left == 0 {
            return Ok(FastMatch::Stop);
        }
        self.set_position(buffer.len());
        Ok(FastMatch::Continue)
    }

    #[inline(always)]
    fn match_by_line_fast_invert(&mut self, buffer: &[u8]) -> io::Result<bool> {
        let inverted = match self.find_by_line_fast(buffer) {
            None => {
                let range = Match::new(self.position(), buffer.len());
                self.set_position(range.end());
                range
            }
            Some(line) => {
                let range = Match::new(self.position(), line.start());
                self.set_position(line.end());
                range
            }
        };
        if inverted.is_empty() {
            return Ok(true);
        }
        self.has_matched = true;
        if !self.after_context_by_line(buffer, inverted.start())? {
            return Ok(false);
        }
        if !self.before_context_by_line(buffer, inverted.start())? {
            return Ok(false);
        }
        let mut step = LineStep::new(
            self.line_terminator_byte(),
            inverted.start(),
            inverted.end(),
        );
        while let Some(line) = step.next_match(buffer) {
            self.count += 1;
            if !self.sink_matched(buffer, line)? {
                return Ok(false);
            }
            if self.has_exceeded_match_limit() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    #[inline(always)]
    fn find_by_line_fast(&self, buffer: &[u8]) -> Option<Match> {
        let line_terminator = self.line_terminator_byte();
        let mut position = self.position();
        while position < buffer.len() {
            if self.has_exceeded_match_limit() {
                return None;
            }
            match self.matcher.find_candidate_line(&buffer[position..])? {
                LineMatchKind::Confirmed(found) => {
                    let line =
                        lines::locate(buffer, line_terminator, Match::zero(found).offset(position));
                    if line.start() == buffer.len() {
                        position = buffer.len();
                        continue;
                    }
                    return Some(line);
                }
                LineMatchKind::Candidate(found) => {
                    let line =
                        lines::locate(buffer, line_terminator, Match::zero(found).offset(position));
                    if self.is_match(&buffer[line]) {
                        return Some(line);
                    }
                    position = line.end();
                }
            }
        }
        None
    }

    #[inline(always)]
    pub(crate) fn sink_matched(&mut self, buffer: &[u8], range: Match) -> io::Result<bool> {
        if self.detects_binary_per_line && self.detect_binary(buffer, range)? {
            return Ok(false);
        }
        if !self.sink_break_context(range.start())? {
            return Ok(false);
        }
        self.count_lines(buffer, range.start());
        let sink_match = SinkMatch {
            line_terminator: self.searcher.config.line_terminator,
            bytes: &buffer[range],
            absolute_byte_offset: self.absolute_byte_offset + range.start() as u64,
            line_number: self.line_number,
            buffer,
            bytes_range_in_buffer: range.start()..range.end(),
        };
        if !self.sink.matched(self.searcher, &sink_match)? {
            return Ok(false);
        }
        self.last_line_visited = range.end();
        self.after_context_left = self.searcher.config.after_context;
        self.has_sunk = true;
        Ok(true)
    }

    fn sink_context(
        &mut self,
        buffer: &[u8],
        range: Match,
        kind: SinkContextKind,
    ) -> io::Result<bool> {
        if kind == SinkContextKind::After {
            assert!(
                self.after_context_left >= 1,
                "after context sunk with none left"
            );
        }
        if self.detects_binary_per_line && self.detect_binary(buffer, range)? {
            return Ok(false);
        }
        self.count_lines(buffer, range.start());
        let context = SinkContext {
            bytes: &buffer[range],
            kind,
            absolute_byte_offset: self.absolute_byte_offset + range.start() as u64,
            line_number: self.line_number,
        };
        if !self.sink.context(self.searcher, &context)? {
            return Ok(false);
        }
        self.last_line_visited = range.end();
        if kind == SinkContextKind::After {
            self.after_context_left -= 1;
        }
        self.has_sunk = true;
        Ok(true)
    }

    fn sink_break_context(&mut self, start_of_line: usize) -> io::Result<bool> {
        let config = &self.searcher.config;
        let is_gap = self.last_line_visited < start_of_line;
        let any_context = config.before_context > 0 || config.after_context > 0;
        if !any_context || !self.has_sunk || !is_gap {
            return Ok(true);
        }
        self.sink.context_break(self.searcher)
    }

    fn count_lines(&mut self, buffer: &[u8], upto: usize) {
        let Some(line_number) = self.line_number.as_mut() else {
            return;
        };
        if self.last_line_counted >= upto {
            return;
        }
        *line_number += lines::count(
            &buffer[self.last_line_counted..upto],
            self.searcher.config.line_terminator.as_byte(),
        );
        self.last_line_counted = upto;
    }

    fn is_line_by_line_fast(&self) -> bool {
        let config = &self.searcher.config;
        if config.passthru {
            return false;
        }
        if config.stop_on_nonmatch && self.has_matched {
            return false;
        }
        if let Some(line_terminator) = self.matcher.line_terminator() {
            if line_terminator.as_byte() == b'\x00' {
                return false;
            }
            if line_terminator == config.line_terminator {
                return true;
            }
        }
        self.matcher
            .non_matching_bytes()
            .is_some_and(|non_matching| non_matching.contains(config.line_terminator.as_byte()))
    }

    fn has_exceeded_match_limit(&self) -> bool {
        self.searcher
            .config
            .max_matches
            .is_some_and(|limit| self.count >= limit)
    }
}
