use std::cell::{Cell, RefCell};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;

use super::color::ColorSpecs;
use super::hyperlink::{self, HyperlinkConfig, InterpolatorStatus};
use super::stats::{Started, Stats};
use super::util::{
    CounterWriter, DecimalFormatter, PrinterPath, Replacer, Sunk, find_iter_at_in_context,
    trim_ascii_prefix, trim_line_terminator,
};
use crate::ansi::{ColorSpec, WriteColor};
use crate::matcher::{Match, Matcher};
use crate::searcher::{LineStep, Searcher, Sink, SinkContext, SinkFinish, SinkMatch};

#[derive(Clone, Debug)]
struct Config {
    colors: ColorSpecs,
    hyperlink: HyperlinkConfig,
    stats: bool,
    heading: bool,
    path: bool,
    only_matching: bool,
    per_match: bool,
    per_match_one_line: bool,
    replacement: Arc<Option<Vec<u8>>>,
    max_columns: Option<u64>,
    max_columns_preview: bool,
    column: bool,
    byte_offset: bool,
    trim_ascii: bool,
    separator_search: Arc<Option<Vec<u8>>>,
    separator_context: Arc<Option<Vec<u8>>>,
    separator_field_match: Arc<Vec<u8>>,
    separator_field_context: Arc<Vec<u8>>,
    separator_path: Option<u8>,
    path_terminator: Option<u8>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            colors: ColorSpecs::default(),
            hyperlink: HyperlinkConfig::default(),
            stats: false,
            heading: false,
            path: true,
            only_matching: false,
            per_match: false,
            per_match_one_line: false,
            replacement: Arc::new(None),
            max_columns: None,
            max_columns_preview: false,
            column: false,
            byte_offset: false,
            trim_ascii: false,
            separator_search: Arc::new(None),
            separator_context: Arc::new(Some(b"--".to_vec())),
            separator_field_match: Arc::new(b":".to_vec()),
            separator_field_context: Arc::new(b"-".to_vec()),
            separator_path: None,
            path_terminator: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct StandardBuilder {
    config: Config,
}

impl StandardBuilder {
    #[inline(always)]
    pub(crate) fn new() -> StandardBuilder {
        StandardBuilder::default()
    }

    pub(crate) fn build<W: WriteColor>(&self, writer: W) -> Standard<W> {
        Standard {
            config: self.config.clone(),
            writer: RefCell::new(CounterWriter::new(writer)),
            matches: Vec::new(),
        }
    }

    pub(crate) fn color_specs(&mut self, specs: ColorSpecs) -> &mut StandardBuilder {
        self.config.colors = specs;
        self
    }

    pub(crate) fn hyperlink(&mut self, config: HyperlinkConfig) -> &mut StandardBuilder {
        self.config.hyperlink = config;
        self
    }

    pub(crate) fn stats(&mut self, yes: bool) -> &mut StandardBuilder {
        self.config.stats = yes;
        self
    }

    pub(crate) fn heading(&mut self, yes: bool) -> &mut StandardBuilder {
        self.config.heading = yes;
        self
    }

    pub(crate) fn path(&mut self, yes: bool) -> &mut StandardBuilder {
        self.config.path = yes;
        self
    }

    pub(crate) fn only_matching(&mut self, yes: bool) -> &mut StandardBuilder {
        self.config.only_matching = yes;
        self
    }

    pub(crate) fn per_match(&mut self, yes: bool) -> &mut StandardBuilder {
        self.config.per_match = yes;
        self
    }

    pub(crate) fn per_match_one_line(&mut self, yes: bool) -> &mut StandardBuilder {
        self.config.per_match_one_line = yes;
        self
    }

    pub(crate) fn replacement(&mut self, replacement: Option<Vec<u8>>) -> &mut StandardBuilder {
        self.config.replacement = Arc::new(replacement);
        self
    }

    pub(crate) fn max_columns(&mut self, limit: Option<u64>) -> &mut StandardBuilder {
        self.config.max_columns = limit;
        self
    }

    pub(crate) fn max_columns_preview(&mut self, yes: bool) -> &mut StandardBuilder {
        self.config.max_columns_preview = yes;
        self
    }

