use std::cell::RefCell;
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;

use super::color::ColorSpecs;
use super::hyperlink::{self, HyperlinkConfig, InterpolatorStatus};
use super::stats::{Started, Stats};
use super::util::{CounterWriter, DecimalFormatter, PrinterPath, find_iter_at_in_context};
use crate::ansi::WriteColor;
use crate::matcher::Matcher;
use crate::searcher::{Searcher, Sink, SinkFinish, SinkMatch};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SummaryKind {
    Count,
    CountMatches,
    PathWithMatch,
    PathWithoutMatch,
    QuietWithMatch,
    QuietWithoutMatch,
}

impl SummaryKind {
    fn requires_path(&self) -> bool {
        matches!(
            self,
            SummaryKind::PathWithMatch | SummaryKind::PathWithoutMatch
        )
    }

    fn requires_stats(&self) -> bool {
        matches!(self, SummaryKind::CountMatches)
    }

    fn quit_early(&self) -> bool {
        matches!(
            self,
            SummaryKind::PathWithMatch | SummaryKind::QuietWithMatch
        )
    }
}

#[derive(Clone, Debug)]
struct Config {
    kind: SummaryKind,
    colors: ColorSpecs,
    hyperlink: HyperlinkConfig,
    stats: bool,
    path: bool,
    exclude_zero: bool,
    separator_field: Arc<Vec<u8>>,
    separator_path: Option<u8>,
    path_terminator: Option<u8>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            kind: SummaryKind::Count,
            colors: ColorSpecs::default(),
            hyperlink: HyperlinkConfig::default(),
            stats: false,
            path: true,
            exclude_zero: true,
            separator_field: Arc::new(b":".to_vec()),
            separator_path: None,
            path_terminator: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SummaryBuilder {
    config: Config,
}

impl SummaryBuilder {
    pub(crate) fn new() -> SummaryBuilder {
        SummaryBuilder::default()
    }

    pub(crate) fn build<W: WriteColor>(&self, writer: W) -> Summary<W> {
        Summary {
            config: self.config.clone(),
            writer: RefCell::new(CounterWriter::new(writer)),
        }
    }

    pub(crate) fn kind(&mut self, kind: SummaryKind) -> &mut SummaryBuilder {
        self.config.kind = kind;
        self
    }

    pub(crate) fn color_specs(&mut self, specs: ColorSpecs) -> &mut SummaryBuilder {
        self.config.colors = specs;
        self
    }

    pub(crate) fn hyperlink(&mut self, config: HyperlinkConfig) -> &mut SummaryBuilder {
        self.config.hyperlink = config;
        self
    }

    pub(crate) fn stats(&mut self, yes: bool) -> &mut SummaryBuilder {
        self.config.stats = yes;
        self
    }

    pub(crate) fn path(&mut self, yes: bool) -> &mut SummaryBuilder {
        self.config.path = yes;
        self
    }

    pub(crate) fn exclude_zero(&mut self, yes: bool) -> &mut SummaryBuilder {
        self.config.exclude_zero = yes;
        self
    }

    pub(crate) fn separator_field(&mut self, separator: Vec<u8>) -> &mut SummaryBuilder {
        self.config.separator_field = Arc::new(separator);
        self
    }

    pub(crate) fn separator_path(&mut self, separator: Option<u8>) -> &mut SummaryBuilder {
        self.config.separator_path = separator;
        self
    }

