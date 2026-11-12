use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::ops::Range;
use std::path::Path;

use crate::matcher::{LineTerminator, Matcher};

use self::decode::DecodeReader;
use self::line_buffer::{BinaryDetectionMode, LineBuffer};
pub(crate) use self::lines::{LineIter, LineStep};

mod core;
mod decode;
mod line_buffer;
mod lines;
mod strategy;

const DECODE_BUFFER_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct BinaryDetection(BinaryDetectionMode);

impl BinaryDetection {
    pub(crate) fn none() -> BinaryDetection {
        BinaryDetection(BinaryDetectionMode::None)
    }

    pub(crate) fn quit(binary_byte: u8) -> BinaryDetection {
        BinaryDetection(BinaryDetectionMode::Quit(binary_byte))
    }

    pub(crate) fn convert(binary_byte: u8) -> BinaryDetection {
        BinaryDetection(BinaryDetectionMode::Convert(binary_byte))
    }

    pub(crate) fn quit_byte(&self) -> Option<u8> {
        match self.0 {
            BinaryDetectionMode::Quit(byte) => Some(byte),
            _ => None,
        }
    }

    pub(crate) fn convert_byte(&self) -> Option<u8> {
        match self.0 {
            BinaryDetectionMode::Convert(byte) => Some(byte),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Encoding(&'static encoding_rs::Encoding);

impl Encoding {
    pub(crate) fn new(label: &str) -> Result<Encoding, ConfigError> {
        match encoding_rs::Encoding::for_label_no_replacement(label.as_bytes()) {
            Some(encoding) => Ok(Encoding(encoding)),
            None => Err(ConfigError::UnknownEncoding {
                label: label.as_bytes().to_vec(),
            }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ConfigError {
    MismatchedLineTerminators {
        matcher: LineTerminator,
        searcher: LineTerminator,
    },
    UnknownEncoding {
        label: Vec<u8>,
    },
}

impl std::error::Error for ConfigError {}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::MismatchedLineTerminators { matcher, searcher } => write!(
                f,
                "grep config error: mismatched line terminators, matcher has {matcher:?} but searcher has {searcher:?}"
            ),
            ConfigError::UnknownEncoding { label } => {
                write!(
                    f,
                    "grep config error: unknown encoding: {}",
                    String::from_utf8_lossy(label)
                )
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Config {
    line_terminator: LineTerminator,
    invert_match: bool,
    after_context: usize,
    before_context: usize,
    passthru: bool,
    line_number: bool,
    binary: BinaryDetection,
    multi_line: bool,
    encoding: Option<Encoding>,
    bom_sniffing: bool,
    stop_on_nonmatch: bool,
    max_matches: Option<u64>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            line_terminator: LineTerminator::default(),
            invert_match: false,
            after_context: 0,
            before_context: 0,
            passthru: false,
            line_number: true,
            binary: BinaryDetection::default(),
            multi_line: false,
            encoding: None,
            bom_sniffing: true,
            stop_on_nonmatch: false,
            max_matches: None,
        }
    }
}

impl Config {
    fn max_context(&self) -> usize {
        self.before_context.max(self.after_context)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SearcherBuilder {
    config: Config,
}

impl SearcherBuilder {
    pub(crate) fn new() -> SearcherBuilder {
        SearcherBuilder::default()
    }

    pub(crate) fn build(&self) -> Searcher {
        let mut config = self.config.clone();
        if config.passthru {
            config.before_context = 0;
            config.after_context = 0;
        }
        Searcher {
            config,
            decode_buffer: vec![0; DECODE_BUFFER_BYTES],
            line_buffer: LineBuffer::new(),
            multi_line_buffer: Vec::new(),
        }
    }

    pub(crate) fn line_terminator(
        &mut self,
        line_terminator: LineTerminator,
    ) -> &mut SearcherBuilder {
        self.config.line_terminator = line_terminator;
        self
    }

    pub(crate) fn invert_match(&mut self, yes: bool) -> &mut SearcherBuilder {
        self.config.invert_match = yes;
        self
    }

    pub(crate) fn line_number(&mut self, yes: bool) -> &mut SearcherBuilder {
        self.config.line_number = yes;
        self
    }

    pub(crate) fn multi_line(&mut self, yes: bool) -> &mut SearcherBuilder {
        self.config.multi_line = yes;
        self
    }

    pub(crate) fn after_context(&mut self, line_count: usize) -> &mut SearcherBuilder {
        self.config.after_context = line_count;
        self
    }

    pub(crate) fn before_context(&mut self, line_count: usize) -> &mut SearcherBuilder {
        self.config.before_context = line_count;
        self
    }

    pub(crate) fn passthru(&mut self, yes: bool) -> &mut SearcherBuilder {
        self.config.passthru = yes;
        self
    }

    pub(crate) fn binary_detection(&mut self, detection: BinaryDetection) -> &mut SearcherBuilder {
        self.config.binary = detection;
        self
    }

    pub(crate) fn encoding(&mut self, encoding: Option<Encoding>) -> &mut SearcherBuilder {
        self.config.encoding = encoding;
        self
    }

    pub(crate) fn bom_sniffing(&mut self, yes: bool) -> &mut SearcherBuilder {
        self.config.bom_sniffing = yes;
        self
    }

    pub(crate) fn stop_on_nonmatch(&mut self, yes: bool) -> &mut SearcherBuilder {
        self.config.stop_on_nonmatch = yes;
        self
    }

    pub(crate) fn max_matches(&mut self, limit: Option<u64>) -> &mut SearcherBuilder {
        self.config.max_matches = limit;
        self
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Searcher {
    config: Config,
    decode_buffer: Vec<u8>,
    line_buffer: LineBuffer,
    multi_line_buffer: Vec<u8>,
}

impl Searcher {
    pub(crate) fn search_path<M: Matcher, S: Sink>(
        &mut self,
        matcher: M,
        path: &Path,
        sink: S,
    ) -> io::Result<()> {
        let file = File::open(path)?;
        if !self.multi_line_with_matcher(&matcher) {
            return self.search_reader(matcher, file, sink);
        }
        self.check_config(&matcher)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let capacity = file
            .metadata()
            .map_or(0, |metadata| metadata.len() as usize + 1);
        let mut decode_buffer = std::mem::take(&mut self.decode_buffer);
        let mut slice = std::mem::take(&mut self.multi_line_buffer);
        slice.clear();
        slice.reserve(capacity);
        let result = self
            .decoder(file, &mut decode_buffer)
            .read_to_end(&mut slice)
            .and_then(|_| strategy::search_multi_line(self, matcher, &slice, sink));
        self.decode_buffer = decode_buffer;
        self.multi_line_buffer = slice;
        result
    }

    pub(crate) fn search_reader<M: Matcher, R: Read, S: Sink>(
        &mut self,
        matcher: M,
        reader: R,
        sink: S,
    ) -> io::Result<()> {
        self.check_config(&matcher)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let mut decode_buffer = std::mem::take(&mut self.decode_buffer);
        let mut decoder = self.decoder(reader, &mut decode_buffer);
        let result = if self.multi_line_with_matcher(&matcher) {
            let mut slice = std::mem::take(&mut self.multi_line_buffer);
            slice.clear();
            let result = decoder
                .read_to_end(&mut slice)
                .and_then(|_| strategy::search_multi_line(self, matcher, &slice, sink));
            self.multi_line_buffer = slice;
            result
        } else {
            let mut line_buffer = std::mem::take(&mut self.line_buffer);
            line_buffer.begin(self.config.line_terminator.as_byte(), self.config.binary.0);
            let result =
                strategy::search_by_line(self, matcher, &mut decoder, &mut line_buffer, sink);
            self.line_buffer = line_buffer;
            result
        };
        self.decode_buffer = decode_buffer;
        result
    }

    fn decoder<'b, R: Read>(&self, reader: R, buffer: &'b mut [u8]) -> DecodeReader<'b, R> {
        let encoding = self.config.encoding.as_ref().map(|encoding| encoding.0);
        DecodeReader::new(reader, buffer, encoding, self.config.bom_sniffing)
    }

    pub(crate) fn set_binary_detection(&mut self, detection: BinaryDetection) {
        self.config.binary = detection;
    }

    fn check_config<M: Matcher>(&self, matcher: M) -> Result<(), ConfigError> {
        let Some(matcher_line_terminator) = matcher.line_terminator() else {
            return Ok(());
        };
        if matcher_line_terminator != self.config.line_terminator {
            return Err(ConfigError::MismatchedLineTerminators {
                matcher: matcher_line_terminator,
                searcher: self.config.line_terminator,
            });
        }
        Ok(())
    }

    pub(crate) fn line_terminator(&self) -> LineTerminator {
        self.config.line_terminator
    }

    pub(crate) fn binary_detection(&self) -> &BinaryDetection {
        &self.config.binary
    }

    pub(crate) fn invert_match(&self) -> bool {
        self.config.invert_match
    }

    pub(crate) fn multi_line_with_matcher<M: Matcher>(&self, matcher: M) -> bool {
        if !self.config.multi_line {
            return false;
        }
        if matcher.line_terminator() == Some(self.config.line_terminator) {
            return false;
        }
        if let Some(non_matching) = matcher.non_matching_bytes()
            && non_matching.contains(self.config.line_terminator.as_byte())
        {
            return false;
        }
        true
    }
}

pub(crate) trait Sink {
    fn matched(&mut self, searcher: &Searcher, sink_match: &SinkMatch<'_>) -> io::Result<bool>;

    #[inline]
    fn context(&mut self, _searcher: &Searcher, _context: &SinkContext<'_>) -> io::Result<bool> {
        Ok(true)
    }

    #[inline]
    fn context_break(&mut self, _searcher: &Searcher) -> io::Result<bool> {
        Ok(true)
    }

    #[inline]
    fn binary_data(&mut self, _searcher: &Searcher, _binary_byte_offset: u64) -> io::Result<bool> {
        Ok(true)
    }

    #[inline]
    fn begin(&mut self, _searcher: &Searcher) -> io::Result<bool> {
        Ok(true)
    }

    #[inline]
    fn finish(&mut self, _searcher: &Searcher, _finish: &SinkFinish) -> io::Result<()> {
        Ok(())
    }
}

impl<S: Sink> Sink for &mut S {
    #[inline]
    fn matched(&mut self, searcher: &Searcher, sink_match: &SinkMatch<'_>) -> io::Result<bool> {
        (**self).matched(searcher, sink_match)
    }

    #[inline]
    fn context(&mut self, searcher: &Searcher, context: &SinkContext<'_>) -> io::Result<bool> {
        (**self).context(searcher, context)
    }

    #[inline]
    fn context_break(&mut self, searcher: &Searcher) -> io::Result<bool> {
        (**self).context_break(searcher)
    }

    #[inline]
    fn binary_data(&mut self, searcher: &Searcher, binary_byte_offset: u64) -> io::Result<bool> {
        (**self).binary_data(searcher, binary_byte_offset)
    }

    #[inline]
    fn begin(&mut self, searcher: &Searcher) -> io::Result<bool> {
        (**self).begin(searcher)
    }

    #[inline]
    fn finish(&mut self, searcher: &Searcher, finish: &SinkFinish) -> io::Result<()> {
        (**self).finish(searcher, finish)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SinkFinish {
    byte_count: u64,
    binary_byte_offset: Option<u64>,
}

impl SinkFinish {
    #[inline(always)]
    pub(crate) fn byte_count(&self) -> u64 {
        self.byte_count
    }

    #[inline(always)]
    pub(crate) fn binary_byte_offset(&self) -> Option<u64> {
        self.binary_byte_offset
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SinkMatch<'b> {
    line_terminator: LineTerminator,
    bytes: &'b [u8],
    absolute_byte_offset: u64,
    line_number: Option<u64>,
    buffer: &'b [u8],
    bytes_range_in_buffer: Range<usize>,
}

impl<'b> SinkMatch<'b> {
    #[inline(always)]
    pub(crate) fn bytes(&self) -> &'b [u8] {
        self.bytes
    }

    #[inline(always)]
    pub(crate) fn lines(&self) -> LineIter<'b> {
        LineIter::new(self.line_terminator.as_byte(), self.bytes)
    }

    #[inline(always)]
    pub(crate) fn absolute_byte_offset(&self) -> u64 {
        self.absolute_byte_offset
    }

    #[inline(always)]
    pub(crate) fn line_number(&self) -> Option<u64> {
        self.line_number
    }

    #[inline(always)]
    pub(crate) fn buffer(&self) -> &'b [u8] {
        self.buffer
    }

    #[inline(always)]
    pub(crate) fn bytes_range_in_buffer(&self) -> Range<usize> {
        self.bytes_range_in_buffer.clone()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SinkContextKind {
    Before,
    After,
    Other,
}

#[derive(Clone, Debug)]
pub(crate) struct SinkContext<'b> {
    bytes: &'b [u8],
    kind: SinkContextKind,
    absolute_byte_offset: u64,
    line_number: Option<u64>,
}

impl<'b> SinkContext<'b> {
    #[inline(always)]
    pub(crate) fn bytes(&self) -> &'b [u8] {
        self.bytes
    }

    #[inline(always)]
    pub(crate) fn kind(&self) -> SinkContextKind {
        self.kind
    }

    #[inline(always)]
    pub(crate) fn absolute_byte_offset(&self) -> u64 {
        self.absolute_byte_offset
    }

    #[inline(always)]
    pub(crate) fn line_number(&self) -> Option<u64> {
        self.line_number
    }
}