    pub(crate) fn column(&mut self, yes: bool) -> &mut StandardBuilder {
        self.config.column = yes;
        self
    }

    pub(crate) fn byte_offset(&mut self, yes: bool) -> &mut StandardBuilder {
        self.config.byte_offset = yes;
        self
    }

    pub(crate) fn trim_ascii(&mut self, yes: bool) -> &mut StandardBuilder {
        self.config.trim_ascii = yes;
        self
    }

    pub(crate) fn separator_search(&mut self, separator: Option<Vec<u8>>) -> &mut StandardBuilder {
        self.config.separator_search = Arc::new(separator);
        self
    }

    pub(crate) fn separator_context(&mut self, separator: Option<Vec<u8>>) -> &mut StandardBuilder {
        self.config.separator_context = Arc::new(separator);
        self
    }

    pub(crate) fn separator_field_match(&mut self, separator: Vec<u8>) -> &mut StandardBuilder {
        self.config.separator_field_match = Arc::new(separator);
        self
    }

    pub(crate) fn separator_field_context(&mut self, separator: Vec<u8>) -> &mut StandardBuilder {
        self.config.separator_field_context = Arc::new(separator);
        self
    }

    pub(crate) fn separator_path(&mut self, separator: Option<u8>) -> &mut StandardBuilder {
        self.config.separator_path = separator;
        self
    }

