use std::io;

use super::core::Core;
use super::line_buffer::{BUFFER_CAPACITY_BYTES, BinaryDetectionMode, LineBuffer};
use super::lines::{self, LineStep};
use super::{Searcher, Sink};
use crate::matcher::{Match, Matcher};

pub(crate) fn search_by_line<M, R, S>(
    searcher: &Searcher,
    matcher: M,
    reader: &mut R,
    buffer: &mut LineBuffer,
    sink: S,
) -> io::Result<()>
where
    M: Matcher,
    R: io::Read + ?Sized,
    S: Sink,
{
    let mut core = Core::new(searcher, matcher, sink, false);
    if core.begin()? {
        while fill(searcher, &mut core, reader, buffer)? {
            if !core.match_by_line(buffer.buffer())? {
                buffer.consume(core.position());
                break;
            }
        }
    }
    core.finish(buffer.absolute_byte_offset(), buffer.binary_byte_offset())
}

fn fill<M, R, S>(
    searcher: &Searcher,
    core: &mut Core<'_, M, S>,
    reader: &mut R,
    buffer: &mut LineBuffer,
) -> io::Result<bool>
where
    M: Matcher,
    R: io::Read + ?Sized,
    S: Sink,
{
    assert_eq!(
        core.position(),
        buffer.buffer().len(),
        "the buffer was not searched to its end"
    );
    let already_binary = buffer.binary_byte_offset().is_some();
    let old_length = buffer.buffer().len();
    let consumed = core.roll(buffer.buffer());
    buffer.consume(consumed);
    let did_read = buffer.fill(reader)?;
    if !already_binary
        && let Some(offset) = buffer.binary_byte_offset()
        && !core.binary_data(offset)?
    {
        return Ok(false);
    }
    let quits_on_binary = matches!(searcher.config.binary.0, BinaryDetectionMode::Quit(_));
    if !did_read || (buffer.binary_byte_offset().is_some() && quits_on_binary) {
        return Ok(false);
    }
    if consumed == 0 && old_length == buffer.buffer().len() {
        buffer.consume(old_length);
        return Ok(false);
    }
    Ok(true)
}

pub(crate) fn search_multi_line<M: Matcher, S: Sink>(
    searcher: &Searcher,
    matcher: M,
    slice: &[u8],
    sink: S,
) -> io::Result<()> {
    let mut search = MultiLine {
        searcher,
        core: Core::new(searcher, matcher, sink, true),
        slice,
        last_match: None,
    };
    if search.core.begin()? {
        let binary_range = Match::new(0, slice.len().min(BUFFER_CAPACITY_BYTES));
        if !search.core.detect_binary(slice, binary_range)? {
            search.run()?;
        }
    }
    let byte_count = search.byte_count();
    let binary_byte_offset = search.core.binary_byte_offset();
    search.core.finish(byte_count, binary_byte_offset)
}

struct MultiLine<'s, M, S> {
    searcher: &'s Searcher,
    core: Core<'s, M, S>,
    slice: &'s [u8],
    last_match: Option<Match>,
}

impl<M: Matcher, S: Sink> MultiLine<'_, M, S> {
    fn run(&mut self) -> io::Result<()> {
        while self.core.position() < self.slice.len() {
            if !self.sink()? {
                return Ok(());
            }
        }
        if let Some(last_match) = self.last_match.take()
            && self.sink_context(last_match)?
        {
            self.sink_matched(last_match)?;
        }
        if self.searcher.config.passthru {
            self.core
                .other_context_by_line(self.slice, self.slice.len())?;
        } else {
            self.core
                .after_context_by_line(self.slice, self.slice.len())?;
        }
        Ok(())
    }

    fn line_terminator_byte(&self) -> u8 {
        self.searcher.config.line_terminator.as_byte()
    }

    fn sink(&mut self) -> io::Result<bool> {
        if self.searcher.config.invert_match {
            return self.sink_matched_inverted();
        }
        let Some(found) = self.find() else {
            self.core.set_position(self.slice.len());
            return Ok(true);
        };
        self.advance(found);
        let line = lines::locate(self.slice, self.line_terminator_byte(), found);
        let Some(last_match) = self.last_match.take() else {
            self.last_match = Some(line);
            return Ok(true);
        };
        if last_match.end() >= line.start() {
            self.last_match = Some(last_match.with_end(line.end()));
            return Ok(true);
        }
        self.last_match = Some(line);
        if !self.sink_context(last_match)? {
            return Ok(false);
        }
        self.sink_matched(last_match)
    }

    fn sink_matched_inverted(&mut self) -> io::Result<bool> {
        let inverted = match self.find() {
            None => {
                let range = Match::new(self.core.position(), self.slice.len());
                self.core.set_position(range.end());
                range
            }
            Some(found) => {
                let line = lines::locate(self.slice, self.line_terminator_byte(), found);
                let range = Match::new(self.core.position(), line.start());
                self.advance(line);
                range
            }
        };
        if inverted.is_empty() {
            return Ok(true);
        }
        if !self.sink_context(inverted)? {
            return Ok(false);
        }
        let mut step = LineStep::new(
            self.line_terminator_byte(),
            inverted.start(),
            inverted.end(),
        );
        while let Some(line) = step.next_match(self.slice) {
            if !self.sink_matched(line)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn sink_matched(&mut self, range: Match) -> io::Result<bool> {
        if range.is_empty() {
            return Ok(false);
        }
        self.core.sink_matched(self.slice, range)
    }

    fn sink_context(&mut self, range: Match) -> io::Result<bool> {
        if self.searcher.config.passthru {
            return self.core.other_context_by_line(self.slice, range.start());
        }
        if !self.core.after_context_by_line(self.slice, range.start())? {
            return Ok(false);
        }
        self.core.before_context_by_line(self.slice, range.start())
    }

    fn find(&mut self) -> Option<Match> {
        let position = self.core.position();
        self.core
            .find(&self.slice[position..])
            .map(|found| found.offset(position))
    }

    fn advance(&mut self, range: Match) {
        self.core.set_position(range.end());
        if range.is_empty() && self.core.position() < self.slice.len() {
            self.core.set_position(self.core.position() + 1);
        }
    }

    fn byte_count(&self) -> u64 {
        let position = self.core.position() as u64;
        match self.core.binary_byte_offset() {
            Some(offset) if offset < position => offset,
            _ => position,
        }
    }
}