    pub(crate) fn path_terminator(&mut self, terminator: Option<u8>) -> &mut SummaryBuilder {
        self.config.path_terminator = terminator;
        self
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Summary<W> {
    config: Config,
    writer: RefCell<CounterWriter<W>>,
}

impl<W: WriteColor> Summary<W> {
    pub(crate) fn sink_with_path<'p, 's, M: Matcher>(
        &'s mut self,
        matcher: M,
        path: &'p Path,
    ) -> SummarySink<'p, 's, M, W> {
        let path = (self.config.path || self.config.kind.requires_path())
            .then(|| PrinterPath::new(path).with_separator(self.config.separator_path));
        let interpolator = hyperlink::Interpolator::new(&self.config.hyperlink);
        let stats = (self.config.stats || self.config.kind.requires_stats()).then(Stats::new);
        SummarySink {
            matcher,
            summary: self,
            interpolator,
            path,
            start_time: Started::now(),
            match_count: 0,
            stats,
        }
    }
}

impl<W> Summary<W> {
    pub(crate) fn get_mut(&mut self) -> &mut W {
        self.writer.get_mut().get_mut()
    }
}

#[derive(Debug)]
pub(crate) struct SummarySink<'p, 's, M: Matcher, W> {
    matcher: M,
    summary: &'s mut Summary<W>,
    interpolator: hyperlink::Interpolator,
    path: Option<PrinterPath<'p>>,
    start_time: Started,
    match_count: u64,
    stats: Option<Stats>,
}

impl<M: Matcher, W: WriteColor> SummarySink<'_, '_, M, W> {
    pub(crate) fn has_match(&self) -> bool {
        match self.summary.config.kind {
            SummaryKind::PathWithoutMatch | SummaryKind::QuietWithoutMatch => self.match_count == 0,
            _ => self.match_count > 0,
        }
    }

    pub(crate) fn stats(&self) -> Option<&Stats> {
        self.stats.as_ref()
    }

    fn write_path_line(&mut self, searcher: &Searcher) -> io::Result<()> {
        if self.path.is_none() {
            return Ok(());
        }
        self.write_path()?;
        match self.summary.config.path_terminator {
            Some(terminator) => self.write(&[terminator]),
            None => self.write(searcher.line_terminator().as_bytes()),
        }
    }

    fn write_path_field(&mut self) -> io::Result<()> {
        if self.path.is_none() {
            return Ok(());
        }
        self.write_path()?;
        match self.summary.config.path_terminator {
            Some(terminator) => self.write(&[terminator]),
            None => self.write(&self.summary.config.separator_field.clone()),
        }
    }

    fn write_path(&mut self) -> io::Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        let status = match path.as_hyperlink() {
            None => InterpolatorStatus::Inactive,
            Some(hyperlink_path) => {
                let values = hyperlink::Values::new(hyperlink_path);
                self.interpolator
                    .begin(&values, &mut *self.summary.writer.borrow_mut())?
            }
        };
        let mut writer = self.summary.writer.borrow_mut();
        writer.set_color(self.summary.config.colors.path())?;
        writer.write_all(path.as_bytes())?;
        writer.reset()?;
        self.interpolator.finish(status, &mut *writer)
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        self.summary.writer.borrow_mut().write_all(bytes)
    }

    fn write_count(&mut self, searcher: &Searcher, count: u64) -> io::Result<()> {
        self.write_path_field()?;
        self.write(DecimalFormatter::new(count).as_bytes())?;
        self.write(searcher.line_terminator().as_bytes())
    }
}

impl<M: Matcher, W: WriteColor> Sink for SummarySink<'_, '_, M, W> {
    fn matched(&mut self, searcher: &Searcher, sink_match: &SinkMatch<'_>) -> io::Result<bool> {
        let is_multi_line = searcher.multi_line_with_matcher(&self.matcher);
        let sink_match_count = if self.stats.is_none() && !is_multi_line {
            1
        } else {
            let mut count = 0;
            find_iter_at_in_context(
                searcher,
                &self.matcher,
                sink_match.buffer(),
                sink_match.bytes_range_in_buffer(),
                |_| {
                    count += 1;
                    true
                },
            );
            count.max(1)
        };
        self.match_count += if is_multi_line { sink_match_count } else { 1 };
        match self.stats.as_mut() {
            Some(stats) => {
                stats.add_matches(sink_match_count);
                stats.add_matched_lines(sink_match.lines().count() as u64);
            }
            None if self.summary.config.kind.quit_early() => return Ok(false),
            None => {}
        }
        Ok(true)
    }

    fn begin(&mut self, _searcher: &Searcher) -> io::Result<bool> {
        if self.path.is_none() && self.summary.config.kind.requires_path() {
            return Err(io::Error::other(format!(
                "output kind {:?} requires a file path",
                self.summary.config.kind
            )));
        }
        self.summary.writer.borrow_mut().reset_count();
        self.start_time = Started::now();
        self.match_count = 0;
        Ok(true)
    }

    fn finish(&mut self, searcher: &Searcher, finish: &SinkFinish) -> io::Result<()> {
        let bytes_printed = self.summary.writer.borrow().count();
        let matched = self.match_count > 0;
        if let Some(stats) = self.stats.as_mut() {
            stats.record_search(self.start_time, matched, finish.byte_count(), bytes_printed);
        }
        if finish.binary_byte_offset().is_some()
            && searcher.binary_detection().quit_byte().is_some()
        {
            self.match_count = 0;
            return Ok(());
        }
        let show_count = !self.summary.config.exclude_zero || self.match_count > 0;
        match self.summary.config.kind {
            SummaryKind::Count if show_count => self.write_count(searcher, self.match_count)?,
            SummaryKind::CountMatches if show_count => {
                let stats = self
                    .stats
                    .as_ref()
                    .expect("counting matches tracks statistics");
                let matches = stats.matches();
                self.write_count(searcher, matches)?;
            }
            SummaryKind::PathWithMatch if self.match_count > 0 => self.write_path_line(searcher)?,
            SummaryKind::PathWithoutMatch if self.match_count == 0 => {
                self.write_path_line(searcher)?
            }
            _ => {}
        }
        Ok(())
    }
}