    pub(crate) fn path_terminator(&mut self, terminator: Option<u8>) -> &mut StandardBuilder {
        self.config.path_terminator = terminator;
        self
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Standard<W> {
    config: Config,
    writer: RefCell<CounterWriter<W>>,
    matches: Vec<Match>,
}

impl<W: WriteColor> Standard<W> {
    pub(crate) fn sink_with_path<'p, 's, M: Matcher>(
        &'s mut self,
        matcher: M,
        path: &'p Path,
    ) -> StandardSink<'p, 's, M, W> {
        let path = self
            .config
            .path
            .then(|| PrinterPath::new(path).with_separator(self.config.separator_path));
        let interpolator = hyperlink::Interpolator::new(&self.config.hyperlink);
        let stats = self.config.stats.then(Stats::new);
        let needs_match_granularity = self.needs_match_granularity();
        StandardSink {
            matcher,
            standard: self,
            replacer: Replacer::new(),
            interpolator,
            path,
            start_time: Started::now(),
            match_count: 0,
            binary_byte_offset: None,
            stats,
            needs_match_granularity,
        }
    }

    fn needs_match_granularity(&self) -> bool {
        let supports_color = self.writer.borrow().supports_color();
        let match_colored = !self.config.colors.matched().is_none();
        (supports_color && match_colored)
            || self.config.column
            || self.config.replacement.is_some()
            || self.config.per_match
            || self.config.only_matching
            || self.config.stats
    }
}

impl<W> Standard<W> {
    pub(crate) fn get_mut(&mut self) -> &mut W {
        self.writer.get_mut().get_mut()
    }
}

#[derive(Debug)]
pub(crate) struct StandardSink<'p, 's, M: Matcher, W> {
    matcher: M,
    standard: &'s mut Standard<W>,
    replacer: Replacer<M>,
    interpolator: hyperlink::Interpolator,
    path: Option<PrinterPath<'p>>,
    start_time: Started,
    match_count: u64,
    binary_byte_offset: Option<u64>,
    stats: Option<Stats>,
    needs_match_granularity: bool,
}

impl<M: Matcher, W: WriteColor> StandardSink<'_, '_, M, W> {
    pub(crate) fn has_match(&self) -> bool {
        self.match_count > 0
    }

    pub(crate) fn stats(&self) -> Option<&Stats> {
        self.stats.as_ref()
    }

    fn record_matches(&mut self, searcher: &Searcher, bytes: &[u8], range: std::ops::Range<usize>) {
        self.standard.matches.clear();
        if !self.needs_match_granularity {
            return;
        }
        let matches = &mut self.standard.matches;
        let start = range.start;
        let end = range.end;
        find_iter_at_in_context(searcher, &self.matcher, bytes, range, |found| {
            matches.push(Match::new(found.start() - start, found.end() - start));
            true
        });
        if matches
            .last()
            .is_some_and(|last| last.is_empty() && last.start() >= end)
        {
            matches.pop();
        }
    }

    fn replace(&mut self, searcher: &Searcher, bytes: &[u8], range: std::ops::Range<usize>) {
        self.replacer.clear();
        if let Some(replacement) = self.standard.config.replacement.as_ref() {
            self.replacer
                .replace_all(searcher, &self.matcher, bytes, range, replacement);
        }
    }
}

impl<M: Matcher, W: WriteColor> Sink for StandardSink<'_, '_, M, W> {
    fn matched(&mut self, searcher: &Searcher, sink_match: &SinkMatch<'_>) -> io::Result<bool> {
        self.match_count += 1;
        self.record_matches(
            searcher,
            sink_match.buffer(),
            sink_match.bytes_range_in_buffer(),
        );
        self.replace(
            searcher,
            sink_match.buffer(),
            sink_match.bytes_range_in_buffer(),
        );
        if let Some(stats) = self.stats.as_mut() {
            stats.add_matches(self.standard.matches.len() as u64);
            stats.add_matched_lines(sink_match.lines().count() as u64);
        }
        if searcher.binary_detection().convert_byte().is_some() && self.binary_byte_offset.is_some()
        {
            return Ok(false);
        }
        StandardImpl::from_match(searcher, self, sink_match).sink()?;
        Ok(true)
    }

    fn context(&mut self, searcher: &Searcher, context: &SinkContext<'_>) -> io::Result<bool> {
        self.standard.matches.clear();
        self.replacer.clear();
        if searcher.invert_match() {
            self.record_matches(searcher, context.bytes(), 0..context.bytes().len());
            self.replace(searcher, context.bytes(), 0..context.bytes().len());
        }
        if searcher.binary_detection().convert_byte().is_some() && self.binary_byte_offset.is_some()
        {
            return Ok(false);
        }
        StandardImpl::from_context(searcher, self, context).sink()?;
        Ok(true)
    }

    fn context_break(&mut self, searcher: &Searcher) -> io::Result<bool> {
        StandardImpl::new(searcher, self).write_context_separator()?;
        Ok(true)
    }

    fn binary_data(&mut self, _searcher: &Searcher, binary_byte_offset: u64) -> io::Result<bool> {
        self.binary_byte_offset = Some(binary_byte_offset);
        Ok(true)
    }

    fn begin(&mut self, _searcher: &Searcher) -> io::Result<bool> {
        self.standard.writer.borrow_mut().reset_count();
        self.start_time = Started::now();
        self.match_count = 0;
        self.binary_byte_offset = None;
        Ok(true)
    }

    fn finish(&mut self, searcher: &Searcher, finish: &SinkFinish) -> io::Result<()> {
        if let Some(offset) = self.binary_byte_offset {
            StandardImpl::new(searcher, self).write_binary_message(offset)?;
        }
        let bytes_printed = self.standard.writer.borrow().count();
        if let Some(stats) = self.stats.as_mut() {
            stats.record_search(
                self.start_time,
                self.match_count > 0,
                finish.byte_count(),
                bytes_printed,
            );
        }
        Ok(())
    }
}

struct StandardImpl<'a, M: Matcher, W> {
    searcher: &'a Searcher,
    sink: &'a StandardSink<'a, 'a, M, W>,
    sunk: Sunk<'a>,
    in_color_match: Cell<bool>,
}

impl<'a, M: Matcher, W: WriteColor> StandardImpl<'a, M, W> {
    #[inline(always)]
    fn new(searcher: &'a Searcher, sink: &'a StandardSink<'_, '_, M, W>) -> StandardImpl<'a, M, W> {
        StandardImpl {
            searcher,
            sink,
            sunk: Sunk::empty(),
            in_color_match: Cell::new(false),
        }
    }

    fn from_match(
        searcher: &'a Searcher,
        sink: &'a StandardSink<'_, '_, M, W>,
        sink_match: &'a SinkMatch<'a>,
    ) -> StandardImpl<'a, M, W> {
        let sunk = Sunk::from_sink_match(
            sink_match,
            &sink.standard.matches,
            sink.replacer.replacement(),
        );
        StandardImpl {
            sunk,
            ..StandardImpl::new(searcher, sink)
        }
    }

    fn from_context(
        searcher: &'a Searcher,
        sink: &'a StandardSink<'_, '_, M, W>,
        context: &'a SinkContext<'a>,
    ) -> StandardImpl<'a, M, W> {
        let sunk =
            Sunk::from_sink_context(context, &sink.standard.matches, sink.replacer.replacement());
        StandardImpl {
            sunk,
            ..StandardImpl::new(searcher, sink)
        }
    }

    fn sink(&self) -> io::Result<()> {
        self.write_search_prelude()?;
        let multi_line_match = self.multi_line() && !self.is_context();
        match (self.sunk.matches().is_empty(), multi_line_match) {
            (true, true) => self.sink_fast_multi_line(),
            (true, false) => self.sink_fast(),
            (false, true) => self.sink_slow_multi_line(),
            (false, false) => self.sink_slow(),
        }
    }

    fn sink_fast(&self) -> io::Result<()> {
        self.write_prelude(
            self.sunk.absolute_byte_offset(),
            self.sunk.line_number(),
            None,
        )?;
        self.write_line(self.sunk.bytes())
    }

    fn sink_fast_multi_line(&self) -> io::Result<()> {
        let line_terminator = self.searcher.line_terminator().as_byte();
        let mut absolute_byte_offset = self.sunk.absolute_byte_offset();
        for (index, line) in self.sunk.lines(line_terminator).enumerate() {
            let line_number = self.sunk.line_number().map(|number| number + index as u64);
            self.write_prelude(absolute_byte_offset, line_number, None)?;
            absolute_byte_offset += line.len() as u64;
            self.write_line(line)?;
        }
        Ok(())
    }

    fn sink_slow(&self) -> io::Result<()> {
        let matches = self.sunk.matches();
        if self.config().only_matching {
            for &found in matches {
                self.write_prelude(
                    self.sunk.absolute_byte_offset() + found.start() as u64,
                    self.sunk.line_number(),
                    Some(found.start() as u64 + 1),
                )?;
                let bytes = &self.sunk.bytes()[found];
                self.write_colored_line(&[Match::new(0, bytes.len())], bytes)?;
            }
        } else if self.config().per_match {
            for &found in matches {
                self.write_prelude(
                    self.sunk.absolute_byte_offset() + found.start() as u64,
                    self.sunk.line_number(),
                    Some(found.start() as u64 + 1),
                )?;
                self.write_colored_line(&[found], self.sunk.bytes())?;
            }
        } else {
            self.write_prelude(
                self.sunk.absolute_byte_offset(),
                self.sunk.line_number(),
                Some(matches[0].start() as u64 + 1),
            )?;
            self.write_colored_line(matches, self.sunk.bytes())?;
        }
        Ok(())
    }

    fn sink_slow_multi_line(&self) -> io::Result<()> {
        if self.config().only_matching {
            return self.sink_slow_multi_line_only_matching();
        }
        if self.config().per_match {
            return self.sink_slow_multi_line_per_match();
        }
        let line_terminator = self.searcher.line_terminator().as_byte();
        let bytes = self.sunk.bytes();
        let matches = self.sunk.matches();
        let mut match_index = 0;
        let mut count = 0;
        let mut step = LineStep::new(line_terminator, 0, bytes.len());
        while let Some(mut line) = step.next_match(bytes) {
            self.write_prelude(
                self.sunk.absolute_byte_offset() + line.start() as u64,
                self.sunk.line_number().map(|number| number + count),
                Some(matches[0].start() as u64 + 1),
            )?;
            count += 1;
            self.trim_ascii_prefix(bytes, &mut line);
            if self.exceeds_max_columns(&bytes[line]) {
                self.write_exceeded_line(bytes, line, matches, &mut match_index)?;
            } else {
                self.write_colored_matches(bytes, line, matches, &mut match_index)?;
                self.write_line_terminator()?;
            }
        }
        Ok(())
    }

    fn sink_slow_multi_line_only_matching(&self) -> io::Result<()> {
        let line_terminator = self.searcher.line_terminator().as_byte();
        let spec = self.config().colors.matched();
        let bytes = self.sunk.bytes();
        let matches = self.sunk.matches();
        let mut match_index = 0;
        let mut count = 0;
        let mut step = LineStep::new(line_terminator, 0, bytes.len());
        while let Some(mut line) = step.next_match(bytes) {
            self.trim_line_terminator(bytes, &mut line);
            self.trim_ascii_prefix(bytes, &mut line);
            while !line.is_empty() {
                if matches[match_index].end() <= line.start() {
                    if match_index + 1 < matches.len() {
                        match_index += 1;
                        continue;
                    }
                    break;
                }
                let found = matches[match_index];
                if line.start() < found.start() {
                    line = line.with_start(line.end().min(found.start()));
                    continue;
                }
                let upto = line.end().min(found.end());
                self.write_prelude(
                    self.sunk.absolute_byte_offset() + found.start() as u64,
                    self.sunk.line_number().map(|number| number + count),
                    Some(found.start() as u64 + 1),
                )?;
                let this_line = line.with_end(upto);
                line = line.with_start(upto);
                if self.exceeds_max_columns(&bytes[this_line]) {
                    self.write_exceeded_line(bytes, this_line, matches, &mut match_index)?;
                } else {
                    self.write_spec(spec, &bytes[this_line])?;
                    self.write_line_terminator()?;
                }
            }
            count += 1;
        }
        Ok(())
    }

    fn sink_slow_multi_line_per_match(&self) -> io::Result<()> {
        let line_terminator = self.searcher.line_terminator().as_byte();
        let spec = self.config().colors.matched();
        let bytes = self.sunk.bytes();
        for &found in self.sunk.matches() {
            let mut count = 0;
            let mut step = LineStep::new(line_terminator, 0, bytes.len());
            while let Some(mut line) = step.next_match(bytes) {
                if line.start() >= found.end() {
                    break;
                }
                if line.end() <= found.start() {
                    count += 1;
                    continue;
                }
                self.write_prelude(
                    self.sunk.absolute_byte_offset() + line.start() as u64,
                    self.sunk.line_number().map(|number| number + count),
                    Some(found.start().saturating_sub(line.start()) as u64 + 1),
                )?;
                count += 1;
                self.trim_line_terminator(bytes, &mut line);
                self.trim_ascii_prefix(bytes, &mut line);
                if self.exceeds_max_columns(&bytes[line]) {
                    self.write_exceeded_line(bytes, line, &[found], &mut 0)?;
                    continue;
                }
                while !line.is_empty() {
                    if found.end() <= line.start() {
                        self.write(&bytes[line])?;
                        line = line.with_start(line.end());
                    } else if line.start() < found.start() {
                        let upto = line.end().min(found.start());
                        self.write(&bytes[line.with_end(upto)])?;
                        line = line.with_start(upto);
                    } else {
                        let upto = line.end().min(found.end());
                        self.write_spec(spec, &bytes[line.with_end(upto)])?;
                        line = line.with_start(upto);
                    }
                }
                self.write_line_terminator()?;
                if self.config().per_match_one_line {
                    break;
                }
            }
        }
        Ok(())
    }

    #[inline(always)]
    fn write_prelude(
        &self,
        absolute_byte_offset: u64,
        line_number: Option<u64>,
        column: Option<u64>,
    ) -> io::Result<()> {
        let mut prelude = PreludeWriter::new(self);
        prelude.start(line_number, column)?;
        prelude.write_path()?;
        prelude.write_line_number(line_number)?;
        prelude.write_column_number(column)?;
        prelude.write_byte_offset(absolute_byte_offset)?;
        prelude.end()
    }

    #[inline(always)]
    fn write_line(&self, line: &[u8]) -> io::Result<()> {
        let line = if self.config().trim_ascii {
            let full = Match::new(0, line.len());
            &line[trim_ascii_prefix(self.searcher.line_terminator(), line, full)]
        } else {
            line
        };
        if self.exceeds_max_columns(line) {
            return self.write_exceeded_line(
                line,
                Match::new(0, line.len()),
                self.sunk.matches(),
                &mut 0,
            );
        }
        self.write(line)?;
        if !self.searcher.line_terminator().is_suffix(line) {
            self.write_line_terminator()?;
        }
        Ok(())
    }

    fn write_colored_line(&self, matches: &[Match], bytes: &[u8]) -> io::Result<()> {
        let spec = self.config().colors.matched();
        if !self.writer().borrow().supports_color() || spec.is_none() {
            return self.write_line(bytes);
        }
        let mut line = Match::new(0, bytes.len());
        self.trim_ascii_prefix(bytes, &mut line);
        if self.exceeds_max_columns(bytes) {
            return self.write_exceeded_line(bytes, line, matches, &mut 0);
        }
        self.write_colored_matches(bytes, line, matches, &mut 0)?;
        self.write_line_terminator()
    }

    fn write_colored_matches(
        &self,
        bytes: &[u8],
        mut line: Match,
        matches: &[Match],
        match_index: &mut usize,
    ) -> io::Result<()> {
        self.trim_line_terminator(bytes, &mut line);
        if matches.is_empty() {
            return self.write(&bytes[line]);
        }
        self.start_line_highlight()?;
        while !line.is_empty() {
            if matches[*match_index].end() <= line.start() {
                if *match_index + 1 < matches.len() {
                    *match_index += 1;
                    continue;
                }
                self.end_color_match()?;
                self.write(&bytes[line])?;
                break;
            }
            let found = matches[*match_index];
            if line.start() < found.start() {
                let upto = line.end().min(found.start());
                self.end_color_match()?;
                self.write(&bytes[line.with_end(upto)])?;
                line = line.with_start(upto);
            } else {
                let upto = line.end().min(found.end());
                self.start_color_match()?;
                self.write(&bytes[line.with_end(upto)])?;
                line = line.with_start(upto);
            }
        }
        self.end_color_match()?;
        self.end_line_highlight()
    }

    fn write_exceeded_line(
        &self,
        bytes: &[u8],
        mut line: Match,
        matches: &[Match],
        match_index: &mut usize,
    ) -> io::Result<()> {
        if self.config().max_columns_preview {
            let original = line;
            let columns = self
                .config()
                .max_columns
                .map_or(0, |columns| columns as usize);
            let end = super::grapheme::end_of_graphemes(&bytes[line], columns) + line.start();
            line = line.with_end(end);
            self.write_colored_matches(bytes, line, matches, match_index)?;
            if matches.is_empty() {
                self.write(b" [... omitted end of long line]")?;
            } else {
                let remaining = matches
                    .iter()
                    .filter(|found| found.start() >= line.end() && found.start() < original.end())
                    .count();
                let tense = if remaining == 1 { "match" } else { "matches" };
                write!(
                    self.writer().borrow_mut(),
                    " [... {remaining} more {tense}]"
                )?;
            }
            return self.write_line_terminator();
        }
        if self.sunk.original_matches().is_empty() || self.config().only_matching {
            if self.is_context() {
                self.write(b"[Omitted long context line]")?;
            } else {
                self.write(b"[Omitted long matching line]")?;
            }
        } else {
            write!(
                self.writer().borrow_mut(),
                "[Omitted long line with {} matches]",
                self.sunk.original_matches().len()
            )?;
        }
        self.write_line_terminator()
    }

    fn write_path_line(&self) -> io::Result<()> {
        let Some(path) = self.path() else {
            return Ok(());
        };
        self.write_path_hyperlink(path)?;
        match self.config().path_terminator {
            Some(terminator) => self.write(&[terminator]),
            None => self.write_line_terminator(),
        }
    }

    fn write_search_prelude(&self) -> io::Result<()> {
        if self.writer().borrow().count() > 0 {
            return Ok(());
        }
        if let Some(separator) = self.config().separator_search.as_ref()
            && self.writer().borrow().total_count() > 0
        {
            self.write(separator)?;
            self.write_line_terminator()?;
        }
        if self.config().heading {
            self.write_path_line()?;
        }
        Ok(())
    }

    fn write_binary_message(&self, offset: u64) -> io::Result<()> {
        if !self.sink.has_match() {
            return Ok(());
        }
        let detection = self.searcher.binary_detection();
        let (byte, message) = if let Some(byte) = detection.quit_byte() {
            (byte, "WARNING: stopped searching binary file after match")
        } else if let Some(byte) = detection.convert_byte() {
            (byte, "binary file matches")
        } else {
            return Ok(());
        };
        if let Some(path) = self.path() {
            self.write_path_hyperlink(path)?;
            self.write(b": ")?;
        }
        let remainder = format!(
            "{message} (found {} byte around offset {offset})\n",
            quoted_byte(byte)
        );
        self.write(remainder.as_bytes())
    }

    fn write_context_separator(&self) -> io::Result<()> {
        if let Some(separator) = self.config().separator_context.as_ref() {
            self.write(separator)?;
            self.write_line_terminator()?;
        }
        Ok(())
    }

    fn write_line_terminator(&self) -> io::Result<()> {
        self.write(self.searcher.line_terminator().as_bytes())
    }

    fn write_spec(&self, spec: &ColorSpec, bytes: &[u8]) -> io::Result<()> {
        let mut writer = self.writer().borrow_mut();
        writer.set_color(spec)?;
        writer.write_all(bytes)?;
        writer.reset()
    }

    #[inline(always)]
    fn write_path(&self, path: &PrinterPath<'_>) -> io::Result<()> {
        self.write_spec(self.config().colors.path(), path.as_bytes())
    }

    fn write_path_hyperlink(&self, path: &PrinterPath<'_>) -> io::Result<()> {
        let status = self.start_hyperlink(path, None, None)?;
        self.write_path(path)?;
        self.end_hyperlink(status)
    }

    fn start_hyperlink(
        &self,
        path: &PrinterPath<'_>,
        line_number: Option<u64>,
        column: Option<u64>,
    ) -> io::Result<InterpolatorStatus> {
        let Some(hyperlink_path) = path.as_hyperlink() else {
            return Ok(InterpolatorStatus::Inactive);
        };
        let values = hyperlink::Values::new(hyperlink_path)
            .line(line_number)
            .column(column);
        self.sink
            .interpolator
            .begin(&values, &mut *self.writer().borrow_mut())
    }

    fn end_hyperlink(&self, status: InterpolatorStatus) -> io::Result<()> {
        self.sink
            .interpolator
            .finish(status, &mut *self.writer().borrow_mut())
    }

    fn start_color_match(&self) -> io::Result<()> {
        if self.in_color_match.get() {
            return Ok(());
        }
        self.writer()
            .borrow_mut()
            .set_color(self.config().colors.matched())?;
        self.in_color_match.set(true);
        Ok(())
    }

    fn end_color_match(&self) -> io::Result<()> {
        if !self.in_color_match.get() {
            return Ok(());
        }
        if self.highlight_on() {
            self.writer()
                .borrow_mut()
                .set_color(self.config().colors.highlight())?;
        } else {
            self.writer().borrow_mut().reset()?;
        }
        self.in_color_match.set(false);
        Ok(())
    }

    fn highlight_on(&self) -> bool {
        !self.config().colors.highlight().is_none() && !self.is_context()
    }

    fn start_line_highlight(&self) -> io::Result<()> {
        if self.highlight_on() {
            self.writer()
                .borrow_mut()
                .set_color(self.config().colors.highlight())?;
        }
        Ok(())
    }

    fn end_line_highlight(&self) -> io::Result<()> {
        if self.highlight_on() {
            self.writer().borrow_mut().reset()?;
        }
        Ok(())
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        self.writer().borrow_mut().write_all(bytes)
    }

    fn trim_line_terminator(&self, bytes: &[u8], line: &mut Match) {
        trim_line_terminator(self.searcher, bytes, line);
    }

    fn is_context(&self) -> bool {
        self.sunk.context_kind().is_some()
    }

    #[inline(always)]
    fn config(&self) -> &'a Config {
        &self.sink.standard.config
    }

    fn writer(&self) -> &'a RefCell<CounterWriter<W>> {
        &self.sink.standard.writer
    }

    fn path(&self) -> Option<&'a PrinterPath<'a>> {
        self.sink.path.as_ref()
    }

    fn separator_field(&self) -> &'a [u8] {
        if self.is_context() {
            &self.config().separator_field_context
        } else {
            &self.config().separator_field_match
        }
    }

    fn exceeds_max_columns(&self, line: &[u8]) -> bool {
        self.config()
            .max_columns
            .is_some_and(|columns| line.len() as u64 > columns)
    }

    fn multi_line(&self) -> bool {
        self.searcher.multi_line_with_matcher(&self.sink.matcher)
    }

    fn trim_ascii_prefix(&self, bytes: &[u8], range: &mut Match) {
        if self.config().trim_ascii {
            *range = trim_ascii_prefix(self.searcher.line_terminator(), bytes, *range);
        }
    }
}

enum PreludeSeparator {
    None,
    Field,
    PathTerminator,
}

struct PreludeWriter<'a, M: Matcher, W> {
    standard: &'a StandardImpl<'a, M, W>,
    next_separator: PreludeSeparator,
    field_separator: &'a [u8],
    hyperlink: InterpolatorStatus,
}

impl<'a, M: Matcher, W: WriteColor> PreludeWriter<'a, M, W> {
    #[inline(always)]
    fn new(standard: &'a StandardImpl<'a, M, W>) -> PreludeWriter<'a, M, W> {
        PreludeWriter {
            standard,
            next_separator: PreludeSeparator::None,
            field_separator: standard.separator_field(),
            hyperlink: InterpolatorStatus::Inactive,
        }
    }

    #[inline(always)]
    fn config(&self) -> &'a Config {
        self.standard.config()
    }

    #[inline(always)]
    fn start(&mut self, line_number: Option<u64>, column: Option<u64>) -> io::Result<()> {
        let Some(path) = self.standard.path() else {
            return Ok(());
        };
        if self.config().hyperlink.format().is_line_dependent() || !self.config().heading {
            self.hyperlink = self.standard.start_hyperlink(path, line_number, column)?;
        }
        Ok(())
    }

    #[inline(always)]
    fn end(&mut self) -> io::Result<()> {
        let status = std::mem::replace(&mut self.hyperlink, InterpolatorStatus::Inactive);
        self.standard.end_hyperlink(status)?;
        self.write_separator()
    }

    #[inline(always)]
    fn write_path(&mut self) -> io::Result<()> {
        if self.config().heading {
            return Ok(());
        }
        let Some(path) = self.standard.path() else {
            return Ok(());
        };
        self.write_separator()?;
        self.standard.write_path(path)?;
        self.next_separator = if self.config().path_terminator.is_some() {
            PreludeSeparator::PathTerminator
        } else {
            PreludeSeparator::Field
        };
        Ok(())
    }

    #[inline(always)]
    fn write_line_number(&mut self, line_number: Option<u64>) -> io::Result<()> {
        let Some(line_number) = line_number else {
            return Ok(());
        };
        self.write_number(self.config().colors.line(), line_number)
    }

    #[inline(always)]
    fn write_column_number(&mut self, column: Option<u64>) -> io::Result<()> {
        if !self.config().column {
            return Ok(());
        }
        let Some(column) = column else {
            return Ok(());
        };
        self.write_number(self.config().colors.column(), column)
    }

    #[inline(always)]
    fn write_byte_offset(&mut self, offset: u64) -> io::Result<()> {
        if !self.config().byte_offset {
            return Ok(());
        }
        self.write_number(self.config().colors.column(), offset)
    }

    fn write_number(&mut self, spec: &ColorSpec, number: u64) -> io::Result<()> {
        self.write_separator()?;
        self.standard
            .write_spec(spec, DecimalFormatter::new(number).as_bytes())?;
        self.next_separator = PreludeSeparator::Field;
        Ok(())
    }

    #[inline(always)]
    fn write_separator(&mut self) -> io::Result<()> {
        match self.next_separator {
            PreludeSeparator::None => {}
            PreludeSeparator::Field => self.standard.write(self.field_separator)?,
            PreludeSeparator::PathTerminator => {
                if let Some(terminator) = self.config().path_terminator {
                    self.standard.write(&[terminator])?;
                }
            }
        }
        self.next_separator = PreludeSeparator::None;
        Ok(())
    }
}

fn quoted_byte(byte: u8) -> String {
    match byte {
        0 => "\"\\0\"".to_string(),
        0x01..=0x7F => format!("\"{}\"", byte.escape_ascii()),
        _ => format!("\"\\x{byte:02x}\""),
    }
}
